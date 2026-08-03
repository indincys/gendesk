//! 工单收录：扫描 → 校验 → 阈值 → 导入提示词与参考图 → 按参数分桶建批 → 移档留证。
//!
//! ## 顺序不是随便排的
//!
//! 校验（纯读）→ **算任务数**（纯算）→ 记账（intake_jobs 置 running）→ 导入 →
//! **建批**（花钱）→ 记账置 done → 写回执 → 移档。
//!
//! 阈值判定排在导入之前，所以超阈值时**库里一个字节都没变**——不存在「提示词进了库
//! 但没建批」这种要人去收拾的中间态。
//!
//! 记账在**动手之前**：进程若在导入中途被杀，那一行会留在 `running`，下次扫描据此
//! 跳过它，而不是从头再来一遍——半个工单重放会造出重复的提示词与重复的批次。
//! 这与 v0.15.0「提交成功即写 submit_id」是同一条教训：不可撤回的动作，标记要先落。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use sqlx::SqlitePool;

use super::{JobView, Plan};
use crate::commands::prompts::{build_preview_from_parsed, commit_preview};
use crate::commands::refs::ingest_one;
use crate::db::now_unix;
use crate::db::repo::intake as repo;
use crate::db::repo::refs as refs_repo;
use crate::db::repo::tasks as task_repo;
use crate::engine::{self, Engine, RefMapping};
use crate::error::{AppError, AppResult};
use crate::files::DataDirs;
use crate::importer::ParsedGroup;

/// 建批之后唤醒调度器。
///
/// 抽成 trait 只为一件事：让收录的端到端测试不必启动整个引擎（那要 provider 工厂、
/// 事件汇、密钥存储三样东西）。同 `engine::events::EventSink` 的理由——
/// 「这条链路到底把什么写进了库」是本模块唯一值得测的东西，不该被装配成本挡住。
pub trait Kick: Send + Sync {
    fn kick(&self);
}

impl Kick for Engine {
    fn kick(&self) {
        Engine::kick(self);
    }
}

/// 收录进度回报。与 [`Kick`] 同一个理由抽成 trait：本模块不该依赖 Tauri。
///
/// 存在的理由写在 `commands::refs::RefImportProgress` 的注释里，那是同一个故障：
/// 一条十几秒不出声的链路，人会当它死了然后反复重按——于是同一批图进库五六遍。
/// 一份 120 张的工单收录要几十秒，没有进度就是同样的剧本。
pub trait ProgressSink: Send + Sync {
    /// 参考图落盘进度。`done` 单调递增，收完等于 `total`。
    fn refs(&self, job_id: &str, done: i64, total: i64);
}

/// 不回报。后台扫描与测试用——那些场景没有人在等着看进度。
pub struct NoProgress;

impl ProgressSink for NoProgress {
    fn refs(&self, _job_id: &str, _done: i64, _total: i64) {}
}

/// 收录所需的全部句柄。命令层与后台监听各自构造一个，字段都是 clone 友好的。
#[derive(Clone)]
pub struct Ctx {
    pub pool: SqlitePool,
    pub dirs: Arc<DataDirs>,
    pub engine: Arc<dyn Kick>,
    pub progress: Arc<dyn ProgressSink>,
    /// 任务数超过它就不自动开跑，转「待确认」。`<= 0` = 不限。
    pub threshold: i64,
    /// 扫描互斥锁。**必须全进程共用同一把**（见 [`scan`]）。
    pub scan_lock: Arc<tokio::sync::Mutex<()>>,
}

/// 一轮扫描的结果：本轮真正处理掉的工单（已跳过的不在其中）。
pub type ScanResult = Vec<JobView>;

/// 扫描收件目录并逐个收录。**幂等**：已记账的工单一律跳过。
///
/// **全程持锁串行。** 两条路径会同时触发扫描：设置页点「确认开跑」会直接扫一轮，
/// 而写下的 `确认.txt` 落在被监听的收件目录里，watcher 防抖 2 秒后也会扫一轮。
/// 没有这把锁，两轮会在「`repo::exists` 查过了、`insert_running` 还没写」的窗口里
/// 撞车：`job_id UNIQUE` 保证不会重复花钱，但输的那一轮会把 UNIQUE 冲突当成收录失败
/// 抛给用户——一份其实跑得好好的工单显示「确认失败」，而本轮剩下的工单被整个放弃。
///
/// 这个窗口会随着收录变快而**变宽**（同样的 2 秒防抖，能覆盖的工作变多了），
/// 所以它不是「以后再说」的问题。
pub async fn scan(ctx: &Ctx, root: &Path) -> AppResult<ScanResult> {
    let _guard = ctx.scan_lock.lock().await;
    scan_locked(ctx, root).await
}

/// 重试一份失败/中断的工单。
///
/// 查台账、删旧行和重新扫描必须与普通扫描共用**同一段临界区**。否则快速点两次时，
/// 第二次会删掉第一次扫描刚插入的 `running` 行；第一次收录其实已经建批并移档，最后
/// 回读台账却只得到 `RowNotFound`，界面便显示一条与事实相反的数据库错误。
///
/// 旧行不存在或已经变成 `done` 都按“另一轮已经处理”成功返回，使本操作具备幂等性。
pub async fn retry_job(ctx: &Ctx, root: &Path, row_id: i64) -> AppResult<ScanResult> {
    let _guard = ctx.scan_lock.lock().await;
    match repo::get_optional(&ctx.pool, row_id).await? {
        Some(job) if job.status == "done" => return Ok(Vec::new()),
        Some(_) => {
            repo::delete(&ctx.pool, row_id).await?;
        }
        None => {
            // 上一次调用可能已删掉台账、却在开始扫描前被中断。继续扫目录既能恢复它，
            // 若工单已经被处理并移档也只会得到空结果。
        }
    }
    scan_locked(ctx, root).await
}

/// 已持有 `scan_lock` 的扫描主体。只允许 [`scan`] 与 [`retry_job`] 调用。
async fn scan_locked(ctx: &Ctx, root: &Path) -> AppResult<ScanResult> {
    let pending = super::pending_dir(root);
    // 目录不存在就先建出来：用户第一次看设置页时应该能直接「打开目录」把工单丢进去。
    std::fs::create_dir_all(&pending)?;

    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(&pending) else {
        return Ok(out);
    };
    // 目录顺序由文件系统决定，排一下让「先投的先跑」在同一轮里也成立。
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();

    for dir in dirs {
        let Some(name) = dir.file_name().map(|s| s.to_string_lossy().to_string()) else {
            continue;
        };
        // `_` 开头是我们自己的簿记目录（_已收录）；没有 READY.txt 说明 skill 还在写。
        if name.starts_with('_') || !dir.join(super::READY).is_file() {
            continue;
        }
        if let Some(view) = ingest_dir(ctx, root, &dir, &name).await? {
            out.push(view);
        }
    }
    Ok(out)
}

/// 收录单个工单目录。已处理过返回 `None`（不是错误——那是稳态）。
async fn ingest_dir(
    ctx: &Ctx,
    root: &Path,
    dir: &Path,
    dir_name: &str,
) -> AppResult<Option<JobView>> {
    // 1. 校验（只读）。此刻库里还一个字节都没变，失败即整份不发生。
    //    读不动/解析不了也要留痕：一份没人知道为什么没跑的工单，比一份报错的工单
    //    糟糕得多（v0.16.0 那条「操作信号全进了 tracing」的教训）。
    let plan = match super::plan(dir, dir_name) {
        Ok(p) => p,
        Err(msg) => return record_failure(ctx, dir, dir_name, dir_name, msg).await,
    };

    // 2. 去重：任何状态的行都挡住重投（见 0023 表注释）。
    //
    //    挡住之后必须留痕。目录还在收件里、READY.txt 也在，说明有人正等着它跑；
    //    静默返回会让这份工单永远停在「投了没反应」——没有回执、没有台账行、
    //    设置页点「立即扫描」也什么都不显示。这正是 jobId 默认取目录名的代价：
    //    同一天同一主题投第二次（`20260729-卡套-broll`）必然撞键。
    //
    //    只在**一个回执都没有**时写：已成功但移档失败的（有 结果.txt）、
    //    已记 error / hold 的（有 错误.txt / 待确认.txt）都是稳态，不该被反复打扰。
    if repo::exists(&ctx.pool, &plan.job_id).await? {
        if !has_receipt(dir) {
            write_receipt(dir, super::DUPLICATE_FILE, &duplicate_text(&plan.job_id));
            tracing::warn!(job_id = %plan.job_id, "工单名与台账重复，未收录");
        }
        return Ok(None);
    }

    // 3. 阈值。**确认.txt 是确认的唯一表达**——设置页那个按钮做的事就是替你写下它，
    //    所以两条确认路径走的是同一段代码，不可能一条对一条错。
    let confirmed = dir.join(super::CONFIRM_FILE).is_file();
    let total = plan.task_count();
    if !confirmed && ctx.threshold > 0 && total > ctx.threshold {
        return record_hold(ctx, &plan, total, ctx.threshold).await;
    }

    // 4. 记账在动手之前。
    let row_id = repo::insert_running(&ctx.pool, &plan.job_id, &plan.dir_name).await?;

    // 5. 干活。进度直接写进 `done`，失败时它就是「做到哪了」的如实交代。
    let mut done = Applied::default();
    match apply(ctx, &plan, &mut done).await {
        Ok(()) => {
            repo::mark_done(&ctx.pool, row_id, &done).await?;
            write_receipt(&plan.dir, super::RESULT_FILE, &result_text(&plan, &done));
            // 移档留证（同 v0.9.0 / v2v）：成功的工单从收件目录消失，人一眼看得出还剩什么。
            // 移不动就留着——DB 那行已经挡住重投了，最坏只是目录里多一份留档。
            move_to_consumed(root, &plan.dir, &plan.dir_name);
            ctx.engine.kick();
            Ok(Some(repo::get(&ctx.pool, row_id).await?))
        }
        Err(e) => {
            let msg = e.to_string();
            repo::mark_error(&ctx.pool, row_id, &msg, &done).await?;
            write_receipt(&plan.dir, super::ERROR_FILE, &error_text(&msg, &done));
            Ok(Some(repo::get(&ctx.pool, row_id).await?))
        }
    }
}

/// 校验阶段就失败的工单：记一行 error（同样占住去重键，不会每轮重报）+ 写 错误.txt。
async fn record_failure(
    ctx: &Ctx,
    dir: &Path,
    dir_name: &str,
    job_id: &str,
    msg: String,
) -> AppResult<Option<JobView>> {
    if repo::exists(&ctx.pool, job_id).await? {
        return Ok(None);
    }
    let row_id = repo::insert_running(&ctx.pool, job_id, dir_name).await?;
    // 校验阶段就失败 —— 库里确实一个字节都没变，这里的「没有导入任何东西」是真话。
    let nothing = Applied::default();
    repo::mark_error(&ctx.pool, row_id, &msg, &nothing).await?;
    write_receipt(dir, super::ERROR_FILE, &error_text(&msg, &nothing));
    Ok(Some(repo::get(&ctx.pool, row_id).await?))
}

/// 超阈值：**什么都不导入**，记一行 hold + 写 待确认.txt。
async fn record_hold(
    ctx: &Ctx,
    plan: &Plan,
    total: i64,
    threshold: i64,
) -> AppResult<Option<JobView>> {
    let msg = format!(
        "共 {total} 张图（{} 组 · {} 批次），超过阈值 {threshold}，等待确认",
        plan.groups.len(),
        plan.batch_count()
    );
    let row_id = repo::insert_running(&ctx.pool, &plan.job_id, &plan.dir_name).await?;
    repo::mark_hold(&ctx.pool, row_id, &msg, total, plan.groups.len() as i64).await?;
    write_receipt(
        &plan.dir,
        super::HOLD_FILE,
        &hold_text(plan, total, threshold),
    );
    Ok(Some(repo::get(&ctx.pool, row_id).await?))
}

/// 一次收录的产出（写台账用）。
#[derive(Debug, Default)]
pub struct Applied {
    pub batch_ids: Vec<i64>,
    pub params_json: Vec<String>,
    pub wire_json: Vec<String>,
    pub task_count: i64,
    pub group_count: i64,
    pub ref_count: i64,
}

/// 导入提示词 → 导入参考图 → 按参数分桶建批开跑。
///
/// **进度写进 `out` 而不是只在成功时返回**：这一串动作里没有一步是能整体回滚的
/// （参考图要拷文件建缩略图，建批要发编号，第一个批次建完就已经在跑了）。中途失败时
/// 「已经导入了多少」必须留下痕迹，否则台账那行只剩一句错误原文，而回执还在说
/// 「没有导入任何东西」—— 人照着那句话重投，就会得到第二份提示词和第二个批次。
async fn apply(ctx: &Ctx, plan: &Plan, out: &mut Applied) -> AppResult<()> {
    // ── 提示词：全部组合成一份预览，一个事务落库（同手动导入的写路径）。
    let parsed: Vec<ParsedGroup> = plan.groups.iter().map(|g| g.parsed.clone()).collect();
    let preview = build_preview_from_parsed(&ctx.pool, &parsed, "UTF-8".into(), Vec::new()).await?;
    let result = commit_preview(&ctx.pool, &preview, "library").await?;
    out.group_count = result.group_ids.len() as i64;
    if result.group_ids.len() != plan.groups.len() {
        // 理论上不会发生（空组已在校验里排除）；真发生了宁可整单失败也不要错配挂靠。
        return Err(AppError::Internal(
            "分组落库数与工单不一致，已中止（未建批次）".into(),
        ));
    }

    // ── 参考图：可选的图库分组（重名复用，不报错）。
    let ref_group_id = match &plan.ref_group {
        Some(name) => Some(
            match refs_repo::find_group_by_name(&ctx.pool, name).await? {
                Some(g) => g.id,
                None => refs_repo::create_group(&ctx.pool, name).await?.id,
            },
        ),
        None => None,
    };
    ctx.dirs.init()?;
    let refs_dir = ctx.dirs.refs();
    let thumbs_dir = ctx.dirs.thumbs();

    // ── 按 (参数快照, 抽卡) 分桶。参数是**批次级**的，各组比例不同就必须拆批次。
    let mut buckets: Vec<Bucket> = Vec::new();
    let total_refs: i64 = plan.groups.iter().map(|g| g.refs.len() as i64).sum();

    for (g, group_id) in plan.groups.iter().zip(result.group_ids.iter().copied()) {
        let key = g.bucket();
        let slot = match buckets
            .iter()
            .position(|b| (b.params.clone(), b.draws) == key)
        {
            Some(i) => i,
            None => {
                buckets.push(Bucket {
                    params: g.params_json.clone(),
                    wire: g.wire_json.clone(),
                    draws: g.draws,
                    mappings: Vec::new(),
                });
                buckets.len() - 1
            }
        };
        // 分块并行落盘，块内并发、块间串行、**插库严格按原顺序**。
        //
        // 为什么不干脆「全部解完再全部插库」：失败回执是承重的（谎称「没有导入任何
        // 东西」会让人点重试拿到第二份提示词），两段式会在解码阶段失败时留下一整批
        // 谁也不知道存在的孤儿文件，而台账上 ref_count 还是 0。分块把孤儿窗口压到
        // 一个块以内，且 ref_count 是可见地往前走的。
        //
        // 顺序必须保持：mappings 的顺序会喂给 create_batch 决定任务创建顺序。
        // `join_all` 按输入顺序返回，故这里天然保序。
        for chunk in g.refs.chunks(crate::files::decode::max_concurrent()) {
            let jobs = chunk.iter().map(|path| {
                let p = path.to_string_lossy().to_string();
                let (rd, td) = (refs_dir.clone(), thumbs_dir.clone());
                // 解码 + 缩放 + 重编码是纯 CPU，留在异步执行器上会把整个 IPC 卡住
                // （v0.14.0）；`bounded` 同时把它纳入进程级解码预算。
                crate::files::decode::bounded(move |permit| ingest_one(&p, &rd, &td, permit))
            });
            for ing in futures_util::future::join_all(jobs).await {
                let ing = ing?;
                let id = refs_repo::insert(
                    &ctx.pool,
                    &refs_repo::NewRefImage {
                        name: ing.name,
                        ref_group_id,
                        file_path: ing.file_path,
                        thumb_path: ing.thumb_path,
                        width: ing.width,
                        height: ing.height,
                        file_size: ing.file_size,
                        content_hash: ing.content_hash,
                        upload_path: ing.upload_path,
                        ephemeral: plan.ephemeral,
                    },
                )
                .await?;
                out.ref_count += 1;
                ctx.progress.refs(&plan.job_id, out.ref_count, total_refs);
                buckets[slot].mappings.push(RefMapping {
                    ref_image_id: id,
                    prompt_group_id: group_id,
                });
            }
        }
    }

    // ── 建批（花钱的那一步，放在最后）。一桶一个批次。
    let output_dir = ctx.dirs.outputs().to_string_lossy().to_string();
    let multi = buckets.len() > 1;
    for (i, b) in buckets.iter().enumerate() {
        let (batch_id, task_count) =
            engine::create_batch(&ctx.pool, &output_dir, &b.params, &b.mappings, b.draws).await?;
        // 批次备注：拆批时带上序号，否则任务页会出现几张认不出彼此关系的卡片。
        if let Some(note) = &plan.note {
            let note = if multi {
                format!("{note}（{}/{}）", i + 1, buckets.len())
            } else {
                note.clone()
            };
            // 备注失败不该让一个已经跑起来的批次算作「工单失败」。
            let _ = task_repo::rename_batch(&ctx.pool, batch_id, &note).await;
        }
        out.batch_ids.push(batch_id);
        out.params_json.push(b.params.clone());
        out.wire_json.push(b.wire.clone());
        out.task_count += task_count;
    }
    Ok(())
}

/// 一个批次桶：参数相同的组共用它。
struct Bucket {
    params: String,
    wire: String,
    draws: i64,
    mappings: Vec<RefMapping>,
}

/// 成功后移档到 `_已收录/<目录名>-<ts>`。失败只记日志：DB 那行已经挡住重投了。
fn move_to_consumed(root: &Path, dir: &Path, dir_name: &str) {
    let consumed = super::consumed_dir(root);
    if std::fs::create_dir_all(&consumed).is_err() {
        return;
    }
    let dest = consumed.join(format!("{dir_name}-{}", now_unix()));
    if let Err(e) = std::fs::rename(dir, &dest) {
        tracing::warn!(error = %e, dir = %dir.display(), "工单移档失败，目录留在原处");
    }
}

/// 回执文件：让投单那一侧（Claude Code / Codex）能回读结果，不必猜后面发生了什么。
///
/// 失败与待确认的目录**不移走**：回执要和提示词文档摆在一起才有用——改完就地重投。
fn write_receipt(dir: &Path, name: &str, body: &str) {
    if let Err(e) = std::fs::write(dir.join(name), body) {
        tracing::warn!(error = %e, file = %name, "写工单回执失败");
    }
}

/// 这份工单是否已经有过交代。用来区分「稳态的重复扫描」与「静默吞掉的重名工单」。
fn has_receipt(dir: &Path) -> bool {
    [
        super::RESULT_FILE,
        super::HOLD_FILE,
        super::ERROR_FILE,
        super::DUPLICATE_FILE,
    ]
    .iter()
    .any(|f| dir.join(f).is_file())
}

fn duplicate_text(job_id: &str) -> String {
    format!(
        "这份工单**没有收录**：工单名 `{job_id}` 与台账里已有的工单重名。\n\n\
         jobId 默认取工单目录名，而它必须全局唯一——同一天、同一主题投第二次\n\
         （比如两次都叫 `20260729-卡套-broll`）就会撞上。去重是有意的：\n\
         它挡住的是「同一份工单被重放两次」，那会造出重复的提示词、重复的批次、\n\
         重复的钱。但你这一份很可能是**新的内容套了个旧名字**。\n\n\
         怎么办（二选一）：\n\
         1. 想跑：把工单目录改个没用过的名字（末尾加时间戳最省事），\n\
            删掉本文件后 READY.txt 保持在，下一轮扫描就会收录；\n\
         2. 确认是重复投递、本来就不该跑：直接删掉整个工单目录。\n\n\
         想查是哪一份占了这个名字：GenDesk 设置页「Claude Code 收件」的台账里搜这个名字。\n"
    )
}

fn result_text(plan: &Plan, done: &Applied) -> String {
    let mut s = format!(
        "GenDesk 已收录并开跑。\n\n\
         工单：{}\n组数：{}\n参考图：{}\n任务（= 出图张数）：{}\n批次：{}\n\n",
        plan.job_id,
        done.group_count,
        done.ref_count,
        done.task_count,
        done.batch_ids
            .iter()
            .map(|b| format!("#{b}"))
            .collect::<Vec<_>>()
            .join(" "),
    );
    for (i, b) in done.batch_ids.iter().enumerate() {
        s.push_str(&format!(
            "批次 #{b} 实际发往接口的字段：{}\n",
            done.wire_json.get(i).map(String::as_str).unwrap_or("{}")
        ));
    }
    s.push_str("\n验收请在 GenDesk 的验收页完成。\n");
    s
}

fn hold_text(plan: &Plan, total: i64, threshold: i64) -> String {
    format!(
        "这份工单超过了自动开跑阈值，**还没有导入任何东西**（提示词、参考图一条都没进库）。\n\n\
         共 {total} 张图 = {} 组 × 各自的条数 × 挂靠图数 × 抽卡\n\
         将建 {} 个批次（各组比例/抽卡不同就会拆批）\n\
         当前阈值：{threshold}\n\n\
         确认开跑，二选一：\n\
         1. 在这个目录里建一个空文件 `{}`\n\
         2. 或在 GenDesk 设置页「Claude Code 收件」点「确认开跑」（它做的就是第 1 件事）\n\n\
         不想跑就直接删掉整个工单目录。\n",
        plan.groups.len(),
        plan.batch_count(),
        super::CONFIRM_FILE,
    )
}

/// 失败回执。**如实交代已经导入了多少** —— 这不是措辞问题，是重投安全的前提。
///
/// 收录这一串动作没有一步能整体回滚：参考图要拷文件建缩略图，建批要发编号，
/// 第一个批次建完那一刻就已经在花钱跑了。原来这里无条件写「没有导入任何东西」，
/// 而人照着它去点「重试」，得到的是第二份提示词和第二个批次 —— 花两份钱。
fn error_text(msg: &str, done: &Applied) -> String {
    let mut s = format!("GenDesk 收录这份工单时失败了：\n\n{msg}\n\n");
    if done.group_count == 0 && done.ref_count == 0 && done.batch_ids.is_empty() {
        s.push_str("库里**没有导入任何东西**，改好之后直接重投即可。\n\n");
    } else {
        s.push_str(&format!(
            "但**已经导入了一部分**，重投前必须先处理它们，否则会得到第二份：\n\
             · 提示词组：{} 组\n· 参考图：{} 张\n",
            done.group_count, done.ref_count
        ));
        if done.batch_ids.is_empty() {
            s.push_str("· 批次：未建（这一单还没开始花钱）\n\n");
        } else {
            s.push_str(&format!(
                "· 批次：{}（**已经在跑，正在花钱**）—— 不想要就去任务页中止并删除\n\n",
                done.batch_ids
                    .iter()
                    .map(|b| format!("#{b}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            ));
        }
    }
    s.push_str("处理完之后，在 GenDesk 设置页「Claude Code 收件」里点「重试」。\n");
    s
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即失败
mod tests {
    use super::*;
    use crate::db::test_support::test_pool;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// 只数被叫了几次：建批之后必须唤醒调度器，否则任务会一直躺在 `q`
    /// （v0.11.1 那个「设置页填 50、实际只跑 10」的坑同一类：装配漏一处，界面全对）。
    #[derive(Default)]
    struct CountingKick(AtomicUsize);
    impl Kick for CountingKick {
        fn kick(&self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// 最小合法 PNG（1×1 白点）：缩略图那一步要真的能解码。
    fn png() -> Vec<u8> {
        use std::io::Cursor;
        let img = image::RgbImage::from_pixel(1, 1, image::Rgb([255, 255, 255]));
        let mut buf = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        buf.into_inner()
    }

    /// 造一份工单目录（标准形态：提示词.txt + images/ + READY.txt）。
    fn make_job(root: &Path, dir_name: &str, doc: &str, images: &[&str]) -> PathBuf {
        let dir = super::super::pending_dir(root).join(dir_name);
        std::fs::create_dir_all(dir.join(super::super::IMAGES)).unwrap();
        std::fs::write(dir.join(super::super::DOC), doc).unwrap();
        for n in images {
            std::fs::write(dir.join(super::super::IMAGES).join(n), png()).unwrap();
        }
        std::fs::write(dir.join(super::super::READY), "ok").unwrap();
        dir
    }

    /// 记录进度回报，供「进度必须单调且到达 total」那条测试用。
    #[derive(Default)]
    struct RecordingProgress(std::sync::Mutex<Vec<(i64, i64)>>);
    impl ProgressSink for RecordingProgress {
        fn refs(&self, _job_id: &str, done: i64, total: i64) {
            if let Ok(mut v) = self.0.lock() {
                v.push((done, total));
            }
        }
    }

    fn ctx_for(pool: &SqlitePool, data: &Path, threshold: i64) -> (Ctx, Arc<CountingKick>) {
        let (ctx, kick, _) = ctx_for_full(pool, data, threshold);
        (ctx, kick)
    }

    fn ctx_for_full(
        pool: &SqlitePool,
        data: &Path,
        threshold: i64,
    ) -> (Ctx, Arc<CountingKick>, Arc<RecordingProgress>) {
        let dirs = Arc::new(DataDirs::new(data));
        dirs.init().unwrap();
        let kick = Arc::new(CountingKick::default());
        let progress = Arc::new(RecordingProgress::default());
        (
            Ctx {
                pool: pool.clone(),
                dirs,
                engine: kick.clone(),
                progress: progress.clone(),
                threshold,
                scan_lock: Arc::new(tokio::sync::Mutex::new(())),
            },
            kick,
            progress,
        )
    }

    async fn count(pool: &SqlitePool, table: &str) -> i64 {
        sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(pool)
            .await
            .unwrap()
    }

    // 端到端：工单落地 → 提示词进库、参考图进库、批次建成、任务展开、调度器被唤醒。
    // 这条链路上任何一环断掉，用户看到的都是「投了单什么也没发生」。
    #[tokio::test]
    async fn job_becomes_a_running_batch() {
        let (pool, _d) = test_pool().await;
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let (ctx, kick) = ctx_for(&pool, data.path(), 500);

        make_job(
            home.path(),
            "20260727-卡套-买家秀",
            "分组: 黄色系卡套\n前缀: KT\n参考图: images/黄.png\n比例: 9:16\n抽卡: 2\n用途: 图生视频\n\n\
             把卡套放进照片里，自然光\n\n侧逆光，浅景深\n",
            &["黄.png"],
        );

        let jobs = scan(&ctx, home.path()).await.unwrap();
        assert_eq!(jobs.len(), 1);
        let job = &jobs[0];
        assert_eq!(job.status, "done", "收录应成功：{}", job.message);
        assert_eq!(job.group_count, 1);
        assert_eq!(job.ref_count, 1);
        assert_eq!(job.task_count, 4, "2 条 × 1 图 × 抽卡 2");
        assert_eq!(job.batch_ids.len(), 1);
        assert_eq!(kick.0.load(Ordering::SeqCst), 1, "建批后必须唤醒调度器");

        // 用户的核心关切：组头写的比例真的进了这个批次的参数快照。
        let params: String = sqlx::query_scalar("SELECT params_json FROM batches WHERE id = ?1")
            .bind(job.batch_ids[0])
            .fetch_one(&pool)
            .await
            .unwrap();
        let p = crate::provider::GenParams::from_json(&params);
        assert_eq!(p.aspect_ratio.as_deref(), Some("9:16"));

        // 编号前缀与用途标签都该落到位。
        let codes: Vec<String> = sqlx::query_scalar("SELECT code FROM prompts ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(codes, vec!["KT-0001", "KT-0002"]);
        let tags: Vec<String> = sqlx::query_scalar(
            "SELECT t.name FROM tags t JOIN tag_bindings b ON b.tag_id = t.id
              WHERE b.entity_type = 'prompt_group'",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert!(tags.contains(&crate::purpose::PURPOSE_I2V.to_string()));

        // 成功后移档 + 回执随目录一起走。
        assert!(!super::super::pending_dir(home.path())
            .join("20260727-卡套-买家秀")
            .exists());
        let moved: Vec<PathBuf> = std::fs::read_dir(super::super::consumed_dir(home.path()))
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .collect();
        assert_eq!(moved.len(), 1);
        let receipt = std::fs::read_to_string(moved[0].join(super::super::RESULT_FILE)).unwrap();
        assert!(receipt.contains("aspect_ratio"), "回执要写清实际发出去什么");
    }

    /// 分块并行落盘之后，**参考图的入库顺序必须仍然是工单里写的顺序**。
    ///
    /// mappings 的顺序会喂给 `create_batch` 决定任务创建顺序；并行化最容易在这里
    /// 静默出错——图都进去了、数量也对，只是配对错位，而那要到验收时才看得出来。
    /// 同时钉住进度回报：单调、且最终等于总数。
    #[tokio::test]
    async fn parallel_ref_ingest_preserves_order_and_reports_progress() {
        let (pool, _d) = test_pool().await;
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let (ctx, _kick, progress) = ctx_for_full(&pool, data.path(), 500);

        // 七张（> 并发块大小，保证真的跨了多个块），名字本身带顺序。
        let names: Vec<String> = (1..=7).map(|i| format!("r{i}.png")).collect();
        let refs_line = names
            .iter()
            .map(|n| format!("images/{n}"))
            .collect::<Vec<_>>()
            .join(", ");
        let name_refs: Vec<&str> = names.iter().map(String::as_str).collect();
        make_job(
            home.path(),
            "20260727-顺序",
            &format!("分组: 顺序组\n参考图: {refs_line}\n\n提示词一\n"),
            &name_refs,
        );

        let jobs = scan(&ctx, home.path()).await.unwrap();
        assert_eq!(jobs[0].status, "done", "收录应成功：{}", jobs[0].message);
        assert_eq!(jobs[0].ref_count, 7);

        // 入库顺序 = 工单顺序。ref_images.id 递增，故按 id 排出来就是入库顺序。
        let got: Vec<String> = sqlx::query_scalar("SELECT name FROM ref_images ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
        let want: Vec<String> = (1..=7).map(|i| format!("r{i}")).collect();
        assert_eq!(got, want, "并行落盘不得打乱参考图顺序");

        let seen = progress.0.lock().unwrap().clone();
        assert!(!seen.is_empty(), "必须有进度回报");
        assert!(
            seen.windows(2).all(|w| w[0].0 < w[1].0),
            "进度必须单调递增：{seen:?}"
        );
        assert_eq!(seen.last(), Some(&(7, 7)), "最后一条应是 7/7");
    }

    /// 同一个收件目录被并发扫两轮 —— 设置页点「确认开跑」会直接扫一轮，而写下的
    /// `确认.txt` 落在被监听目录里，watcher 也会扫一轮。结果必须是**恰好一个批次**，
    /// 且**两轮都不报错**：输的那一轮该安静地跳过，而不是把 UNIQUE 冲突当成
    /// 「收录失败」抛给用户（一份其实跑得好好的工单显示确认失败）。
    #[tokio::test]
    async fn concurrent_scans_ingest_exactly_once_without_error() {
        let (pool, _d) = test_pool().await;
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let (ctx, kick) = ctx_for(&pool, data.path(), 500);

        make_job(
            home.path(),
            "20260727-并发",
            "分组: 并发组\n参考图: images/a.png\n\n提示词一\n",
            &["a.png"],
        );

        let (c1, c2) = (ctx.clone(), ctx.clone());
        let (h1, h2) = (home.path().to_path_buf(), home.path().to_path_buf());
        let (r1, r2) = tokio::join!(
            tokio::spawn(async move { scan(&c1, &h1).await }),
            tokio::spawn(async move { scan(&c2, &h2).await }),
        );
        let a = r1.unwrap().expect("第一轮扫描不该报错");
        let b = r2.unwrap().expect("第二轮扫描不该报错");

        // 一轮收录了它，另一轮什么都没做。
        assert_eq!(a.len() + b.len(), 1, "工单只该被收录一次");
        assert_eq!(count(&pool, "batches").await, 1, "只该建出一个批次");
        assert_eq!(count(&pool, "intake_jobs").await, 1);
        assert_eq!(kick.0.load(Ordering::SeqCst), 1, "只该唤醒一次调度器");
    }

    /// 同一行快速点两次「重试」：第一轮删掉旧行后，第二轮会带着已经失效的 id 到达。
    /// 它不能抛出 RowNotFound，也不能再触发一份收录；最终的新台账行必须保留下来。
    #[tokio::test]
    async fn concurrent_retries_are_idempotent_and_keep_the_new_ledger_row() {
        let (pool, _d) = test_pool().await;
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let (ctx, kick) = ctx_for(&pool, data.path(), 500);

        make_job(
            home.path(),
            "job-retry",
            "分组: 重试组\n参考图: images/a.png\n\n提示词一\n",
            &["a.png"],
        );
        let old_id = repo::insert_running(&pool, "job-retry", "job-retry")
            .await
            .unwrap();

        let (c1, c2) = (ctx.clone(), ctx.clone());
        let (h1, h2) = (home.path().to_path_buf(), home.path().to_path_buf());
        let (r1, r2) = tokio::join!(
            tokio::spawn(async move { retry_job(&c1, &h1, old_id).await }),
            tokio::spawn(async move { retry_job(&c2, &h2, old_id).await }),
        );
        let a = r1.unwrap().expect("第一次重试不该报错");
        let b = r2.unwrap().expect("重复重试应当幂等成功");

        assert_eq!(a.len() + b.len(), 1, "工单只该被重新收录一次");
        assert_eq!(count(&pool, "batches").await, 1, "只该建出一个批次");
        assert_eq!(count(&pool, "intake_jobs").await, 1, "新台账行必须保留");
        let final_job = repo::list_recent(&pool, 1)
            .await
            .unwrap()
            .pop()
            .expect("新台账行仍应存在");
        assert_eq!(final_job.status, "done");
        assert_eq!(kick.0.load(Ordering::SeqCst), 1, "只该唤醒一次调度器");
    }

    /// 页面可能还显示刷新前的旧 id；这种请求应当安静恢复扫描，而不是把 SQLx 的
    /// RowNotFound 原文抛给用户。
    #[tokio::test]
    async fn retrying_a_missing_ledger_row_is_a_noop() {
        let (pool, _d) = test_pool().await;
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let (ctx, _) = ctx_for(&pool, data.path(), 500);

        let jobs = retry_job(&ctx, home.path(), 404).await.unwrap();
        assert!(jobs.is_empty());
        assert_eq!(count(&pool, "intake_jobs").await, 0);
    }

    // 各组比例不同 → 自动拆成多个批次（params_json 是批次级的，塞不进一个批次）。
    #[tokio::test]
    async fn different_ratios_split_into_separate_batches() {
        let (pool, _d) = test_pool().await;
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let (ctx, _) = ctx_for(&pool, data.path(), 500);

        make_job(
            home.path(),
            "job-split",
            "分组: 黄\n参考图: images/黄.png\n比例: 3:4\n\n黄1\n\n黄2\n\n\
             分组: 蓝\n参考图: images/蓝.png\n比例: 9:16\n\n蓝1\n",
            &["黄.png", "蓝.png"],
        );

        let jobs = scan(&ctx, home.path()).await.unwrap();
        assert_eq!(jobs[0].status, "done", "{}", jobs[0].message);
        assert_eq!(jobs[0].batch_ids.len(), 2, "两种比例必须是两个批次");
        assert_eq!(jobs[0].task_count, 3, "黄 2 条 + 蓝 1 条，各 1 图 1 抽卡");

        // 每个批次拿到的是自己那一份比例，没有互相串味。
        let mut ratios = Vec::new();
        for b in &jobs[0].batch_ids {
            let p: String = sqlx::query_scalar("SELECT params_json FROM batches WHERE id = ?1")
                .bind(b)
                .fetch_one(&pool)
                .await
                .unwrap();
            ratios.push(
                crate::provider::GenParams::from_json(&p)
                    .aspect_ratio
                    .unwrap_or_default(),
            );
        }
        ratios.sort();
        assert_eq!(ratios, vec!["3:4", "9:16"]);
    }

    // 参数一样的多个组并进同一个批次，不该无谓拆开。
    #[tokio::test]
    async fn same_params_share_one_batch() {
        let (pool, _d) = test_pool().await;
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let (ctx, _) = ctx_for(&pool, data.path(), 500);
        make_job(
            home.path(),
            "job-merge",
            "分组: 甲\n参考图: images/a.png\n比例: 3:4\n\n甲1\n\n\
             分组: 乙\n参考图: images/b.png\n比例: 3:4\n\n乙1\n",
            &["a.png", "b.png"],
        );
        let jobs = scan(&ctx, home.path()).await.unwrap();
        assert_eq!(jobs[0].batch_ids.len(), 1);
        assert_eq!(jobs[0].group_count, 2);
    }

    // 重复扫描不得重复建批 —— 这是整个模块最贵的一条错误（重复花钱）。
    #[tokio::test]
    async fn rescan_does_not_create_a_second_batch() {
        let (pool, _d) = test_pool().await;
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let (ctx, _) = ctx_for(&pool, data.path(), 500);
        let doc = "分组: 甲\n参考图: images/a.png\n\n甲1\n";
        make_job(home.path(), "job-1", doc, &["a.png"]);

        assert_eq!(scan(&ctx, home.path()).await.unwrap().len(), 1);
        // 同一份工单原样再投一次（同目录名 = 同 jobId）：目录在，但台账已记，必须跳过。
        make_job(home.path(), "job-1", doc, &["a.png"]);
        assert!(scan(&ctx, home.path()).await.unwrap().is_empty());
        assert_eq!(count(&pool, "batches").await, 1, "同一工单只能建一个批次");
    }

    // 撞名被挡住可以，**不吭声不行**：目录还在、READY.txt 还在，就得有一份交代，
    // 否则就是「投了没反应、扫描也没反应、日志里也找不着」。
    #[tokio::test]
    async fn duplicate_job_name_leaves_a_receipt() {
        let (pool, _d) = test_pool().await;
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let (ctx, _) = ctx_for(&pool, data.path(), 500);
        let doc = "分组: 甲\n参考图: images/a.png\n\n甲1\n";
        make_job(home.path(), "job-dup", doc, &["a.png"]);
        assert_eq!(scan(&ctx, home.path()).await.unwrap().len(), 1);

        // 新内容套了个用过的名字。
        let dir = make_job(
            home.path(),
            "job-dup",
            "分组: 乙\n参考图: images/a.png\n\n乙1\n",
            &["a.png"],
        );
        assert!(scan(&ctx, home.path()).await.unwrap().is_empty());
        let note = dir.join(super::super::DUPLICATE_FILE);
        assert!(note.is_file(), "撞名必须留下回执");
        assert!(std::fs::read_to_string(&note).unwrap().contains("job-dup"));
        assert_eq!(count(&pool, "batches").await, 1, "撞名不得建第二个批次");
    }

    // 成功了但移档失败的工单会带着 结果.txt 留在收件里 —— 那是稳态，
    // 每轮扫描都往里塞一份「重名」回执只会把人吓一跳。
    #[tokio::test]
    async fn duplicate_receipt_not_written_over_existing_result() {
        let (pool, _d) = test_pool().await;
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let (ctx, _) = ctx_for(&pool, data.path(), 500);
        let doc = "分组: 甲\n参考图: images/a.png\n\n甲1\n";
        make_job(home.path(), "job-kept", doc, &["a.png"]);
        assert_eq!(scan(&ctx, home.path()).await.unwrap().len(), 1);

        let dir = make_job(home.path(), "job-kept", doc, &["a.png"]);
        std::fs::write(dir.join(super::super::RESULT_FILE), "已收录并开跑").unwrap();
        assert!(scan(&ctx, home.path()).await.unwrap().is_empty());
        assert!(
            !dir.join(super::super::DUPLICATE_FILE).exists(),
            "已有回执的工单不该再被打扰"
        );
    }

    // 没有 READY.txt = skill 还在写，一律不碰（半份工单建出来的批次是错的）。
    #[tokio::test]
    async fn job_without_ready_is_skipped() {
        let (pool, _d) = test_pool().await;
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let (ctx, _) = ctx_for(&pool, data.path(), 500);
        let dir = make_job(
            home.path(),
            "job-2",
            "分组: 甲\n参考图: images/a.png\n\n甲1\n",
            &["a.png"],
        );
        std::fs::remove_file(dir.join(super::super::READY)).unwrap();

        assert!(scan(&ctx, home.path()).await.unwrap().is_empty());
        assert!(dir.exists(), "未就绪的工单不该被移走");
    }

    // 参数非法 = 整份工单不发生：一条提示词、一张图、一个批次都不该进库。
    #[tokio::test]
    async fn invalid_params_leave_nothing_behind() {
        let (pool, _d) = test_pool().await;
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let (ctx, kick) = ctx_for(&pool, data.path(), 500);
        let dir = make_job(
            home.path(),
            "job-3",
            "分组: 甲\n参考图: images/a.png\n尺寸: 1080x1920\n\n甲1\n",
            &["a.png"],
        );

        let jobs = scan(&ctx, home.path()).await.unwrap();
        assert_eq!(jobs[0].status, "error");
        assert!(jobs[0].message.contains("16 的倍数"), "{}", jobs[0].message);
        assert_eq!(kick.0.load(Ordering::SeqCst), 0);
        for t in ["batches", "prompts", "ref_images", "tasks"] {
            assert_eq!(count(&pool, t).await, 0, "{t} 不该有任何记录");
        }
        // 失败的工单留在原处 + 写下原因，改完就能重投。
        assert!(dir.join(super::super::ERROR_FILE).is_file());
        // 失败也占住去重键：下一轮不该自动重来（半份工单重放会造重复提示词）。
        assert!(scan(&ctx, home.path()).await.unwrap().is_empty());
    }

    // 超阈值：什么都不导入、不建批、不唤醒调度器，只留一份说明。
    #[tokio::test]
    async fn over_threshold_holds_without_importing_anything() {
        let (pool, _d) = test_pool().await;
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let (ctx, kick) = ctx_for(&pool, data.path(), 3);
        let dir = make_job(
            home.path(),
            "job-big",
            "分组: 甲\n参考图: images/a.png\n抽卡: 2\n\n甲1\n\n甲2\n\n甲3\n",
            &["a.png"],
        );

        let jobs = scan(&ctx, home.path()).await.unwrap();
        assert_eq!(jobs[0].status, "hold");
        assert_eq!(jobs[0].task_count, 6, "3 条 × 1 图 × 抽卡 2");
        assert!(jobs[0].message.contains("超过阈值"), "{}", jobs[0].message);
        assert_eq!(kick.0.load(Ordering::SeqCst), 0);
        for t in ["batches", "prompts", "ref_images", "tasks"] {
            assert_eq!(count(&pool, t).await, 0, "{t} 在待确认时必须是空的");
        }
        assert!(dir.join(super::super::HOLD_FILE).is_file());
        assert!(dir.exists(), "待确认的工单留在原处");
    }

    // 确认.txt 是确认的唯一表达：删掉台账那行 + 放上确认文件 → 下一轮照常开跑。
    #[tokio::test]
    async fn confirm_file_lets_a_held_job_run() {
        let (pool, _d) = test_pool().await;
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let (ctx, kick) = ctx_for(&pool, data.path(), 3);
        let dir = make_job(
            home.path(),
            "job-big",
            "分组: 甲\n参考图: images/a.png\n抽卡: 2\n\n甲1\n\n甲2\n\n甲3\n",
            &["a.png"],
        );
        let held = scan(&ctx, home.path()).await.unwrap();
        assert_eq!(held[0].status, "hold");

        // 「确认」= 删台账行 + 写确认文件（设置页那个按钮做的就是这两件事）。
        repo::delete(&pool, held[0].id).await.unwrap();
        std::fs::write(dir.join(super::super::CONFIRM_FILE), "ok").unwrap();

        let jobs = scan(&ctx, home.path()).await.unwrap();
        assert_eq!(jobs[0].status, "done", "{}", jobs[0].message);
        assert_eq!(jobs[0].task_count, 6);
        assert_eq!(kick.0.load(Ordering::SeqCst), 1);
    }

    // 阈值 <= 0 = 不限：多大的工单都直接跑。
    #[tokio::test]
    async fn zero_threshold_means_unlimited() {
        let (pool, _d) = test_pool().await;
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let (ctx, _) = ctx_for(&pool, data.path(), 0);
        make_job(
            home.path(),
            "job-unl",
            "分组: 甲\n参考图: images/a.png\n抽卡: 5\n\n甲1\n\n甲2\n",
            &["a.png"],
        );
        let jobs = scan(&ctx, home.path()).await.unwrap();
        assert_eq!(jobs[0].status, "done", "{}", jobs[0].message);
        assert_eq!(jobs[0].task_count, 10);
    }

    // 多组没点名挂靠 → 拒单，不猜。
    #[tokio::test]
    async fn multi_group_without_mapping_fails() {
        let (pool, _d) = test_pool().await;
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let (ctx, _) = ctx_for(&pool, data.path(), 500);
        make_job(
            home.path(),
            "job-4",
            "分组: 甲\n\n甲1\n\n分组: 乙\n参考图: images/b.png\n\n乙1\n",
            &["a.png", "b.png"],
        );
        let jobs = scan(&ctx, home.path()).await.unwrap();
        assert_eq!(jobs[0].status, "error");
        assert!(jobs[0].message.contains("必须点名"), "{}", jobs[0].message);
    }
}
