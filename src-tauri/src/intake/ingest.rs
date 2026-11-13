//! 工单收录：CLI 指名提交 → 校验 → 阈值 → 导入提示词与参考图 → 按参数分桶建批 → 移档留证。
//!
//! ## 顺序不是随便排的
//!
//! 校验（纯读）→ **算任务数**（纯算）→ 记账（intake_jobs 置 running）→ 导入 →
//! **建批**（花钱）→ 记账置 done → 写回执 → 移档。
//!
//! 阈值判定排在导入之前，所以超阈值时**库里一个字节都没变**——不存在「提示词进了库
//! 但没建批」这种要人去收拾的中间态。
//!
//! 记账在**动手之前**：进程若在导入中途被杀，那一行会留在 `running`，重复提交据此
//! 跳过它，而不是从头再来一遍——半个工单重放会造出重复的提示词与重复的批次。
//! 这与 v0.15.0「提交成功即写 submit_id」是同一条教训：不可撤回的动作，标记要先落。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use sqlx::SqlitePool;

use super::{JobView, Plan};
use crate::commands::prompts::{build_preview_from_parsed, commit_preview_in_tx};
use crate::commands::refs::ingest_one;
use crate::db::now_unix;
use crate::db::repo::intake as repo;
use crate::db::repo::refs as refs_repo;
use crate::db::repo::tasks as task_repo;
use crate::engine::{self, Engine};
use crate::error::{AppError, AppResult};
use crate::files::DataDirs;
use crate::importer::ParsedGroup;

/// 建批之后唤醒调度器。
///
/// 抽成 trait 只为一件事：让收录的端到端测试不必启动整个引擎（那要 provider 工厂、
/// 事件汇、密钥存储三样东西）。同 `engine::events::EventSink` 的理由——
/// 「这条链路到底把什么写进了库」是本模块唯一值得测的东西，不该被装配成本挡住。
/// 投单时的 Poster 归属检查。开启「自动导入 Poster」时，返回审稿通过后进不了 Poster
/// 的款式说明；空 = 放行。放在收录里而不是审稿后：对不上的图生成了也进不了 Poster，
/// 等到审稿才发现时钱已经花了。
#[async_trait::async_trait]
pub trait PosterGate: Send + Sync {
    async fn unresolved(&self, keys: &[(String, String)]) -> Vec<String>;
    /// 显式指定的 Poster 图片类型（`organization.posterType`）里对不上的说明；空 = 放行。
    async fn unresolved_types(&self, _labels: &[String]) -> Vec<String> {
        Vec::new()
    }
}

/// 投单时查出的 Poster 归属问题：款式对不上商品、显式类型对不上 Poster 图片类型。
#[derive(Debug, Default)]
struct PosterIssues {
    styles: Vec<String>,
    types: Vec<String>,
}

impl PosterIssues {
    fn is_empty(&self) -> bool {
        self.styles.is_empty() && self.types.is_empty()
    }
}

/// 不做检查（测试用）。
#[cfg(test)]
pub struct NoPosterGate;

#[cfg(test)]
#[async_trait::async_trait]
impl PosterGate for NoPosterGate {
    async fn unresolved(&self, _keys: &[(String, String)]) -> Vec<String> {
        Vec::new()
    }
}

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

/// 不回报。测试用。
pub struct NoProgress;

impl ProgressSink for NoProgress {
    fn refs(&self, _job_id: &str, _done: i64, _total: i64) {}
}

/// 收录所需的全部句柄。每次 CLI 调用各自构造一个，字段都是 clone 友好的。
#[derive(Clone)]
pub struct Ctx {
    pub pool: SqlitePool,
    pub dirs: Arc<DataDirs>,
    pub engine: Arc<dyn Kick>,
    pub progress: Arc<dyn ProgressSink>,
    /// 任务数超过它就不自动开跑，转「待确认」。`<= 0` = 不限。
    pub threshold: i64,
    /// Poster 款式对不上时同样转「待确认」。
    pub poster: Arc<dyn PosterGate>,
    /// 收录互斥锁。**必须全进程共用同一把**：CLI 的重复提交、确认与重试串行执行。
    pub lock: Arc<tokio::sync::Mutex<()>>,
}

/// 收录 CLI 指名的一份工单。全进程串行，同一工单无论重复调用多少次都只收录一次。
///
/// 没有目录监听，也没有全量扫描：工单只在 `gendesk submit` 明确交给应用时处理，
/// 其他目录（另一个 Agent 还在写的、早已移档的）一概不碰。
///
/// 同一目录被并发提交两次时，后到的一方发现目录已被收录移走，返回 `None`
/// （「没有新动作」），由 CLI 按 jobId 回读状态，而不是报一个假的失败。
pub async fn submit(ctx: &Ctx, root: &Path, dir_name: &str) -> AppResult<Option<JobView>> {
    let _guard = ctx.lock.lock().await;
    if !super::pending_dir(root).join(dir_name).is_dir()
        && repo::dir_known(&ctx.pool, dir_name).await?
    {
        return Ok(None);
    }
    let dir = job_dir(root, dir_name)?;
    if !dir.join(super::READY).is_file() {
        return Err(AppError::InvalidInput(format!(
            "工单尚未写完（缺少 {}）：{dir_name}",
            super::READY
        )));
    }
    ingest_dir(ctx, root, &dir, dir_name).await
}

/// 放行一份超阈值待确认的工单。确认只能由 Agent 在用户明确同意后调用。
pub async fn confirm(ctx: &Ctx, root: &Path, job_id: &str) -> AppResult<Option<JobView>> {
    let _guard = ctx.lock.lock().await;
    let job = repo::get_by_job_id(&ctx.pool, job_id)
        .await?
        .ok_or_else(|| AppError::InvalidInput(format!("未找到工单：{job_id}")))?;
    if job.status != "hold" {
        return Err(AppError::InvalidInput(
            "只有「待确认」的工单需要确认".into(),
        ));
    }
    let dir = job_dir(root, &job.dir_name)?;
    std::fs::write(
        dir.join(super::CONFIRM_FILE),
        format!("经 CLI 确认开跑（{}）\n", now_unix()),
    )
    .map_err(|e| AppError::Io(format!("写确认文件失败：{e}")))?;
    ingest_dir(ctx, root, &dir, &job.dir_name).await
}

/// 按 jobId 重试失败或中断的工单；只补未完成步骤。
pub async fn retry(ctx: &Ctx, root: &Path, job_id: &str) -> AppResult<Option<JobView>> {
    let job = repo::get_by_job_id(&ctx.pool, job_id)
        .await?
        .ok_or_else(|| AppError::InvalidInput(format!("未找到工单：{job_id}")))?;
    Ok(retry_job(ctx, root, job.id).await?.into_iter().next())
}

/// 收件目录里的一个工单目录。名字只能是单层安全目录名。
fn job_dir(root: &Path, dir_name: &str) -> AppResult<PathBuf> {
    let safe = !dir_name.is_empty()
        && !dir_name.starts_with('_')
        && !dir_name.starts_with('.')
        && !dir_name.contains(['/', '\\'])
        && dir_name != "..";
    let dir = super::pending_dir(root).join(dir_name);
    if !safe || !dir.is_dir() {
        return Err(AppError::InvalidInput(format!(
            "工单目录不存在：{dir_name}"
        )));
    }
    Ok(dir)
}

/// Resume only the unfinished stages of the same ledger row under the shared lock.
/// Stage identities live in SQLite; missing legacy identities are never guessed.
pub async fn retry_job(ctx: &Ctx, root: &Path, row_id: i64) -> AppResult<Vec<JobView>> {
    let _guard = ctx.lock.lock().await;
    let Some(job) = repo::get_optional(&ctx.pool, row_id).await? else {
        return Ok(Vec::new());
    };
    if job.status == "done" {
        return Ok(Vec::new());
    }
    if job.status == "hold" {
        let dir = job_dir(root, &job.dir_name)?;
        return Ok(ingest_dir(ctx, root, &dir, &job.dir_name)
            .await?
            .into_iter()
            .collect());
    }
    let saved = repo::checkpoint(&ctx.pool, &job.job_id).await?;
    if saved.is_none()
        && (job.status == "running"
            || job.group_count > 0
            || job.ref_count > 0
            || !job.batch_ids.is_empty())
    {
        return Err(AppError::InvalidInput(
            "这份旧工单没有恢复检查点，无法确认已导入身份；已保留台账，请先核对遗留，不能整单重放"
                .into(),
        ));
    }
    let dir = super::pending_dir(root).join(&job.dir_name);
    let plan = super::plan(&dir, &job.dir_name).map_err(AppError::InvalidInput)?;
    if plan.job_id != job.job_id {
        let mut checkpoint: Checkpoint = match saved {
            Some(raw) => serde_json::from_str(&raw)?,
            None => Checkpoint {
                unbound_identity: job.job_id == job.dir_name,
                ..Default::default()
            },
        };
        if !checkpoint.unbound_identity
            || !checkpoint.fingerprint.is_empty()
            || !checkpoint.prompt_ids.is_empty()
            || !checkpoint.refs.is_empty()
            || job.group_count > 0
            || job.ref_count > 0
            || !job.batch_ids.is_empty()
        {
            return Err(AppError::InvalidInput("重试不能修改原工单 jobId".into()));
        }
        checkpoint.unbound_identity = false;
        repo::rebind_empty_job(
            &ctx.pool,
            row_id,
            &plan.job_id,
            &serde_json::to_string(&checkpoint)?,
        )
        .await?;
    }
    repo::mark_running(&ctx.pool, row_id).await?;
    let result = run_plan(ctx, root, &plan, row_id).await?;
    Ok(result.into_iter().collect())
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
    //    `gendesk status` 也查不到。这正是 jobId 默认取目录名的代价：
    //    同一天同一主题投第二次（`20260729-卡套-broll`）必然撞键。
    //
    //    只在**一个回执都没有**时写：已成功但移档失败的（有 结果.txt）、
    //    已记 error / hold 的（有 错误.txt / 待确认.txt）都是稳态，不该被反复打扰。
    if let Some(mut previous) = repo::get_by_job_id(&ctx.pool, &plan.job_id).await? {
        // Commit can precede the receipt write or archival by an interruption. Reconcile
        // the same content from the durable done ledger instead of replaying its batch.
        if previous.status == "done" && previous.dir_name == dir_name {
            if let Some(raw) = repo::checkpoint(&ctx.pool, &plan.job_id).await? {
                let checkpoint: Checkpoint = serde_json::from_str(&raw)?;
                if super::preflight::check(&plan)
                    .await
                    .is_ok_and(|checked| checked.fingerprint == checkpoint.fingerprint)
                {
                    write_receipt(
                        dir,
                        super::RESULT_FILE,
                        &result_text(&plan, &checkpoint.applied),
                    );
                    if write_json_receipt(dir, &previous) {
                        move_to_consumed(root, dir, dir_name);
                    }
                    return Ok(None);
                }
            }
        }
        // 文件确认与 UI 确认使用同一个临界区；只解除本目录的 hold，绝不重放 error/run/done。
        if previous.status == "hold"
            && previous.dir_name == dir_name
            && dir.join(super::CONFIRM_FILE).is_file()
        {
            repo::delete(&ctx.pool, previous.id).await?;
        } else {
            if !has_receipt(dir) {
                write_receipt(dir, super::DUPLICATE_FILE, &duplicate_text(&plan.job_id));
                previous.status = "duplicate".into();
                previous.dir_name = dir_name.to_string();
                previous.message = duplicate_text(&plan.job_id);
                write_json_receipt(dir, &previous);
                tracing::warn!(job_id = %plan.job_id, "工单名与台账重复，未收录");
            }
            return Ok(None);
        }
    }

    // 3. 阈值与 Poster 归属。**确认.txt 是确认的唯一表达**——`gendesk confirm` 做的事就是
    //    替你写下它；确认同时放行两项（用户可能明确不需要进 Poster）。
    let confirmed = dir.join(super::CONFIRM_FILE).is_file();
    let total = plan.task_count();
    if !confirmed {
        // 阈值管的是花钱：外部成图导入不调用生图，不受它限制。
        let over = !plan.import && ctx.threshold > 0 && total > ctx.threshold;
        let poster = PosterIssues {
            styles: ctx.poster.unresolved(&plan.poster_keys()).await,
            types: ctx.poster.unresolved_types(&plan.poster_types()).await,
        };
        if over || !poster.is_empty() {
            let threshold = over.then_some(ctx.threshold);
            return record_hold(ctx, &plan, total, threshold, &poster).await;
        }
    }

    // 4. 记账在动手之前。
    let row_id = start_job(ctx, &plan.job_id, &plan.dir_name).await?;
    write_json_receipt(dir, &repo::get(&ctx.pool, row_id).await?);

    run_plan(ctx, root, &plan, row_id).await
}

async fn run_plan(ctx: &Ctx, root: &Path, plan: &Plan, row_id: i64) -> AppResult<Option<JobView>> {
    // 5. 干活。进度直接写进 `done`，失败时它就是「做到哪了」的如实交代。
    let mut done = Applied::default();
    match apply(ctx, plan, row_id, &mut done).await {
        Ok(()) => {
            write_receipt(&plan.dir, super::RESULT_FILE, &result_text(plan, &done));
            let view = repo::get(&ctx.pool, row_id).await?;
            let receipt_written = write_json_receipt(&plan.dir, &view);
            // 移档留证（同 v0.9.0 / v2v）：成功的工单从收件目录消失，人一眼看得出还剩什么。
            // 移不动就留着——DB 那行已经挡住重投了，最坏只是目录里多一份留档。
            if receipt_written {
                move_to_consumed(root, &plan.dir, &plan.dir_name);
            }
            ctx.engine.kick();
            Ok(Some(view))
        }
        Err(e) => {
            let msg = e.to_string();
            repo::mark_error(&ctx.pool, row_id, &msg, &done).await?;
            write_receipt(&plan.dir, super::ERROR_FILE, &error_text(&msg, &done));
            let view = repo::get(&ctx.pool, row_id).await?;
            write_json_receipt(&plan.dir, &view);
            Ok(Some(view))
        }
    }
}

// New jobs always acquire an empty recovery identity in the same transaction as their ledger.
async fn start_job(ctx: &Ctx, job_id: &str, dir_name: &str) -> AppResult<i64> {
    start_job_with_state(ctx, job_id, dir_name, &Checkpoint::default()).await
}

async fn start_job_with_state(
    ctx: &Ctx,
    job_id: &str,
    dir_name: &str,
    state: &Checkpoint,
) -> AppResult<i64> {
    let mut tx = ctx.pool.begin().await?;
    let id = repo::insert_running(&mut *tx, job_id, dir_name).await?;
    repo::save_checkpoint(&mut tx, job_id, &serde_json::to_string(state)?).await?;
    tx.commit().await?;
    Ok(id)
}

/// 校验阶段就失败的工单：记一行 error（同样占住去重键，不会每轮重报）+ 写 错误.txt。
async fn record_failure(
    ctx: &Ctx,
    dir: &Path,
    dir_name: &str,
    job_id: &str,
    msg: String,
) -> AppResult<Option<JobView>> {
    // Parameter/body validation can fail even when the envelope already contains a valid
    // explicit jobId. Retain it instead of substituting the directory name.
    let explicit = std::fs::read_to_string(dir.join(super::ORDER))
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|value| {
            value
                .get("jobId")
                .or_else(|| value.get("job_id"))
                .or_else(|| value.get("id"))
                .and_then(|id| id.as_str())
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(str::to_string)
        });
    let job_id = explicit.as_deref().unwrap_or(job_id);
    if repo::exists(&ctx.pool, job_id).await? {
        return Ok(None);
    }
    let state = Checkpoint {
        unbound_identity: explicit.is_none(),
        ..Default::default()
    };
    let row_id = start_job_with_state(ctx, job_id, dir_name, &state).await?;
    // 校验阶段就失败 —— 库里确实一个字节都没变，这里的「没有导入任何东西」是真话。
    let nothing = Applied::default();
    repo::mark_error(&ctx.pool, row_id, &msg, &nothing).await?;
    write_receipt(dir, super::ERROR_FILE, &error_text(&msg, &nothing));
    let view = repo::get(&ctx.pool, row_id).await?;
    write_json_receipt(dir, &view);
    Ok(Some(view))
}

/// 超阈值或 Poster 款式对不上：**什么都不导入**，记一行 hold + 写 待确认.txt。
async fn record_hold(
    ctx: &Ctx,
    plan: &Plan,
    total: i64,
    threshold: Option<i64>,
    poster: &PosterIssues,
) -> AppResult<Option<JobView>> {
    let mut reasons = Vec::new();
    if let Some(threshold) = threshold {
        reasons.push(format!(
            "共 {total} 张图（{total} 个条目），超过阈值 {threshold}"
        ));
    }
    if !poster.styles.is_empty() {
        reasons.push(format!("Poster 款式对不上：{}", poster.styles.join("；")));
    }
    if !poster.types.is_empty() {
        reasons.push(format!(
            "Poster 图片类型对不上：{}",
            poster.types.join("；")
        ));
    }
    let msg = format!("{}，等待确认", reasons.join("；"));
    let row_id = start_job(ctx, &plan.job_id, &plan.dir_name).await?;
    repo::mark_hold(&ctx.pool, row_id, &msg, total, total).await?;
    write_receipt(
        &plan.dir,
        super::HOLD_FILE,
        &hold_text(plan, total, threshold, poster),
    );
    let view = repo::get(&ctx.pool, row_id).await?;
    write_json_receipt(&plan.dir, &view);
    Ok(Some(view))
}

/// 一次收录的产出（写台账用）。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Applied {
    pub batch_ids: Vec<i64>,
    pub params_json: Vec<String>,
    pub wire_json: Vec<String>,
    pub task_count: i64,
    pub group_count: i64,
    pub ref_count: i64,
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Checkpoint {
    #[serde(default)]
    unbound_identity: bool,
    fingerprint: String,
    prompt_ids: Vec<i64>,
    group_ids: Vec<i64>,
    refs: Vec<(PathBuf, i64)>,
    /// 参考图库抽图结果（0051）。空 = 还没抽或本单没有图库引用；抽过就原样沿用。
    #[serde(default)]
    library: Vec<crate::ref_library::SlotPick>,
    applied: Applied,
}

impl Checkpoint {
    async fn save(
        &self,
        conn: &mut sqlx::SqliteConnection,
        plan: &Plan,
        row_id: i64,
        finished: bool,
    ) -> AppResult<()> {
        repo::save_checkpoint(&mut *conn, &plan.job_id, &serde_json::to_string(self)?).await?;
        repo::mark_done(&mut *conn, row_id, &self.applied).await?;
        if !finished {
            repo::mark_running(&mut *conn, row_id).await?;
        }
        Ok(())
    }
}

/// Each imported stage and its identity checkpoint commit together. The final batch,
/// tasks and done receipt commit together, so a process interruption cannot replay a batch.
async fn apply(ctx: &Ctx, plan: &Plan, row_id: i64, out: &mut Applied) -> AppResult<()> {
    let saved = repo::checkpoint(&ctx.pool, &plan.job_id).await?;
    let mut checkpoint: Checkpoint = match saved {
        Some(raw) => serde_json::from_str(&raw)?,
        None => Checkpoint::default(),
    };
    *out = checkpoint.applied.clone();
    let checked = super::preflight::check(plan).await?;
    if !checkpoint.fingerprint.is_empty()
        && checkpoint.fingerprint != checked.fingerprint
        && (!checkpoint.prompt_ids.is_empty()
            || !checkpoint.refs.is_empty()
            || !checkpoint.library.is_empty())
    {
        return Err(AppError::InvalidInput("工单内容或参考图已改变，不能复用已导入检查点；请还原原内容后重试，或先核对原工单再另行授权新单".into()));
    }
    if checkpoint.prompt_ids.is_empty()
        && !plan.import
        && ctx.threshold > 0
        && plan.task_count() > ctx.threshold
        && !plan.dir.join(super::CONFIRM_FILE).is_file()
    {
        return Err(AppError::InvalidInput(
            "修订工单超过当前阈值，请先明确确认再重试".into(),
        ));
    }
    checkpoint.fingerprint = checked.fingerprint.clone();
    checkpoint.unbound_identity = false;
    if checkpoint
        .refs
        .iter()
        .any(|(path, _)| !plan.distinct_refs().contains(path))
    {
        return Err(AppError::InvalidInput(
            "参考图路径与已导入检查点不一致，请还原原工单路径".into(),
        ));
    }
    if !checkpoint.applied.batch_ids.is_empty() {
        // A committed batch is terminal even if an old UI/receipt still reports an error.
        let mut tx = ctx.pool.begin().await?;
        checkpoint.save(&mut tx, plan, row_id, true).await?;
        tx.commit().await?;
        *out = checkpoint.applied;
        return Ok(());
    }
    let mut tx = ctx.pool.begin().await?;
    checkpoint.save(&mut tx, plan, row_id, false).await?;
    tx.commit().await?;

    // ── 参考图库：在导入任何内容之前抽图。抽中即记账，与检查点同事务；
    //    无图可抽就整单失败，此时什么都没导入、也没记账。重试沿用检查点里的结果。
    let library_slots = plan.library_slots();
    if !library_slots.is_empty() && checkpoint.library.is_empty() {
        let requests: Vec<_> = library_slots
            .iter()
            .map(|(item, slot, spec)| crate::ref_library::SlotRequest {
                item: *item,
                slot: *slot,
                item_name: &plan.items[*item].name,
                spec,
            })
            .collect();
        let mut tx = ctx.pool.begin().await?;
        checkpoint.library = crate::ref_library::draw_for_job(
            &mut tx,
            &ctx.pool,
            &plan.job_id,
            &requests,
            crate::db::now_unix(),
            &mut crate::ref_library::os_rng,
        )
        .await?;
        checkpoint.save(&mut tx, plan, row_id, false).await?;
        tx.commit().await?;
    }
    if checkpoint.library.len() != library_slots.len()
        || checkpoint
            .library
            .iter()
            .zip(&library_slots)
            .any(|(pick, (item, slot, _))| pick.item != *item || pick.slot != *slot)
    {
        return Err(AppError::InvalidInput(
            "参考图库抽图检查点与工单不一致，已停止重试".into(),
        ));
    }
    for pick in &checkpoint.library {
        let existing = refs_repo::get(&ctx.pool, pick.ref_image_id).await?;
        if !existing.is_some_and(|r| r.deleted_at.is_none() && Path::new(&r.file_path).is_file()) {
            return Err(AppError::InvalidInput(format!(
                "抽到的图库图片 #{} 文件缺失，不能继续",
                pick.image_id
            )));
        }
    }

    // ── 提示词：一单一组，一条条目 = 一条提示词。走的是手动导入那条落库路径
    //    （`build_preview_from_parsed` → `commit_preview`），故前缀分配、号池、
    //    用途校验三处口径与手工导入完全一致 —— 铁律「写路径只有一条」。
    //
    //    「组」在工单 v2 里已经不是一个概念（`items[].name` 即身份），但 prompts
    //    的外键要求它有个落脚点，而编号是**按前缀**发的。故这里造一个以投单为名的组：
    //    它是容器，不是身份。
    let parsed = ParsedGroup {
        name: plan.note.clone(),
        prefix: None,
        scene: String::new(),
        tags: Vec::new(),
        origin: crate::importer::GroupOrigin::Explicit,
        skill: plan.skill.clone(),
        prompts: plan
            .items
            .iter()
            .map(|it| crate::importer::ParsedPrompt {
                title: None,
                text: it.prompt_text.clone(),
                item_name: it.name.clone(),
                sku: it.sku.clone(),
                style: it.style.clone(),
            })
            .collect(),
        ..Default::default()
    };
    let mut preview =
        build_preview_from_parsed(&ctx.pool, &[parsed], "UTF-8".into(), Vec::new()).await?;
    // 用途是**整单级**的（0045）：工单里写了就是它，不走关键词预猜 —— 预猜是给
    // 手工导入的兜底，而工单里作者已经明确表过态。
    for g in &mut preview.groups {
        g.purposes = if plan.purpose.is_empty() {
            Vec::new()
        } else {
            vec![plan.purpose.clone()]
        };
        g.purpose_inferred = false;
    }
    if checkpoint.prompt_ids.is_empty() {
        let mut tx = ctx.pool.begin().await?;
        let result = commit_preview_in_tx(&mut tx, &preview, "library").await?;
        if result.prompt_ids.len() != plan.items.len() || result.group_ids.len() != 1 {
            return Err(AppError::Internal(
                "提示词落库数与工单条目数不一致，已回滚".into(),
            ));
        }
        checkpoint.prompt_ids = result.prompt_ids;
        checkpoint.group_ids = result.group_ids;
        checkpoint.applied.group_count = checkpoint.prompt_ids.len() as i64;
        checkpoint.save(&mut tx, plan, row_id, false).await?;
        tx.commit().await?;
        *out = checkpoint.applied.clone();
    }
    if checkpoint.prompt_ids.len() != plan.items.len() || checkpoint.group_ids.len() != 1 {
        return Err(AppError::InvalidInput(
            "提示词恢复检查点不完整，已停止重试".into(),
        ));
    }
    for id in &checkpoint.prompt_ids {
        if !crate::db::repo::prompts::get(&ctx.pool, *id)
            .await?
            .is_some_and(|prompt| prompt.status == "active")
        {
            return Err(AppError::InvalidInput(
                "已导入提示词被删除，不能自动重新导入".into(),
            ));
        }
    }
    let group_id = checkpoint.group_ids[0];

    ctx.dirs.init()?;
    let wanted = plan.distinct_refs();
    let total_refs = wanted.len() as i64;
    for (path, id) in &checkpoint.refs {
        let existing = refs_repo::get(&ctx.pool, *id).await?;
        if !existing.is_some_and(|r| r.deleted_at.is_none() && Path::new(&r.file_path).is_file()) {
            return Err(AppError::InvalidInput(format!(
                "已导入参考图被删除或缺失：{}，不能自动重新导入",
                path.display()
            )));
        }
    }
    let pending: Vec<_> = wanted
        .iter()
        .filter(|path| !checkpoint.refs.iter().any(|(source, _)| source == *path))
        .collect();
    for chunk in pending.chunks(crate::files::decode::max_concurrent()) {
        let prepared = chunk.iter().map(|path| {
            let expected = checked
                .references
                .iter()
                .find(|r| &r.source == *path)
                .map(|r| r.content_hash.as_str());
            prepare_reference(ctx, &plan.job_id, path, expected)
        });
        for (path, ing) in chunk
            .iter()
            .zip(futures_util::future::join_all(prepared).await)
        {
            let ing = ing?;
            let mut tx = ctx.pool.begin().await?;
            let id = refs_repo::insert(
                &mut *tx,
                &refs_repo::NewRefImage {
                    name: ing.name,
                    file_path: ing.file_path,
                    thumb_path: ing.thumb_path,
                    width: ing.width,
                    height: ing.height,
                    file_size: ing.file_size,
                    content_hash: ing.content_hash,
                    upload_path: ing.upload_path,
                    ephemeral: false,
                },
            )
            .await?;
            checkpoint.refs.push(((*path).clone(), id));
            checkpoint.applied.ref_count = checkpoint.refs.len() as i64;
            checkpoint.save(&mut tx, plan, row_id, false).await?;
            tx.commit().await?;
            *out = checkpoint.applied.clone();
            ctx.progress.refs(&plan.job_id, out.ref_count, total_refs);
        }
    }
    let ref_ids = &checkpoint.refs;

    // ── 配对：items[i] 的参考图 ↔ prompt_ids[i]。两个清单同序是上面那句
    //    「落库数不一致就整单失败」在守着的东西。
    let mut specs = Vec::with_capacity(plan.items.len());
    for (it, prompt_id) in plan.items.iter().zip(checkpoint.prompt_ids.iter().copied()) {
        let item_index = specs.len();
        let ids: Vec<i64> = it
            .slots
            .iter()
            .enumerate()
            .map(|(slot_index, slot)| {
                match slot {
                    super::RefSlot::File(path) => {
                        ref_ids.iter().find(|(p, _)| p == path).map(|(_, id)| *id)
                    }
                    super::RefSlot::Library(_) => checkpoint
                        .library
                        .iter()
                        .find(|p| p.item == item_index && p.slot == slot_index)
                        .map(|p| p.ref_image_id),
                }
                .ok_or_else(|| AppError::Internal(format!("条目「{}」的参考图没能入库", it.name)))
            })
            .collect::<AppResult<_>>()?;
        let ref_image_id = ids[0];
        let mut params_json = it.params_json.clone();
        if ids.len() > 1 {
            let mut params: serde_json::Value = serde_json::from_str(&params_json)?;
            params["referenceImageIds"] = serde_json::json!(ids);
            params_json = params.to_string();
        }
        specs.push(engine::TaskSpec {
            ref_image_id,
            prompt_id,
            prompt_group_id: group_id,
            prompt_text: it.prompt_text.clone(),
            params_json,
        });
    }

    // ── 建批（花钱的那一步，放在最后）。**恰好一个**：一次工单 = 一个 run（0045）。
    //
    // 批次级 params_json 留空：这一单的参数在任务行上，批次级那份只是生成页手动建批
    // 的兜底。写一个「代表性的」进去反而骗人 —— 一单里可能有三种画幅。
    //
    // 外部成图导入单：成图先落到与生图结果相同的草稿位置（事务外，文件无法回滚），
    // 再在**同一个事务**里建批并把任务直接置为待验收。调度器只认已提交的 q 态，
    // 而这些任务提交时已经是 rev，所以不会有任何一张被拿去调接口。
    let mut staged_files = crate::files::copy::CreatedFilesGuard::new(Vec::new());
    let staged = if plan.import {
        stage_results(ctx, plan, &checked, &mut staged_files).await?
    } else {
        Vec::new()
    };
    let output_dir = ctx.dirs.outputs().to_string_lossy().to_string();
    let mut tx = ctx.pool.begin().await?;
    let (batch_id, task_count) =
        engine::create_batch_from_specs_in_tx(&mut tx, &output_dir, "{}", &specs).await?;
    task_repo::rename_batch(&mut *tx, batch_id, &plan.note).await?;
    if plan.import {
        let task_ids: Vec<i64> =
            sqlx::query_scalar("SELECT id FROM tasks WHERE batch_id = ?1 ORDER BY id")
                .bind(batch_id)
                .fetch_all(&mut *tx)
                .await?;
        if task_ids.len() != staged.len() {
            return Err(AppError::Internal(
                "外部成图数与任务数不一致，已回滚".into(),
            ));
        }
        for (task_id, result) in task_ids.iter().zip(&staged) {
            task_repo::mark_review(
                &mut *tx,
                *task_id,
                &result.image.to_string_lossy(),
                &result.thumb.to_string_lossy(),
                result.size,
            )
            .await?;
        }
    }
    checkpoint.applied.batch_ids = vec![batch_id];
    for (params, wire) in plan.param_sets() {
        checkpoint.applied.params_json.push(params);
        checkpoint.applied.wire_json.push(wire);
    }
    checkpoint.applied.task_count = task_count;
    checkpoint.save(&mut tx, plan, row_id, true).await?;
    if plan.import {
        repo::set_message(&mut *tx, row_id, &import_message(task_count)).await?;
    }
    tx.commit().await?;
    staged_files.preserve_all();
    *out = checkpoint.applied;
    Ok(())
}

/// 导入单完成时台账上的一句话（应用通知与 CLI 回执都读它）。
pub(crate) fn import_message(count: i64) -> String {
    format!("外部成图 {count} 张已进待验收，未调用生图")
}

/// 落好的一张外部成图。
struct StagedResult {
    image: PathBuf,
    thumb: PathBuf,
    size: Option<(u32, u32)>,
}

/// 外部成图 → 与生图结果同一个草稿位置（作品根 `_草稿/生成图片/产品/款式/需求/`）+ 缩略图。
///
/// 输出处理与生图同一个函数（`openai::deliver`）：按条目的格式与「清元数据 / 去 C2PA」
/// 开关处理，所以外部成图验收通过后交付出去的文件与 GenDesk 自己生成的同一口径。
/// 文件名身份由 jobId 与条目序号决定：重试时覆盖同一个文件，不会越积越多。
async fn stage_results(
    ctx: &Ctx,
    plan: &Plan,
    checked: &super::preflight::CheckedPlan,
    created: &mut crate::files::copy::CreatedFilesGuard,
) -> AppResult<Vec<StagedResult>> {
    use sha2::{Digest, Sha256};
    if checked.results.len() != plan.items.len() {
        return Err(AppError::InvalidInput(
            "外部成图导入单每条都要有成图".into(),
        ));
    }
    let root = crate::library::root(&ctx.pool).await?;
    let job_hash = format!("{:x}", Sha256::digest(plan.job_id.as_bytes()));
    let thumbs = ctx.dirs.thumbs().join("import").join(&job_hash);
    std::fs::create_dir_all(&thumbs)?;
    let at = now_unix();
    let mut out = Vec::with_capacity(plan.items.len());
    for (index, (item, check)) in plan.items.iter().zip(&checked.results).enumerate() {
        let source = item
            .result_path
            .as_ref()
            .ok_or_else(|| AppError::InvalidInput(format!("条目「{}」缺成图", item.name)))?;
        let raw = std::fs::read(source)?;
        if crate::files::content_hash_bytes(&raw) != check.content_hash {
            return Err(AppError::InvalidInput(format!(
                "条目「{}」的成图在预检后发生变化，未导入",
                item.name
            )));
        }
        let params = crate::provider::GenParams::from_json(&item.params_json);
        let image = crate::provider::openai::deliver(
            &raw,
            params.output_format.as_deref(),
            params.clear_ai_metadata.unwrap_or(true),
            params.remove_c2pa.unwrap_or(true),
        )
        .map_err(|e| {
            AppError::InvalidInput(format!(
                "条目「{}」的成图处理失败：{}",
                item.name, e.message
            ))
        })?;
        let identity = format!("import{}n{}", &job_hash[..10], index + 1);
        let full = match crate::library::Organization::from_params(&item.params_json)? {
            Some(organization) => {
                let dir = organization.directory(&root, false, false);
                std::fs::create_dir_all(&dir)?;
                dir.join(crate::library::filename(
                    &organization.requirement,
                    &identity,
                    at,
                    &image.ext,
                ))
            }
            None => {
                std::fs::create_dir_all(ctx.dirs.results())?;
                ctx.dirs.results().join(format!("{identity}.{}", image.ext))
            }
        };
        std::fs::write(&full, &image.bytes)?;
        created.track(full.clone());
        let thumb = thumbs.join(format!("{}.jpg", index + 1));
        let (full_c, thumb_c) = (full.clone(), thumb.clone());
        let size = crate::files::decode::bounded(move |permit| {
            crate::files::generate_thumbnail(&full_c, &thumb_c, permit)
        })
        .await
        .ok();
        let thumb = if size.is_some() {
            created.track(thumb.clone());
            thumb
        } else {
            full.clone()
        };
        out.push(StagedResult {
            image: full,
            thumb,
            size,
        });
    }
    Ok(out)
}

async fn prepare_reference(
    ctx: &Ctx,
    job_id: &str,
    path: &Path,
    expected_hash: Option<&str>,
) -> AppResult<crate::commands::refs::Ingested> {
    let expected_hash = expected_hash.ok_or_else(|| AppError::Internal("参考图预检缺失".into()))?;
    let job_hash = crate::files::content_hash_bytes(job_id.as_bytes());
    let source_hash = crate::files::content_hash_bytes(path.to_string_lossy().as_bytes());
    // A committed reference is skipped above. Only this uncommitted reference owns these
    // private paths; restart may remove its orphan files before materializing it again.
    let rd = ctx
        .dirs
        .refs()
        .join("intake")
        .join(&job_hash)
        .join(&source_hash);
    let td = ctx
        .dirs
        .thumbs()
        .join("intake")
        .join(&job_hash)
        .join(&source_hash);
    for dir in [&rd, &td] {
        if dir.exists() {
            std::fs::remove_dir_all(dir)?;
        }
        std::fs::create_dir_all(dir)?;
    }
    let source = path.to_string_lossy().to_string();
    let ing =
        crate::files::decode::bounded(move |permit| ingest_one(&source, &rd, &td, permit)).await?;
    if ing.content_hash.as_deref() != Some(expected_hash) {
        return Err(AppError::InvalidInput(
            "参考图在预检后发生变化，未导入本张".into(),
        ));
    }
    Ok(ing)
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

/// 回执文件：让投单的 Agent 能回读结果，不必猜后面发生了什么。
///
/// 失败与待确认的目录**不移走**：回执要和提示词文档摆在一起才有用——改完就地重投。
fn write_receipt(dir: &Path, name: &str, body: &str) {
    if let Err(e) = std::fs::write(dir.join(name), body) {
        tracing::warn!(error = %e, file = %name, "写工单回执失败");
    }
}

fn write_json_receipt(dir: &Path, job: &JobView) -> bool {
    let receipt = serde_json::json!({ "version": 1, "job": job });
    if let Err(e) = super::write_json(&dir.join(super::RECEIPT_FILE), &receipt) {
        tracing::warn!(error = %e, "写 JSON 工单回执失败");
        return false;
    }
    true
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
         1. 想跑：用新的 `--job-id` 重新 `gendesk submit`；\n\
         2. 确认是重复投递、本来就不该跑：直接删掉整个工单目录。\n\n\
         想查是哪一份占了这个名字：`gendesk status <jobId>`。\n"
    )
}

fn result_text(plan: &Plan, done: &Applied) -> String {
    if plan.import {
        return format!(
            "GenDesk 已收录外部成图，未调用生图。\n\n\
             工单：{}\n成图：{} 张（已进待验收）\n参考图：{}\n批次：{}\n\n\
             验收请在 GenDesk 的验收页完成。\n",
            plan.job_id,
            done.task_count,
            done.ref_count,
            done.batch_ids
                .iter()
                .map(|b| format!("#{b}"))
                .collect::<Vec<_>>()
                .join(" "),
        );
    }
    let mut s = format!(
        "GenDesk 已收录并开跑。\n\n\
         工单：{}\n条目：{}\n参考图：{}\n任务（= 出图张数）：{}\n批次：{}\n\n",
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
    for w in &done.wire_json {
        s.push_str(&format!("实际发往接口的字段：{w}\n"));
    }
    s.push_str("\n验收请在 GenDesk 的验收页完成。\n");
    s
}

fn hold_text(plan: &Plan, total: i64, threshold: Option<i64>, poster: &PosterIssues) -> String {
    let mut s = String::from(
        "这份工单在等待确认，**还没有导入任何东西**（提示词、参考图一条都没进库）。\n\n",
    );
    if let Some(threshold) = threshold {
        s.push_str(&format!(
            "超过自动开跑阈值：共 {total} 张图 = {total} 个条目（一条 = 一个任务 = 一张图）\n\
             参考图 {} 张 · 将建 1 个批次（一次工单 = 一个 run）\n\
             当前阈值：{threshold}\n\n",
            plan.distinct_refs().len(),
        ));
    }
    if !poster.styles.is_empty() {
        s.push_str("已开启「自动导入 Poster」，但这些款式审稿通过后进不了 Poster：\n");
        for line in &poster.styles {
            s.push_str(&format!("- {line}\n"));
        }
        s.push_str(
            "\n款式名写错：改成参考图库里的产品 / 款式名后，用新的 jobId 重新投单，删掉本目录。\n\
             新款式第一次投单：用户确认对应商品后运行 \
             `gendesk poster bind --product <产品> --style <款式> --target <Poster 商品名>`。\n\
             用户明确不需要进 Poster 时可直接确认，审稿后在设置里对该批选「不导入」。\n\n",
        );
    }
    if !poster.types.is_empty() {
        s.push_str("已开启「自动导入 Poster」，但这些显式指定的 Poster 图片类型对不上：\n");
        for line in &poster.types {
            s.push_str(&format!("- {line}\n"));
        }
        s.push_str(
            "\nposterType 写错：改成 `gendesk poster resolve` 列出的 Poster 图片类型、`封面` 或 `未分类`\
             （或删掉 posterType 按需求名自动对应），用新的 jobId 重新投单，删掉本目录。\n\
             需要新类型：请用户先在 Poster 里新建同名类型，再确认。\n\n",
        );
    }
    s.push_str(&format!(
        "用户明确同意后运行 `gendesk confirm {}` 开跑；不想跑就直接删掉整个工单目录。\n",
        plan.job_id,
    ));
    s
}

/// 失败回执。**如实交代已经导入了多少** —— 这不是措辞问题，是重投安全的前提。
///
/// 收录这一串动作没有一步能整体回滚：参考图要拷文件建缩略图，建批要发编号，
/// 第一个批次建完那一刻就已经在花钱跑了。原来这里无条件写「没有导入任何东西」，
/// 而人照着它去点「重试」，得到的是第二份提示词和第二个批次 —— 花两份钱。
fn error_text(msg: &str, done: &Applied) -> String {
    format!(
        "GenDesk 收录失败：\n\n{msg}\n\n已记录的导入进度：提示词 {} 条，参考图 {} 张，任务 {} 个，批次 {:?}。\n\n\
         新工单保留了导入身份检查点：用 `gendesk retry <jobId>` 对原工单重试，只补未完成步骤，不能换 ID 整单重投。\n\
         已有导入后不得修改原正文、归属、参数或参考图；内容冲突时会停止恢复。\n\
         没有检查点的历史部分导入或中断工单不会盲目重放，请保留回执核对遗留。\n",
        done.group_count, done.ref_count, done.task_count, done.batch_ids
    )
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

    /// 造一份 v2 工单目录（工单.json + prompts/ + refs/ + 最后写的 READY.txt）。
    ///
    /// `items` = (条目名, 正文, 参考图文件名, 条目级参数 JSON 片段)。
    fn make_job(
        root: &Path,
        dir_name: &str,
        head: &str,
        items: &[(&str, &str, &str, &str)],
    ) -> PathBuf {
        let dir = super::super::pending_dir(root).join(dir_name);
        std::fs::create_dir_all(dir.join(super::super::PROMPTS)).unwrap();
        std::fs::create_dir_all(dir.join(super::super::REFS)).unwrap();
        let mut refs_written: Vec<String> = Vec::new();
        let mut specs = Vec::new();
        for (name, text, img, extra) in items {
            std::fs::write(
                dir.join(super::super::PROMPTS).join(format!("{name}.txt")),
                text,
            )
            .unwrap();
            if !refs_written.contains(&img.to_string()) {
                std::fs::write(dir.join(super::super::REFS).join(img), png()).unwrap();
                refs_written.push(img.to_string());
            }
            specs.push(format!(
                r#"{{"name":"{name}","sku":"55","style":"王一博","ref":"refs/{img}"{extra}}}"#
            ));
        }
        let order = format!(r#"{{"version":1{head},"items":[{}]}}"#, specs.join(","));
        std::fs::write(dir.join(super::super::ORDER), order).unwrap();
        std::fs::write(dir.join(super::super::READY), "ok").unwrap();
        dir
    }

    /// 最常见的一份：一个条目、一张图、无额外参数。
    fn simple(name: &str) -> Vec<(&str, &str, &str, &str)> {
        vec![(name, "正文一", "a.png", "")]
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
                poster: Arc::new(NoPosterGate),
                lock: Arc::new(tokio::sync::Mutex::new(())),
            },
            kick,
            progress,
        )
    }

    /// 测试便利：按目录名顺序把收件目录里每份带 READY 的工单逐一 `submit`，
    /// 只返回本轮真正处理掉的工单（与 CLI 逐份提交同一条代码路径）。
    async fn scan(ctx: &Ctx, root: &Path) -> AppResult<Vec<JobView>> {
        let pending = super::super::pending_dir(root);
        std::fs::create_dir_all(&pending)?;
        let mut names: Vec<String> = std::fs::read_dir(&pending)?
            .flatten()
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| !n.starts_with('_'))
            .collect();
        names.sort();
        let mut out = Vec::new();
        for name in names {
            // 没写完的跳过（与 CLI 只提交写好 READY 的目录一致）。列目录与提交之间
            // 另一轮可能已把它收录移走——那时目录已不在，submit 对此返回 None。
            let dir = pending.join(&name);
            if dir.exists() && !dir.join(super::super::READY).is_file() {
                continue;
            }
            if let Some(view) = submit(ctx, root, &name).await? {
                out.push(view);
            }
        }
        Ok(out)
    }

    async fn count(pool: &SqlitePool, table: &str) -> i64 {
        sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(pool)
            .await
            .unwrap()
    }

    /// 参考图库：款式「王一博」正面图 3 张、45度角 2 张。返回 (正面图库编号, 45度编号)。
    async fn seed_library(pool: &SqlitePool, data: &Path) -> (Vec<i64>, Vec<i64>) {
        use crate::ref_library as lib;
        let style = lib::save_style(pool, None, "王一博", "", "").await.unwrap();
        let src = data.join("library-src");
        std::fs::create_dir_all(&src).unwrap();
        let dirs = DataDirs::new(data);
        let mut out = Vec::new();
        for (category, n, seed) in [("正面图", 3u8, 10u8), ("45度角", 2, 50)] {
            let category = lib::find_category(pool, category).await.unwrap().id;
            let mut paths = Vec::new();
            for k in 0..n {
                use std::io::Cursor;
                let img = image::RgbImage::from_pixel(2, 2, image::Rgb([seed + k, 1, 2]));
                let mut buf = Cursor::new(Vec::new());
                image::DynamicImage::ImageRgb8(img)
                    .write_to(&mut buf, image::ImageFormat::Png)
                    .unwrap();
                let p = src.join(format!("{seed}-{k}.png"));
                std::fs::write(&p, buf.into_inner()).unwrap();
                paths.push(p.to_string_lossy().to_string());
            }
            let report = lib::import_files(pool, &dirs, style, category, &paths, &|_, _, _, _| {})
                .await
                .unwrap();
            out.push(report.imported.iter().map(|v| v.id).collect());
        }
        let second = out.pop().unwrap();
        (out.pop().unwrap(), second)
    }

    #[tokio::test]
    async fn library_references_are_drawn_once_and_reuse_the_library_files() {
        let (pool, _d) = test_pool().await;
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let (ctx, _) = ctx_for(&pool, data.path(), 100);
        let (front, angle) = seed_library(&pool, data.path()).await;
        let dir = make_job(
            home.path(),
            "library-job",
            r#","organization":{"product":"","style":"王一博","requirement":"场景图","source":"测试"}"#,
            &[("甲", "正文一", "a.png", ""), ("乙", "正文二", "a.png", "")],
        );
        let mut spec: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("工单.json")).unwrap()).unwrap();
        spec["items"][0].as_object_mut().unwrap().remove("ref");
        spec["items"][0]["refs"] =
            serde_json::json!([{"category": "正面图"}, {"category": "45度角"}, "refs/a.png"]);
        spec["items"][1].as_object_mut().unwrap().remove("ref");
        spec["items"][1]["refs"] = serde_json::json!([{"category": "正面图"}]);
        std::fs::write(dir.join("工单.json"), spec.to_string()).unwrap();
        let ref_rows_before = count(&pool, "ref_images").await;

        // 建批那一步失败：抽图已记账，重试沿用检查点里的结果，不再抽第二次。
        sqlx::query("CREATE TRIGGER fail_batch BEFORE INSERT ON batches BEGIN SELECT RAISE(FAIL, 'injected'); END")
            .execute(&pool)
            .await
            .unwrap();
        let jobs = scan(&ctx, home.path()).await.unwrap();
        assert_eq!(jobs[0].status, "error");
        let first = crate::ref_library::picks_for_job(&pool, "library-job")
            .await
            .unwrap();
        assert_eq!(first.len(), 3);
        sqlx::query("DROP TRIGGER fail_batch")
            .execute(&pool)
            .await
            .unwrap();
        let job = retry(&ctx, home.path(), "library-job")
            .await
            .unwrap()
            .unwrap();
        let jobs = [job];
        assert_eq!(jobs[0].status, "done", "{}", jobs[0].message);
        assert_eq!(jobs[0].task_count, 2);
        // 图库图片直接复用原 ref_images 行，只有包内的 a.png 新入库。
        assert_eq!(count(&pool, "ref_images").await, ref_rows_before + 1);
        let picks = crate::ref_library::picks_for_job(&pool, "library-job")
            .await
            .unwrap();
        assert_eq!(picks.len(), 3);
        assert_eq!((picks[0].item_name.as_str(), picks[0].slot), ("甲", 1));
        assert!(front.contains(&picks[0].image.id) && angle.contains(&picks[1].image.id));
        // 同一单两条都要正面图：可用图足够时不重复。
        assert_ne!(picks[0].image.id, picks[2].image.id);
        assert!(picks.iter().all(|p| !p.fallback));
        let ids = |v: &[crate::ref_library::JobLibraryPick]| {
            v.iter().map(|p| p.image.id).collect::<Vec<_>>()
        };
        assert_eq!(ids(&picks), ids(&first));

        let (primary, raw): (i64, String) =
            sqlx::query_as("SELECT ref_image_id, params_json FROM tasks ORDER BY id LIMIT 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        let order = engine::reference_ids(&raw, primary).unwrap();
        assert_eq!(order.len(), 3);
        let library_ref: i64 =
            sqlx::query_scalar("SELECT ref_image_id FROM ref_library_images WHERE id = ?1")
                .bind(picks[0].image.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(order[0], library_ref);
        assert_eq!(count(&pool, "ref_library_usage").await, 3);
    }

    #[tokio::test]
    async fn a_library_reference_without_images_fails_before_importing_anything() {
        let (pool, _d) = test_pool().await;
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let (ctx, _) = ctx_for(&pool, data.path(), 100);
        seed_library(&pool, data.path()).await;
        let dir = make_job(home.path(), "empty-category", "", &simple("甲"));
        let mut spec: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("工单.json")).unwrap()).unwrap();
        spec["items"][0]["ref"] = serde_json::json!({"category": "礼盒图"});
        std::fs::write(dir.join("工单.json"), spec.to_string()).unwrap();
        let jobs = scan(&ctx, home.path()).await.unwrap();
        assert_eq!(jobs[0].status, "error");
        assert!(jobs[0].message.contains("礼盒图"), "{}", jobs[0].message);
        for table in ["prompts", "tasks", "batches", "ref_library_usage"] {
            assert_eq!(count(&pool, table).await, 0, "{table}");
        }
    }

    #[tokio::test]
    async fn ordered_reference_pair_is_persisted_with_one_task_and_both_batch_refs() {
        let (pool, _d) = test_pool().await;
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let (ctx, _) = ctx_for(&pool, data.path(), 100);
        let dir = make_job(home.path(), "two-references", "", &simple("日历场景"));
        std::fs::write(dir.join("refs/desk.png"), png()).unwrap();
        let mut spec: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("工单.json")).unwrap()).unwrap();
        spec["items"][0].as_object_mut().unwrap().remove("ref");
        spec["items"][0]["refs"] = serde_json::json!(["refs/a.png", "refs/desk.png"]);
        std::fs::write(dir.join("工单.json"), spec.to_string()).unwrap();
        let jobs = scan(&ctx, home.path()).await.unwrap();
        assert_eq!(jobs[0].status, "done", "{}", jobs[0].message);
        assert_eq!(jobs[0].task_count, 1);
        assert_eq!(jobs[0].ref_count, 2);
        assert_eq!(count(&pool, "batch_refs").await, 2);
        let (primary, raw): (i64, String) =
            sqlx::query_as("SELECT ref_image_id, params_json FROM tasks")
                .fetch_one(&pool)
                .await
                .unwrap();
        let refs: Vec<i64> = sqlx::query_scalar("SELECT id FROM ref_images ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(engine::reference_ids(&raw, primary).unwrap(), refs);
    }

    #[tokio::test]
    async fn missing_or_ambiguous_secondary_reference_rejects_before_any_import() {
        for extra in [
            serde_json::json!(["refs/a.png", "refs/missing.png"]),
            serde_json::json!(["refs/a.png", "refs/a.png"]),
        ] {
            let (pool, _d) = test_pool().await;
            let home = tempfile::tempdir().unwrap();
            let data = tempfile::tempdir().unwrap();
            let (ctx, _) = ctx_for(&pool, data.path(), 100);
            let dir = make_job(home.path(), "bad-pair", "", &simple("日历场景"));
            let mut spec: serde_json::Value =
                serde_json::from_slice(&std::fs::read(dir.join("工单.json")).unwrap()).unwrap();
            spec["items"][0].as_object_mut().unwrap().remove("ref");
            spec["items"][0]["refs"] = extra;
            std::fs::write(dir.join("工单.json"), spec.to_string()).unwrap();
            let jobs = scan(&ctx, home.path()).await.unwrap();
            assert_eq!(jobs[0].status, "error");
            assert_eq!(count(&pool, "tasks").await, 0);
            assert_eq!(count(&pool, "prompts").await, 0);
            assert_eq!(count(&pool, "ref_images").await, 0);
        }
    }

    /// 端到端：真实 `scripts/gendesk.mjs` 打包 → 控制协议 → 真实 Rust 收录。
    ///
    /// 测试里起一个最小控制通道（只认 hello / intake.status / intake.submit），
    /// 收到 `intake.submit` 就调用本模块的 [`submit`]，与 `control.rs` 的分发一致。
    #[tokio::test(flavor = "multi_thread")]
    async fn cli_manifest_to_real_rust_import_and_cli_receipt() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        let (pool, _d) = test_pool().await;
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let (ctx, kick) = ctx_for(&pool, data.path(), 100);
        std::fs::write(home.path().join("商品实拍.png"), png()).unwrap();
        std::fs::write(
            home.path().join("场景正文.txt"),
            "比例：3:4 的竖构图\n保留商品外观",
        )
        .unwrap();
        let manifest = home.path().join("场景任务.json");
        std::fs::write(
            &manifest,
            serde_json::json!({
                "defaults": { "ratio": "3:4", "format": "jpg" },
                "items": [
                    {"name":"桌面正面", "sku":"55", "style":"台历桌面",
                     "promptFile":"场景正文.txt", "ref":"商品实拍.png"},
                    {"name":"桌面侧面", "sku":"55", "style":"台历桌面",
                     "promptFile":"场景正文.txt", "ref":"商品实拍.png", "ratio":"9:16"}
                ]
            })
            .to_string(),
        )
        .unwrap();
        let cli = Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/gendesk.mjs");
        // CLI 预检要调真正的应用二进制（target/<profile>/gendesk）。cargo 只在同一次构建里有
        // 集成测试时才产出它：整套 `cargo test` 会有；单跑 `--lib` 须加 `--test intake_preflight`
        // （`--bins` 只编 bin 的单测壳，不产出它；CI 覆盖率闸门即按此跑）。
        // 缺了就在这里说清楚，别让 CLI 报一个看不出原因的 ENOENT。
        let app_bin = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join(format!("gendesk{}", std::env::consts::EXE_SUFFIX));
        assert!(
            app_bin.exists(),
            "缺少应用二进制 {}：单跑 lib 测试时请加 --test intake_preflight",
            app_bin.display()
        );
        let root = home.path().join("GenDesk交接");
        std::fs::create_dir_all(&root).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        std::fs::write(
            root.join("连接.json"),
            serde_json::json!({
                "version": 2, "protocol": 2, "token": "t",
                "port": listener.local_addr().unwrap().port(),
                "appPath": app_bin, "root": root, "taskThreshold": 100,
            })
            .to_string(),
        )
        .unwrap();

        // 跑一条 CLI 命令，同时在本任务里逐个应答它的请求（CLI 串行发请求）。
        let run = |args: Vec<String>| {
            let cli = cli.clone();
            let home = home.path().to_path_buf();
            tokio::task::spawn_blocking(move || {
                std::process::Command::new("node")
                    .env("HOME", home)
                    .env("GENDESK_LAUNCH", "0")
                    .arg(cli)
                    .args(args)
                    .output()
                    .unwrap()
            })
        };
        async fn serve_until(
            listener: &tokio::net::TcpListener,
            ctx: &Ctx,
            root: &Path,
            mut child: tokio::task::JoinHandle<std::process::Output>,
        ) -> std::process::Output {
            loop {
                tokio::select! {
                    out = &mut child => return out.unwrap(),
                    conn = listener.accept() => {
                        let (stream, _) = conn.unwrap();
                        let (read, mut write) = stream.into_split();
                        let mut line = String::new();
                        BufReader::new(read).read_line(&mut line).await.unwrap();
                        let req: serde_json::Value = serde_json::from_str(&line).unwrap();
                        assert_eq!(req["token"], "t");
                        let args = &req["args"];
                        let data = match req["cmd"].as_str().unwrap() {
                            "hello" => serde_json::json!({ "protocol": 2, "root": root }),
                            "intake.status" => serde_json::json!({
                                "job": repo::get_by_job_id(&ctx.pool, args["jobId"].as_str().unwrap()).await.unwrap()
                            }),
                            "intake.submit" => serde_json::json!({
                                "job": submit(ctx, root, args["dir"].as_str().unwrap()).await.unwrap()
                            }),
                            other => serde_json::json!({ "unexpected": other }),
                        };
                        let body = serde_json::json!({ "ok": true, "data": data });
                        write.write_all(format!("{body}\n").as_bytes()).await.unwrap();
                        write.shutdown().await.unwrap();
                    }
                }
            }
        }

        let output = serve_until(
            &listener,
            &ctx,
            &root,
            run(vec![
                "submit".into(),
                "--manifest".into(),
                manifest.to_string_lossy().into_owned(),
            ]),
        )
        .await;
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let submitted: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(submitted["status"], "ingested", "{submitted}");
        let job_id = submitted["jobId"].as_str().unwrap().to_string();
        let job = repo::get_by_job_id(&pool, &job_id).await.unwrap().unwrap();
        assert_eq!(job.status, "done", "{}", job.message);
        assert_eq!(job.task_count, 2);
        assert_eq!(job.ref_count, 1);
        assert_eq!(kick.0.load(Ordering::SeqCst), 1);
        assert_eq!(count(&pool, "tasks").await, 2);
        assert!(job.wire_json[0].contains("1536x2048"));
        assert!(job.wire_json[1].contains("1152x2048"));
        assert!(job.wire_json.iter().all(|s| s.contains("jpeg")));
        assert!(job
            .wire_json
            .iter()
            .all(|s| s.contains("\"quality\":\"high\"")));

        let status = serve_until(
            &listener,
            &ctx,
            &root,
            run(vec!["status".into(), job_id.clone()]),
        )
        .await;
        assert!(status.status.success());
        let receipt: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
        assert_eq!(receipt["status"], "ingested");
        assert_eq!(receipt["job"]["batchIds"][0], job.batch_ids[0]);
        assert_eq!(receipt["job"]["taskCount"], 2);
    }

    #[tokio::test]
    async fn external_confirmation_without_db_access_runs_once_and_exports_json() {
        let (pool, _d) = test_pool().await;
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let (ctx, kick) = ctx_for(&pool, data.path(), 1);
        let dir = make_job(
            home.path(),
            "external-confirm",
            "",
            &[
                ("桌面正面", "正文一", "a.png", ""),
                ("桌面侧面", "正文二", "a.png", ""),
            ],
        );
        let held = scan(&ctx, home.path()).await.unwrap();
        assert_eq!(held[0].status, "hold");
        let receipt: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join(super::super::RECEIPT_FILE)).unwrap())
                .unwrap();
        assert_eq!(receipt["version"], 1);
        assert_eq!(receipt["job"]["status"], "hold");
        assert_eq!(count(&pool, "tasks").await, 0);

        // 外部 harness 只写文件，不能像 UI 命令那样删除台账行。
        std::fs::write(dir.join(super::super::CONFIRM_FILE), "confirmed").unwrap();
        let jobs = scan(&ctx, home.path()).await.unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].status, "done");
        assert_eq!(jobs[0].task_count, 2);
        assert_eq!(kick.0.load(Ordering::SeqCst), 1);
        assert!(scan(&ctx, home.path()).await.unwrap().is_empty());
        assert_eq!(count(&pool, "tasks").await, 2);
        let archived = std::fs::read_dir(super::super::consumed_dir(home.path()))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let receipt: serde_json::Value = serde_json::from_slice(
            &std::fs::read(archived.join(super::super::RECEIPT_FILE)).unwrap(),
        )
        .unwrap();
        assert_eq!(receipt["job"]["status"], "done");
        assert_eq!(receipt["job"]["taskCount"], 2);
        assert_eq!(receipt["job"]["batchIds"][0], jobs[0].batch_ids[0]);
        assert!(!archived.join("回执.json.pending").exists());
    }

    #[tokio::test]
    async fn confirmation_file_never_replays_an_error_or_another_directory() {
        let (pool, _d) = test_pool().await;
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let (ctx, kick) = ctx_for(&pool, data.path(), 500);
        let dir = make_job(home.path(), "external-error", "", &simple("桌面正面"));
        let id = repo::insert_running(&pool, "external-error", "external-error")
            .await
            .unwrap();
        repo::mark_error(&pool, id, "部分导入失败", &Applied::default())
            .await
            .unwrap();
        std::fs::write(dir.join(super::super::CONFIRM_FILE), "confirmed").unwrap();
        assert!(scan(&ctx, home.path()).await.unwrap().is_empty());
        assert_eq!(kick.0.load(Ordering::SeqCst), 0);
        assert_eq!(count(&pool, "tasks").await, 0);
        let receipt: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join(super::super::RECEIPT_FILE)).unwrap())
                .unwrap();
        assert_eq!(receipt["job"]["status"], "duplicate");
        assert_eq!(repo::get(&pool, id).await.unwrap().status, "error");

        let other = make_job(
            home.path(),
            "another-directory",
            r#", "jobId":"held-elsewhere""#,
            &simple("桌面远景"),
        );
        let held = repo::insert_running(&pool, "held-elsewhere", "original-directory")
            .await
            .unwrap();
        repo::mark_hold(&pool, held, "等待确认", 1, 1)
            .await
            .unwrap();
        std::fs::write(other.join(super::super::CONFIRM_FILE), "confirmed").unwrap();
        assert!(scan(&ctx, home.path()).await.unwrap().is_empty());
        assert_eq!(repo::get(&pool, held).await.unwrap().status, "hold");
        assert_eq!(count(&pool, "tasks").await, 0);
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
            r#","purpose":"i2v","defaults":{"ratio":"9:16"}"#,
            &[
                ("W-王一博-55-正面", "把卡套放进照片里，自然光", "黄.png", ""),
                ("W-王一博-55-侧面", "侧逆光，浅景深", "黄.png", ""),
            ],
        );

        let jobs = scan(&ctx, home.path()).await.unwrap();
        assert_eq!(jobs.len(), 1);
        let job = &jobs[0];
        assert_eq!(job.status, "done", "收录应成功：{}", job.message);
        assert_eq!(job.group_count, 2, "两个条目");
        assert_eq!(job.ref_count, 1, "两条共用一张原图，只该入库一次");
        assert_eq!(job.task_count, 2, "一条 = 一个任务 = 一张图");
        assert_eq!(job.batch_ids.len(), 1);
        assert_eq!(kick.0.load(Ordering::SeqCst), 1, "建批后必须唤醒调度器");

        // 用户的核心关切：工单里写的比例真的进了参数快照。
        //
        // 读的是**任务级**那份（0045）：参数下沉到任务之后，批次级那一列对工单收录
        // 恒为空 —— 一单里可能有三种画幅，写一个「代表性的」进批次反而骗人。
        let params: Vec<String> = sqlx::query_scalar("SELECT params_json FROM tasks ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(params.len(), 2);
        for j in &params {
            let p = crate::provider::GenParams::from_json(j);
            assert_eq!(p.aspect_ratio.as_deref(), Some("9:16"));
        }

        // 条目级身份（0044）跟着提示词落库：语义名 / SKU / 风格 / 用途。
        let ids: Vec<(String, String, String, String)> =
            sqlx::query_as("SELECT item_name, sku_code, style, purpose FROM prompts ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(
            ids,
            vec![
                (
                    "W-王一博-55-正面".into(),
                    "55".into(),
                    "王一博".into(),
                    "图生视频".into()
                ),
                (
                    "W-王一博-55-侧面".into(),
                    "55".into(),
                    "王一博".into(),
                    "图生视频".into()
                ),
            ]
        );
        // 正文进库必须**逐字符**等于 txt —— 新格式的全部意义就是正文里零元信息。
        let texts: Vec<String> = sqlx::query_scalar("SELECT text FROM prompts ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(texts, vec!["把卡套放进照片里，自然光", "侧逆光，浅景深"]);
        // 用途**不再**走 tag_bindings（0045）：它是无外键的多态表，分组 id 被复用时
        // 旧绑定会把用途安到无关的组头上。载体是上面那一列。
        let tags: Vec<String> = sqlx::query_scalar(
            "SELECT t.name FROM tags t JOIN tag_bindings b ON b.tag_id = t.id
              WHERE b.entity_type = 'prompt_group'",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert!(
            !tags.contains(&crate::purpose::PURPOSE_I2V.to_string()),
            "用途不该再被绑成分组标签：{tags:?}"
        );

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

    /// 分块并行落盘之后，**参考图的入库顺序必须仍然是工单里写的顺序**，
    /// 而且每个条目必须配到自己那一张。
    ///
    /// 并行化最容易在这里静默出错——图都进去了、数量也对，只是配对错位，
    /// 而那要到验收时才看得出来。同时钉住进度回报：单调、且最终等于总数。
    #[tokio::test]
    async fn parallel_ref_ingest_preserves_order_and_reports_progress() {
        let (pool, _d) = test_pool().await;
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let (ctx, _kick, progress) = ctx_for_full(&pool, data.path(), 500);

        // 七张（> 并发块大小，保证真的跨了多个块），名字本身带顺序。
        let items: Vec<(String, String, String, String)> = (1..=7)
            .map(|i| {
                (
                    format!("W-顺序-{i:02}"),
                    format!("正文{i}"),
                    format!("r{i}.png"),
                    String::new(),
                )
            })
            .collect();
        let item_refs: Vec<(&str, &str, &str, &str)> = items
            .iter()
            .map(|(a, b, c, d)| (a.as_str(), b.as_str(), c.as_str(), d.as_str()))
            .collect();
        make_job(home.path(), "20260727-顺序", "", &item_refs);

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

        // 配对正确性：第 i 条词挂的必须是第 i 张图。数量对而配对错是这条链路上
        // 最贵的静默失败 —— 整批图配错提示词，要到验收时才看得出来。
        let pairs: Vec<(String, String)> = sqlx::query_as(
            "SELECT p.item_name, r.name FROM tasks t
             JOIN prompts p ON p.id = t.prompt_id
             JOIN ref_images r ON r.id = t.ref_image_id ORDER BY t.id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        let want_pairs: Vec<(String, String)> = (1..=7)
            .map(|i| (format!("W-顺序-{i:02}"), format!("r{i}")))
            .collect();
        assert_eq!(pairs, want_pairs, "条目与参考图必须一一对上");

        let seen = progress.0.lock().unwrap().clone();
        assert!(!seen.is_empty(), "必须有进度回报");
        assert!(
            seen.windows(2).all(|w| w[0].0 < w[1].0),
            "进度必须单调递增：{seen:?}"
        );
        assert_eq!(seen.last(), Some(&(7, 7)), "最后一条应是 7/7");
    }

    /// 同一份工单被两个 CLI 进程并发提交。结果必须是**恰好一个批次**，
    /// 且**两轮都不报错**：输的那一轮该安静地跳过，而不是把 UNIQUE 冲突当成
    /// 「收录失败」抛给用户（一份其实跑得好好的工单显示确认失败）。
    #[tokio::test]
    async fn concurrent_scans_ingest_exactly_once_without_error() {
        let (pool, _d) = test_pool().await;
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let (ctx, kick) = ctx_for(&pool, data.path(), 500);

        make_job(home.path(), "20260727-并发", "", &simple("W-并发-01"));

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

        make_job(home.path(), "job-retry", "", &simple("W-重试-01"));
        let old_id = repo::insert_running(&pool, "job-retry", "job-retry")
            .await
            .unwrap();
        // New interrupted jobs own a durable checkpoint. Legacy running rows without one
        // cannot prove zero imports and are deliberately covered by the legacy refusal test.
        let mut tx = pool.begin().await.unwrap();
        repo::save_checkpoint(
            &mut tx,
            "job-retry",
            &serde_json::to_string(&Checkpoint::default()).unwrap(),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();

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

    /// 一份工单里各组画幅不同 → **恰好一个批次**（0045）。
    ///
    /// 这条替代了原来那条 `different_ratios_split_into_separate_batches`（「两种比例
    /// 必须是两个批次」）。它的前提是「params_json 存在 batches 上」，而那个前提正是
    /// 本次改动移除的东西 —— 不是为了让 CI 变绿而放宽断言：这里守住的是同一件事的
    /// 新形态，**而且更严**：不仅要一个批次，每条任务还必须拿到自己那一份比例，
    /// 没有互相串味。参数错配是这条链路上唯一会花错钱的失败。
    #[tokio::test]
    async fn one_job_is_one_run_even_with_mixed_params() {
        let (pool, _d) = test_pool().await;
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let (ctx, _) = ctx_for(&pool, data.path(), 500);

        make_job(
            home.path(),
            "job-split",
            "",
            &[
                ("W-黄-01", "黄1", "黄.png", r#","ratio":"3:4""#),
                ("W-黄-02", "黄2", "黄.png", r#","ratio":"3:4""#),
                ("W-蓝-01", "蓝1", "蓝.png", r#","ratio":"9:16""#),
                ("W-绿-01", "绿1", "绿.png", r#","ratio":"1:1""#),
            ],
        );

        let jobs = scan(&ctx, home.path()).await.unwrap();
        assert_eq!(jobs[0].status, "done", "{}", jobs[0].message);
        assert_eq!(jobs[0].batch_ids.len(), 1, "三种画幅仍然只该有一个 run");
        assert_eq!(jobs[0].task_count, 4, "四个条目 = 四个任务");
        assert_eq!(count(&pool, "batches").await, 1);

        // 每条任务拿到的是自己那一份比例。
        let raw: Vec<String> = sqlx::query_scalar("SELECT params_json FROM tasks")
            .fetch_all(&pool)
            .await
            .unwrap();
        let mut ratios: Vec<String> = raw
            .iter()
            .map(|j| {
                crate::provider::GenParams::from_json(j)
                    .aspect_ratio
                    .unwrap_or_default()
            })
            .collect();
        ratios.sort();
        assert_eq!(ratios, vec!["1:1", "3:4", "3:4", "9:16"]);

        // 批次级那一列对工单收录恒为空 —— 任务级优先、批次级兜底的读取顺序才成立。
        let batch_params: String =
            sqlx::query_scalar("SELECT params_json FROM batches WHERE id = ?1")
                .bind(jobs[0].batch_ids[0])
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(batch_params, "{}");

        // 台账仍要答得出「这一单实际发出去过几套参数」。
        assert_eq!(jobs[0].params_json.len(), 3, "三套参数都要留痕");
    }

    /// 用途是**整单级**且必须显式写：没写就是没有，GenDesk 不靠组名关键词预猜。
    ///
    /// 预猜（`purpose::infer_purposes`）是给**手工导入**的兜底 —— 那条路上人没有地方
    /// 表态。工单里作者已经表过态了，再去猜组名只会把「这批图会不会自动进视频流水线」
    /// 变成一件说不清的事。
    #[tokio::test]
    async fn purpose_must_be_declared_not_guessed_from_the_name() {
        let (pool, _d) = test_pool().await;
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let (ctx, _) = ctx_for(&pool, data.path(), 500);
        // 单名里全是会命中 i2v 预猜的关键词（B-Roll / 分镜 / 首帧），但工单没写 purpose。
        make_job(home.path(), "260804-B-Roll分镜首帧", "", &simple("W-甲-01"));

        let jobs = scan(&ctx, home.path()).await.unwrap();
        assert_eq!(jobs[0].status, "done", "{}", jobs[0].message);
        let purpose: String = sqlx::query_scalar("SELECT purpose FROM prompts")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(purpose, "", "没写就是没有 —— 不得从名字里猜出一个用途");
    }

    /// 未知用途一律拒单，**不能静默当成「没有用途」**：那会让整批本该进视频流水线的图
    /// 停在作品库里，而没有任何一处会说为什么。
    #[tokio::test]
    async fn an_unknown_purpose_rejects_the_whole_order() {
        let (pool, _d) = test_pool().await;
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let (ctx, kick) = ctx_for(&pool, data.path(), 500);
        make_job(
            home.path(),
            "job-purpose",
            r#","purpose":"图转视频""#,
            &simple("W-甲-01"),
        );

        let jobs = scan(&ctx, home.path()).await.unwrap();
        assert_eq!(jobs[0].status, "error");
        assert!(jobs[0].message.contains("未知用途"), "{}", jobs[0].message);
        assert_eq!(kick.0.load(Ordering::SeqCst), 0);
        for t in ["batches", "prompts", "ref_images", "tasks"] {
            assert_eq!(count(&pool, t).await, 0, "{t} 不该有任何记录");
        }
    }

    // 参数完全一样时，台账里只该留**一套**参数的痕迹，不该逐条重复。
    #[tokio::test]
    async fn identical_params_are_recorded_once() {
        let (pool, _d) = test_pool().await;
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let (ctx, _) = ctx_for(&pool, data.path(), 500);
        make_job(
            home.path(),
            "job-merge",
            r#","defaults":{"ratio":"3:4"}"#,
            &[
                ("W-甲-01", "甲1", "a.png", ""),
                ("W-乙-01", "乙1", "b.png", ""),
            ],
        );
        let jobs = scan(&ctx, home.path()).await.unwrap();
        assert_eq!(jobs[0].status, "done", "{}", jobs[0].message);
        assert_eq!(jobs[0].batch_ids.len(), 1);
        assert_eq!(jobs[0].group_count, 2, "两个条目");
        assert_eq!(jobs[0].params_json.len(), 1, "同一套参数只留一份痕迹");
    }

    // 重复扫描不得重复建批 —— 这是整个模块最贵的一条错误（重复花钱）。
    #[tokio::test]
    async fn rescan_does_not_create_a_second_batch() {
        let (pool, _d) = test_pool().await;
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let (ctx, _) = ctx_for(&pool, data.path(), 500);
        make_job(home.path(), "job-1", "", &simple("W-甲-01"));

        assert_eq!(scan(&ctx, home.path()).await.unwrap().len(), 1);
        // 同一份工单原样再投一次（同目录名 = 同 jobId）：目录在，但台账已记，必须跳过。
        make_job(home.path(), "job-1", "", &simple("W-甲-01"));
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
        make_job(home.path(), "job-dup", "", &simple("W-甲-01"));
        assert_eq!(scan(&ctx, home.path()).await.unwrap().len(), 1);

        // 新内容套了个用过的名字。
        let dir = make_job(home.path(), "job-dup", "", &simple("W-乙-01"));
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
        make_job(home.path(), "job-kept", "", &simple("W-甲-01"));
        assert_eq!(scan(&ctx, home.path()).await.unwrap().len(), 1);

        let dir = make_job(home.path(), "job-kept", "", &simple("W-甲-01"));
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
        let dir = make_job(home.path(), "job-2", "", &simple("W-甲-01"));
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
            "",
            &[("W-甲-01", "甲1", "a.png", r#","size":"1080x1920""#)],
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
            "",
            &[
                ("W-甲-01", "甲1", "a.png", ""),
                ("W-甲-02", "甲2", "a.png", ""),
                ("W-甲-03", "甲3", "a.png", ""),
                ("W-甲-04", "甲4", "a.png", ""),
            ],
        );

        let jobs = scan(&ctx, home.path()).await.unwrap();
        assert_eq!(jobs[0].status, "hold");
        assert_eq!(jobs[0].task_count, 4, "4 个条目 = 4 张图");
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
            "",
            &[
                ("W-甲-01", "甲1", "a.png", ""),
                ("W-甲-02", "甲2", "a.png", ""),
                ("W-甲-03", "甲3", "a.png", ""),
                ("W-甲-04", "甲4", "a.png", ""),
            ],
        );
        let held = scan(&ctx, home.path()).await.unwrap();
        assert_eq!(held[0].status, "hold");

        // 「确认」= 删台账行 + 写确认文件（`gendesk confirm` 做的就是这两件事）。
        repo::delete(&pool, held[0].id).await.unwrap();
        std::fs::write(dir.join(super::super::CONFIRM_FILE), "ok").unwrap();

        let jobs = scan(&ctx, home.path()).await.unwrap();
        assert_eq!(jobs[0].status, "done", "{}", jobs[0].message);
        assert_eq!(jobs[0].task_count, 4);
        assert_eq!(kick.0.load(Ordering::SeqCst), 1);
    }

    /// 测试用 Poster 检查：记下被问到的款式，并把它们都报成对不上。
    #[derive(Default)]
    struct RejectingPoster(std::sync::Mutex<Vec<(String, String)>>);

    #[async_trait::async_trait]
    impl PosterGate for RejectingPoster {
        async fn unresolved(&self, keys: &[(String, String)]) -> Vec<String> {
            self.0.lock().unwrap().extend_from_slice(keys);
            keys.iter()
                .map(|(sku, style)| format!("「{sku} / {style}」尚未绑定"))
                .collect()
        }
    }

    /// 测试用 Poster 检查：款式都对得上，记下被问到的显式类型并全部报成对不上。
    #[derive(Default)]
    struct RejectingTypes(std::sync::Mutex<Vec<String>>);

    #[async_trait::async_trait]
    impl PosterGate for RejectingTypes {
        async fn unresolved(&self, _keys: &[(String, String)]) -> Vec<String> {
            Vec::new()
        }
        async fn unresolved_types(&self, labels: &[String]) -> Vec<String> {
            self.0.lock().unwrap().extend_from_slice(labels);
            labels
                .iter()
                .map(|l| format!("Poster 里没有图片类型「{l}」"))
                .collect()
        }
    }

    // 显式 posterType 对不上：同样什么都不导入；只问显式写的，按需求名的不拦。
    #[tokio::test]
    async fn unknown_explicit_poster_type_holds_without_importing() {
        let (pool, _d) = test_pool().await;
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let (mut ctx, kick) = ctx_for(&pool, data.path(), 100);
        let gate = Arc::new(RejectingTypes::default());
        ctx.poster = gate.clone();
        make_job(
            home.path(),
            "job-type",
            "",
            &[
                (
                    "侧面",
                    "正文一",
                    "a.png",
                    r#","organization":{"product":"2027明星日历","style":"王一博","requirement":"侧面图","posterType":"侧视图"}"#,
                ),
                (
                    "正面",
                    "正文二",
                    "a.png",
                    r#","organization":{"product":"2027明星日历","style":"王一博","requirement":"正视图"}"#,
                ),
            ],
        );
        let held = scan(&ctx, home.path()).await.unwrap();
        assert_eq!(held[0].status, "hold", "{}", held[0].message);
        assert!(
            held[0].message.contains("Poster 图片类型对不上") && held[0].message.contains("侧视图"),
            "{}",
            held[0].message
        );
        assert_eq!(*gate.0.lock().unwrap(), vec!["侧视图".to_string()]);
        assert_eq!(count(&pool, "prompts").await, 0);
        assert_eq!(kick.0.load(Ordering::SeqCst), 0);
    }

    // Poster 款式对不上：和超阈值一样什么都不导入，确认后照常开跑。
    #[tokio::test]
    async fn unmatched_poster_style_holds_until_confirmed() {
        let (pool, _d) = test_pool().await;
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let (mut ctx, kick) = ctx_for(&pool, data.path(), 100);
        let gate = Arc::new(RejectingPoster::default());
        ctx.poster = gate.clone();
        let dir = make_job(
            home.path(),
            "job-poster",
            "",
            &[
                ("W-甲-01", "甲1", "a.png", ""),
                ("W-甲-02", "甲2", "a.png", ""),
            ],
        );

        let held = scan(&ctx, home.path()).await.unwrap();
        assert_eq!(held[0].status, "hold");
        assert!(
            held[0].message.contains("Poster 款式对不上"),
            "{}",
            held[0].message
        );
        assert!(!held[0].message.contains("超过阈值"), "{}", held[0].message);
        assert_eq!(
            *gate.0.lock().unwrap(),
            vec![("55".to_string(), "王一博".to_string())],
            "同一款式只问一次"
        );
        let text = std::fs::read_to_string(dir.join(super::super::HOLD_FILE)).unwrap();
        assert!(text.contains("gendesk poster bind"), "{text}");
        for t in ["batches", "prompts", "ref_images", "tasks"] {
            assert_eq!(count(&pool, t).await, 0, "{t} 在待确认时必须是空的");
        }

        repo::delete(&pool, held[0].id).await.unwrap();
        std::fs::write(dir.join(super::super::CONFIRM_FILE), "ok").unwrap();
        let jobs = scan(&ctx, home.path()).await.unwrap();
        assert_eq!(jobs[0].status, "done", "{}", jobs[0].message);
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
            "",
            &[
                ("W-甲-01", "甲1", "a.png", ""),
                ("W-甲-02", "甲2", "a.png", ""),
            ],
        );
        let jobs = scan(&ctx, home.path()).await.unwrap();
        assert_eq!(jobs[0].status, "done", "{}", jobs[0].message);
        assert_eq!(jobs[0].task_count, 2);
    }

    /// 外部成图导入单（`mode=import`）：成图落到草稿位置、任务**直接**是待验收，
    /// 一张都不进 q 态（调度器只取 q，故不会调接口花钱）；不受张数阈值限制。
    #[tokio::test]
    async fn imported_results_land_in_review_without_ever_being_queued() {
        let (pool, _d) = test_pool().await;
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let works = tempfile::tempdir().unwrap();
        crate::db::repo::settings::set_raw(
            &pool,
            &serde_json::json!({
                "scheduleStrategy": "round_robin", "retryCount": 1, "outputDir": works.path(),
                "motion": "standard", "paused": false, "onboarded": true,
            })
            .to_string(),
        )
        .await
        .unwrap();
        // 先确认作品根真的指到临时目录，绝不往用户主目录里写。
        assert_eq!(crate::library::root(&pool).await.unwrap(), works.path());
        // 阈值 1、两张图：生图单会 hold，导入单不花钱，照常收录。
        let (ctx, _kick) = ctx_for(&pool, data.path(), 1);
        let dir = make_job(
            home.path(),
            "codex-成图",
            r#","mode":"import","organization":{"product":"陶瓷水杯","style":"奶油白款","requirement":"厨房场景图","source":"Codex"}"#,
            &[
                (
                    "水杯晨光",
                    "晨光桌面",
                    "a.png",
                    r#","result":"results/1.png""#,
                ),
                (
                    "水杯窗边",
                    "窗边竖幅",
                    "a.png",
                    r#","result":"results/2.png""#,
                ),
            ],
        );
        std::fs::create_dir_all(dir.join("results")).unwrap();
        std::fs::write(dir.join("results/1.png"), png()).unwrap();
        std::fs::write(dir.join("results/2.png"), png()).unwrap();

        let jobs = scan(&ctx, home.path()).await.unwrap();
        assert_eq!(jobs.len(), 1);
        let job = &jobs[0];
        assert_eq!(job.status, "done", "导入应成功：{}", job.message);
        assert_eq!(job.task_count, 2);
        assert_eq!(job.message, import_message(2));

        let rows: Vec<(String, Option<String>, Option<String>, String)> = sqlx::query_as(
            "SELECT status, result_image_path, result_thumb_path, params_json FROM tasks ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(rows.len(), 2);
        for (status, image, thumb, params) in &rows {
            assert_eq!(status, "rev", "外部成图必须直接待验收");
            let image = PathBuf::from(image.as_deref().unwrap());
            assert!(image.is_file());
            assert!(
                image.starts_with(
                    works
                        .path()
                        .join("_草稿/生成图片/陶瓷水杯/奶油白款/厨房场景图")
                ),
                "成图应与生图结果同一个草稿位置：{}",
                image.display()
            );
            assert!(Path::new(thumb.as_deref().unwrap()).is_file());
            let params: serde_json::Value = serde_json::from_str(params).unwrap();
            assert_eq!(params["externalResult"], serde_json::json!(true));
        }
        let attempts = count(&pool, "task_attempts").await;
        assert_eq!(attempts, 0, "没有任何生图尝试");
        let receipt = std::fs::read_dir(super::super::consumed_dir(home.path()))
            .unwrap()
            .flatten()
            .next()
            .map(|e| std::fs::read_to_string(e.path().join(super::super::RESULT_FILE)).unwrap())
            .unwrap();
        assert!(receipt.contains("未调用生图"));

        // 同一份重投：去重挡住，不会再导入一份。
        let again = make_job(
            home.path(),
            "codex-成图-重投",
            r#","jobId":"codex-成图","mode":"import","organization":{"product":"陶瓷水杯","style":"奶油白款","requirement":"厨房场景图","source":"Codex"}"#,
            &[(
                "水杯晨光",
                "晨光桌面",
                "a.png",
                r#","result":"results/1.png""#,
            )],
        );
        std::fs::create_dir_all(again.join("results")).unwrap();
        std::fs::write(again.join("results/1.png"), png()).unwrap();
        assert!(scan(&ctx, home.path()).await.unwrap().is_empty());
        assert_eq!(count(&pool, "tasks").await, 2);
    }

    /// 导入单缺成图、生图单夹带成图、导入单按分类随机抽参考图，都整单拒收、什么都不导入。
    #[tokio::test]
    async fn result_fields_must_match_the_order_mode() {
        let (pool, _d) = test_pool().await;
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let (ctx, _kick) = ctx_for(&pool, data.path(), 100);
        make_job(
            home.path(),
            "a-导入缺图",
            r#","mode":"import""#,
            &simple("甲"),
        );
        let dir = make_job(
            home.path(),
            "b-生图带图",
            "",
            &[("乙", "正文", "a.png", r#","result":"results/1.png""#)],
        );
        std::fs::create_dir_all(dir.join("results")).unwrap();
        std::fs::write(dir.join("results/1.png"), png()).unwrap();
        make_job(
            home.path(),
            "c-未知模式",
            r#","mode":"edit""#,
            &simple("丙"),
        );
        let jobs = scan(&ctx, home.path()).await.unwrap();
        assert_eq!(jobs.len(), 3);
        assert!(jobs.iter().all(|j| j.status == "error"), "{jobs:?}");
        assert!(
            jobs[0].message.contains("没写 result"),
            "{}",
            jobs[0].message
        );
        assert!(
            jobs[1].message.contains("gendesk import"),
            "{}",
            jobs[1].message
        );
        assert!(jobs[2].message.contains("mode"), "{}", jobs[2].message);
        assert_eq!(count(&pool, "tasks").await, 0);
        assert_eq!(count(&pool, "prompts").await, 0);

        let spec: super::super::OrderSpec = serde_json::from_str(
            r#"{"version":1,"mode":"import","items":[{"name":"丁","sku":"55","style":"王一博",
                "ref":{"category":"正面图"},"result":"results/1.png"}]}"#,
        )
        .unwrap();
        let err = super::super::plan_from_spec(home.path(), "d", &spec).unwrap_err();
        assert!(err.contains("不能按分类随机抽"), "{err}");
    }

    // 条目没点名挂靠 → 拒单，不猜。挂靠现在是**条目级、每条必填**：
    // 按顺序猜错的代价是整批图配错提示词，而那要到验收时才看得出来。
    #[tokio::test]
    async fn an_item_without_a_ref_is_rejected_end_to_end() {
        let (pool, _d) = test_pool().await;
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let (ctx, kick) = ctx_for(&pool, data.path(), 500);
        let dir = super::super::pending_dir(home.path()).join("job-4");
        std::fs::create_dir_all(dir.join(super::super::PROMPTS)).unwrap();
        std::fs::create_dir_all(dir.join(super::super::REFS)).unwrap();
        std::fs::write(dir.join(super::super::PROMPTS).join("A.txt"), "甲1").unwrap();
        std::fs::write(dir.join(super::super::REFS).join("a.png"), png()).unwrap();
        std::fs::write(
            dir.join(super::super::ORDER),
            r#"{"version":1,"items":[{"name":"A","sku":"55","style":"王一博"}]}"#,
        )
        .unwrap();
        std::fs::write(dir.join(super::super::READY), "ok").unwrap();

        let jobs = scan(&ctx, home.path()).await.unwrap();
        assert_eq!(jobs[0].status, "error");
        assert!(
            jobs[0].message.contains("必须点名参考图"),
            "{}",
            jobs[0].message
        );
        assert_eq!(kick.0.load(Ordering::SeqCst), 0);
        for t in ["batches", "prompts", "ref_images", "tasks"] {
            assert_eq!(count(&pool, t).await, 0, "{t} 不该有任何记录");
        }
    }

    // 旧格式（提示词.txt + 组头键）已下线：必须**明确点名**，而不是笼统地报
    // 「找不到工单」—— 存量目录会撞上这条，而人的第一反应是「我明明放了提示词文档」。
    #[tokio::test]
    async fn a_legacy_job_fails_with_a_pointed_message() {
        let (pool, _d) = test_pool().await;
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let (ctx, _) = ctx_for(&pool, data.path(), 500);
        let dir = super::super::pending_dir(home.path()).join("job-legacy");
        std::fs::create_dir_all(dir.join("images")).unwrap();
        std::fs::write(
            dir.join("提示词.txt"),
            "分组: 甲\n参考图: images/a.png\n\n甲1\n",
        )
        .unwrap();
        std::fs::write(dir.join("images").join("a.png"), png()).unwrap();
        std::fs::write(dir.join(super::super::READY), "ok").unwrap();

        let jobs = scan(&ctx, home.path()).await.unwrap();
        assert_eq!(jobs[0].status, "error");
        assert!(jobs[0].message.contains("旧格式"), "{}", jobs[0].message);
        assert!(dir.join(super::super::ERROR_FILE).is_file(), "要留下回执");
    }

    #[tokio::test]
    async fn corrupt_reference_is_rejected_before_import_and_can_be_fixed_without_changing_job_id()
    {
        let (pool, _db) = test_pool().await;
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let (ctx, kick) = ctx_for(&pool, data.path(), 100);
        let dir = make_job(home.path(), "bad-image", "", &simple("正文样本"));
        std::fs::write(dir.join("refs/a.png"), b"broken image").unwrap();
        let failed = scan(&ctx, home.path()).await.unwrap().remove(0);
        assert_eq!(failed.status, "error");
        assert!(failed.message.contains("解码失败"));
        for table in ["prompts", "ref_images", "tasks", "batches"] {
            assert_eq!(count(&pool, table).await, 0);
        }
        std::fs::write(dir.join("refs/a.png"), png()).unwrap();
        let resumed = retry_job(&ctx, home.path(), failed.id)
            .await
            .unwrap()
            .remove(0);
        assert_eq!(resumed.id, failed.id);
        assert_eq!(resumed.status, "done", "{}", resumed.message);
        assert_eq!(count(&pool, "prompts").await, 1);
        assert_eq!(count(&pool, "ref_images").await, 1);
        assert_eq!(kick.0.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn partial_import_resumes_same_prompt_and_reference_ids_after_restart_and_concurrent_retry(
    ) {
        let (pool, _db) = test_pool().await;
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let (ctx, kick) = ctx_for(&pool, data.path(), 100);
        let dir = make_job(
            home.path(),
            "partial-resume",
            "",
            &[
                ("第一场景", "第一正文", "a.png", ""),
                ("第二场景", "第二正文", "b.png", ""),
            ],
        );
        sqlx::query("CREATE TRIGGER fail_second_reference BEFORE INSERT ON ref_images WHEN NEW.name='b' BEGIN SELECT RAISE(FAIL, 'injected reference failure'); END")
            .execute(&pool).await.unwrap();
        let failed = scan(&ctx, home.path()).await.unwrap().remove(0);
        assert_eq!(failed.status, "error");
        assert_eq!(failed.group_count, 2);
        assert_eq!(failed.ref_count, 1);
        assert_eq!(count(&pool, "batches").await, 0);
        assert_eq!(kick.0.load(Ordering::SeqCst), 0);
        let prompts: Vec<i64> = sqlx::query_scalar("SELECT id FROM prompts ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
        let reference: i64 = sqlx::query_scalar("SELECT id FROM ref_images")
            .fetch_one(&pool)
            .await
            .unwrap();
        // Changing content after a committed stage must preserve the original imported rows.
        std::fs::write(dir.join("prompts/第一场景.txt"), "被修改正文").unwrap();
        let refused = retry_job(&ctx, home.path(), failed.id)
            .await
            .unwrap()
            .remove(0);
        assert_eq!(refused.status, "error");
        assert!(refused.message.contains("已改变"));
        assert_eq!(count(&pool, "prompts").await, 2);
        assert_eq!(count(&pool, "ref_images").await, 1);
        std::fs::write(dir.join("prompts/第一场景.txt"), "第一正文").unwrap();
        sqlx::query("DROP TRIGGER fail_second_reference")
            .execute(&pool)
            .await
            .unwrap();
        // Reconstruct the context: only the persisted database checkpoint survives.
        let (resumed_ctx, resumed_kick) = ctx_for(&pool, data.path(), 100);
        repo::mark_running(&pool, failed.id).await.unwrap();
        let (a, b) = tokio::join!(
            retry_job(&resumed_ctx, home.path(), failed.id),
            retry_job(&resumed_ctx, home.path(), failed.id)
        );
        let jobs: Vec<_> = a.unwrap().into_iter().chain(b.unwrap()).collect();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].id, failed.id);
        assert_eq!(jobs[0].status, "done", "{}", jobs[0].message);
        assert_eq!(count(&pool, "prompts").await, 2);
        assert_eq!(count(&pool, "ref_images").await, 2);
        assert_eq!(count(&pool, "tasks").await, 2);
        assert_eq!(count(&pool, "batches").await, 1);
        let after: Vec<i64> = sqlx::query_scalar("SELECT id FROM prompts ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(after, prompts);
        let retained: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ref_images WHERE id=?")
            .bind(reference)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(retained, 1);
        assert_eq!(resumed_kick.0.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn checkpoint_failure_rolls_back_imported_rows_and_final_receipt_failure_rolls_back_batch(
    ) {
        for stage in ["prompts", "refs", "batch"] {
            let (pool, _db) = test_pool().await;
            let home = tempfile::tempdir().unwrap();
            let data = tempfile::tempdir().unwrap();
            let (ctx, kick) = ctx_for(&pool, data.path(), 100);
            make_job(home.path(), "atomic-stage", "", &simple("场景样本"));
            let trigger = match stage {
                "prompts" => "CREATE TRIGGER fail_stage BEFORE UPDATE ON intake_checkpoints WHEN json_array_length(json_extract(NEW.state_json,'$.prompt_ids'))>0 BEGIN SELECT RAISE(FAIL,'checkpoint failure'); END",
                "refs" => "CREATE TRIGGER fail_stage BEFORE UPDATE ON intake_checkpoints WHEN json_array_length(json_extract(NEW.state_json,'$.refs'))>0 BEGIN SELECT RAISE(FAIL,'checkpoint failure'); END",
                _ => "CREATE TRIGGER fail_stage BEFORE UPDATE ON intake_jobs WHEN NEW.task_count>0 BEGIN SELECT RAISE(FAIL,'receipt failure'); END",
            };
            sqlx::query(trigger).execute(&pool).await.unwrap();
            let failed = scan(&ctx, home.path()).await.unwrap().remove(0);
            assert_eq!(failed.status, "error", "{stage}");
            assert_eq!(count(&pool, "batches").await, 0, "{stage}");
            assert_eq!(count(&pool, "tasks").await, 0, "{stage}");
            assert_eq!(
                count(&pool, "prompts").await,
                if stage == "prompts" { 0 } else { 1 }
            );
            assert_eq!(
                count(&pool, "ref_images").await,
                if stage == "batch" { 1 } else { 0 }
            );
            assert_eq!(kick.0.load(Ordering::SeqCst), 0);
            sqlx::query("DROP TRIGGER fail_stage")
                .execute(&pool)
                .await
                .unwrap();
            let jobs = retry_job(&ctx, home.path(), failed.id).await.unwrap();
            assert_eq!(jobs[0].status, "done", "{stage}: {}", jobs[0].message);
            for table in ["prompts", "ref_images", "tasks", "batches"] {
                assert_eq!(count(&pool, table).await, 1, "{stage}: {table}");
            }
            assert_eq!(kick.0.load(Ordering::SeqCst), 1);
        }
    }

    #[tokio::test]
    async fn legacy_partial_or_ambiguous_running_jobs_are_not_replayed() {
        let (pool, _db) = test_pool().await;
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let (ctx, kick) = ctx_for(&pool, data.path(), 100);
        make_job(home.path(), "legacy-partial", "", &simple("旧场景"));
        let id = repo::insert_running(&pool, "legacy-partial", "legacy-partial")
            .await
            .unwrap();
        assert!(retry_job(&ctx, home.path(), id)
            .await
            .unwrap_err()
            .to_string()
            .contains("没有恢复检查点"));
        repo::mark_error(
            &pool,
            id,
            "old partial failure",
            &Applied {
                group_count: 1,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(retry_job(&ctx, home.path(), id).await.is_err());
        assert_eq!(count(&pool, "intake_jobs").await, 1);
        assert_eq!(count(&pool, "prompts").await, 0);
        assert_eq!(count(&pool, "tasks").await, 0);
        assert_eq!(kick.0.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn validation_failure_preserves_explicit_identity_and_repaired_json_can_bind_an_empty_job(
    ) {
        for broken_json in [false, true] {
            let (pool, _db) = test_pool().await;
            let home = tempfile::tempdir().unwrap();
            let data = tempfile::tempdir().unwrap();
            let (ctx, _) = ctx_for(&pool, data.path(), 100);
            let dir = make_job(
                home.path(),
                "package-directory",
                ",\"jobId\":\"stable-job\",\"defaults\":{\"ratio\":\"bad\"}",
                &simple("场景样本"),
            );
            let file = dir.join(super::super::ORDER);
            let mut spec: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
            if broken_json {
                std::fs::write(&file, "{broken").unwrap();
            }
            let failed = scan(&ctx, home.path()).await.unwrap().remove(0);
            assert_eq!(failed.status, "error");
            assert_eq!(count(&pool, "prompts").await, 0);
            if !broken_json {
                assert_eq!(failed.job_id, "stable-job");
            }
            spec["defaults"]["ratio"] = "3:4".into();
            std::fs::write(&file, spec.to_string()).unwrap();
            let resumed = retry_job(&ctx, home.path(), failed.id)
                .await
                .unwrap()
                .remove(0);
            assert_eq!(resumed.id, failed.id);
            assert_eq!(resumed.job_id, "stable-job");
            assert_eq!(resumed.status, "done", "{}", resumed.message);
            assert_eq!(count(&pool, "intake_jobs").await, 1);
            assert_eq!(count(&pool, "prompts").await, 1);
            assert_eq!(count(&pool, "batches").await, 1);
        }
    }

    #[tokio::test]
    async fn a_committed_job_repairs_a_failed_receipt_write_without_reimporting() {
        let (pool, _db) = test_pool().await;
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let (ctx, _) = ctx_for(&pool, data.path(), 100);
        let dir = make_job(home.path(), "receipt-recovery", "", &simple("场景样本"));
        // A filesystem fault, after the DB transaction, must leave the package discoverable.
        std::fs::create_dir(dir.join("回执.json.pending")).unwrap();
        let completed = scan(&ctx, home.path()).await.unwrap().remove(0);
        assert_eq!(completed.status, "done");
        assert!(dir.is_dir());
        assert_eq!(count(&pool, "batches").await, 1);
        std::fs::remove_dir(dir.join("回执.json.pending")).unwrap();
        assert!(scan(&ctx, home.path()).await.unwrap().is_empty());
        assert!(!dir.exists());
        let archived = std::fs::read_dir(super::super::consumed_dir(home.path()))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let receipt: serde_json::Value = serde_json::from_slice(
            &std::fs::read(archived.join(super::super::RECEIPT_FILE)).unwrap(),
        )
        .unwrap();
        assert_eq!(receipt["job"]["status"], "done");
        assert_eq!(receipt["job"]["id"], completed.id);
        for table in ["prompts", "ref_images", "batches", "tasks"] {
            assert_eq!(count(&pool, table).await, 1, "{table}");
        }
    }
}
