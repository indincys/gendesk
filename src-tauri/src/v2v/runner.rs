//! 视频流水线执行器：提交 → 轮询 → 落盘 → 待验收。
//!
//! 与图片引擎（`engine::dispatcher`）同构但独立：视频任务不吃 API Key 信号量，
//! 也没有 7 态重试策略，硬塞进那条单循环只会给一条成熟链路引入回归风险。
//! 复用的是**模式**（状态在库、崩溃恢复、事件驱动、spawn_blocking 隔离阻塞活），
//! 不是代码。
//!
//! ## 额度是一次性的，所以状态迁移的顺序不能反
//!
//! 提交成功 → 立刻写 submit_id 并置 run。反过来（先置 run 再写 id）会留下
//! 「跑着但认不出是哪条」的孤儿，而 `recover_orphan_submits` 只能把它退回 ready
//! 让人重提——那就是花两份钱买同一条视频。

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use sqlx::SqlitePool;

use crate::db::now_unix;
use crate::db::repo::v2v as repo;
use crate::error::AppResult;
use crate::files::DataDirs;

use super::activity::Activity;
use super::dreamina::{self, GenOpts, Outcome};
use super::events::StageCounts;

/// 循环的心跳节拍。**不等于查询频率**：心跳快是为了界面上那句「12 秒前」跟得上。
pub const TICK: Duration = Duration::from_secs(6);

/// 整表扫描的间隔（秒）。**常数，不再随等待时长递增**。
///
/// ## 为什么退避轮询被换掉了
///
/// 退避（`poll_interval_for`）解决的是「19 条在跑就要起 19 个 `query_result` 进程」，
/// 它把频率压下去换成本 —— 代价是**出片延迟**：等满一小时后十分钟才问一次，一条
/// 早就跑完的片子要躺到十分钟后才被发现。那是拿延迟换成本，两头都不满意。
///
/// 真正的解法是换掉「问一次」的单位：`dreamina list_task` **一个进程就回一整页**
/// 全部在跑任务的状态。于是进程数与在跑条数**脱钩**（O(1) 而非 O(n)），频率反而
/// 可以调高。19 条过夜 8 小时：原始每 6 秒逐条 = 九万次；退避 ≈ 1140 次；
/// 30 秒整表扫描 = **960 次，且与条数无关**（100 条在跑仍是 960 次），
/// 同时出片延迟从「最长 10 分钟」降到「最长 30 秒」。成本与延迟同时变好。
///
/// CLI 侧没有任何推送机制（无 watch / stream / webhook；`--poll=N` 只是把 1 秒一次的
/// 轮询搬进子进程，进程被杀即丢），所以「事件驱动不轮询」在这条链路上做不到 ——
/// 能做的就是把轮询的单位从「一条」换成「一整页」。
pub const SWEEP_SECS: i64 = 30;

/// 上一次整表扫描的时刻（进程内）。
///
/// 队列面板要显示「下次查询还有几秒」，而那个数字现在由扫描节拍决定，不再由每条
/// 各自的退避决定 —— 与其让命令层照着公式重算一遍（必然与实际跑的循环分叉），
/// 不如让循环自己记下来。
static LAST_SWEEP: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

/// 距下一次整表扫描还有多少秒（`None` = 还没跑过第一轮）。
pub fn next_sweep_in(now: i64) -> Option<i64> {
    let last = LAST_SWEEP.load(std::sync::atomic::Ordering::Relaxed);
    (last > 0).then(|| (last + SWEEP_SECS - now).max(0))
}

/// 单条 clip 距上次查询要隔多久才再查一次（秒），按它已经等了多久递增。
///
/// **只用于回落路径**：整表扫描里认不出的 submit_id（翻页没覆盖到、CLI 输出变了）
/// 仍走逐条 `query_result`，那时退避照旧生效。主路径已经不需要它了，
/// 但这条路必须留着 —— 认不出的条目恰恰是最不该被放弃轮询的那些。
pub fn poll_interval_for(age_secs: i64) -> i64 {
    match age_secs {
        a if a < 120 => 10,       // 刚提交：想尽快确认进没进队列
        a if a < 600 => 30,       // 前 10 分钟：短任务可能已经出片
        a if a < 3600 => 120,     // 一小时内：两分钟一次
        a if a < 6 * 3600 => 300, // 数小时：五分钟一次
        _ => 600,                 // 过夜：十分钟一次足够
    }
}

/// 这一条到点该查了吗。
///
/// 从没查过的一律立刻查（刚提交的那一刻就想知道进没进队列）。
pub fn is_due(submitted_at: Option<i64>, polled_at: Option<i64>, now: i64) -> bool {
    let Some(last) = polled_at else {
        return true;
    };
    let age = submitted_at.map_or(0, |t| (now - t).max(0));
    now - last >= poll_interval_for(age)
}

/// 超时兜底的默认值：**不限**。
///
/// 原来硬编码 45 分钟，实测把 19 条还在 `querying` 的任务全判死了（提交 72 分钟后
/// 即梦那边仍未结束）。判死一条在跑的任务代价是实打实的钱（额度已扣），而多等的代价
/// 只是看板上多几条「已提交」—— 两边不对等，所以默认不设上限，由人在设置里决定。
///
/// 「未知态判 Running 必须有个尽头」这条顾虑仍然成立，但它的解法不是超时：判超时后
/// `submit_id` 保留、看板给「继续等待」（`repo::resume_timed_out`），而退避轮询让
/// 一条永远卡住的任务每十分钟才问一次 —— 代价已经低到不需要用「判死」来兜底。
pub const DEFAULT_TIMEOUT_HOURS: Option<i64> = None;

/// 提交摘要。
#[derive(Debug, Clone, Default, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SubmitSummary {
    pub submitted: i64,
    pub failed: i64,
    /// 第一条失败的原因（批量提交时逐条上报太吵，给一条代表 + 计数）。
    pub first_error: Option<String>,
}

/// 每条 clip 的生成参数：改写结果里带的优先，其次设置里的默认值。
///
/// skill 对某一条给了具体建议（比如这条动势大，给 8 秒）时应当尊重它——那是看过图的判断。
pub fn opts_for(clip: &repo::ClipRow, defaults: &GenOpts) -> GenOpts {
    GenOpts {
        model_version: clip
            .model_version
            .clone()
            .or_else(|| defaults.model_version.clone()),
        duration: clip.duration.or(defaults.duration),
        video_resolution: clip
            .video_resolution
            .clone()
            .or_else(|| defaults.video_resolution.clone()),
        session: defaults.session,
    }
}

/// 成片与封面的落盘路径。
///
/// 封面**独立成文件**（clip 自己的 jpg），绝不复用 `accepted_works.thumb_path`：
/// 清空废纸篓会物理删除 file_paths 里的路径，若封面指着作品缩略图，
/// 删一条未通过的视频就会顺手删掉还活着的那张作品的缩略图，作品库整片瀑布流跟着空掉。
pub fn clip_paths(dirs: &DataDirs, clip_id: i64) -> (PathBuf, PathBuf) {
    let base = dirs.clips();
    (
        base.join(format!("clip{clip_id}.mp4")),
        base.join(format!("clip{clip_id}.jpg")),
    )
}

/// 批量提交待提交条目。顺序执行：CLI 每次要走网络，并发打过去容易撞限流，
/// 而这一步本来就是人点了「提交」之后的后台活，快几秒没有价值。
pub async fn submit_batch(
    pool: &SqlitePool,
    bin: &str,
    ids: &[i64],
    defaults: &GenOpts,
    log: &Activity,
) -> AppResult<SubmitSummary> {
    let rows = repo::take_ready(pool, ids).await?;
    let total = rows.len();
    let mut sum = SubmitSummary::default();
    if total > 0 {
        log.info("submit", None, format!("开始提交 {total} 条到即梦"), None);
    }
    for (i, clip) in rows.into_iter().enumerate() {
        let who = Some((clip.id, clip.prompt_code.as_str()));
        let Some(prompt) = clip
            .video_prompt
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            log.error("submit", who, "没有视频提示词，跳过", None);
            sum.failed += 1;
            if sum.first_error.is_none() {
                sum.first_error = Some(format!("{} 没有视频提示词", clip.prompt_code));
            }
            continue;
        };
        let opts = opts_for(&clip, defaults);
        // 逐条报「第几条 / 共几条」：批量提交要走 N 趟网络，几十秒里界面一声不吭，
        // 正是 v0.14.0 上传静默那个坑的同款手感（用户以为没点上，反复重按）。
        log.info(
            "submit",
            who,
            format!(
                "提交中 {}/{total} · 模型 {} · 时长 {} · 分辨率 {}",
                i + 1,
                opts.model_version.as_deref().unwrap_or("CLI 默认"),
                opts.duration.map_or("CLI 默认".into(), |d| format!("{d}s")),
                opts.video_resolution.as_deref().unwrap_or("CLI 默认"),
            ),
            None,
        );
        match dreamina::submit(bin, Path::new(&clip.image_path), prompt, &opts, log, who).await {
            Ok(receipt) => {
                repo::mark_submitted(pool, clip.id, &receipt, now_unix()).await?;
                if receipt.looks_healthy() {
                    let billed = receipt.credit_count.unwrap_or(0);
                    log.info(
                        "submit",
                        who,
                        format!("已提交 · {} · 计费 {billed} 额度", receipt.submit_id),
                        None,
                    );
                    // 价格表是实测出来的，即梦随时可以调价而不通知任何人。回执是唯一的
                    // 真账单，对不上就当场喊 —— 否则确认卡会一直拿着过期的数字骗人，
                    // 而下一次发现要等到月底看余额。
                    if let (Some(m), Some(r), Some(d)) = (
                        opts.model_version.as_deref(),
                        opts.video_resolution.as_deref(),
                        opts.duration,
                    ) {
                        if let Some(est) = dreamina::estimate_credits(m, r, d) {
                            if est != billed {
                                log.warn(
                                    "submit",
                                    who,
                                    format!(
                                        "计费与预估不符：{m}/{r}/{d}s 预估 {est}，实收 {billed}。\
                                         即梦可能调价了，价格表（dreamina.rs PRICES）需要重测。"
                                    ),
                                    None,
                                );
                            }
                        }
                    }
                } else {
                    // 提交这一刻还不能判它死（见 `dreamina::submit` 的说明），但必须出声：
                    // 事故那次 18 条全程一句异常都没有，人只能看着卡片停在「已提交」。
                    log.warn(
                        "submit",
                        who,
                        format!(
                            "已提交但回执异常 · {} · 状态 {} · 无计费回执，\
                             若 15 分钟内仍拿不到队列位次将判为幽灵单",
                            receipt.submit_id,
                            if receipt.gen_status.is_empty() {
                                "—"
                            } else {
                                &receipt.gen_status
                            }
                        ),
                        None,
                    );
                }
                sum.submitted += 1;
                // 每条提交完就把看板推一次：整批跑完才刷新的话，人盯着一列不动的卡片
                // 无法判断是「在提交」还是「卡住了」。
                if let Some(app) = log.app() {
                    crate::commands::v2v::emit_changed(pool, app, Some(clip.id)).await;
                }
            }
            Err(e) => {
                // 提交失败**不置 run**：没有 submit_id 就没花钱，留在 ready 让人改完重提。
                let msg = format!("{e}");
                log.error("submit", who, format!("提交失败：{msg}"), None);
                repo::mark_failed(pool, clip.id, "submit", &msg, now_unix()).await?;
                sum.failed += 1;
                if sum.first_error.is_none() {
                    sum.first_error = Some(msg);
                }
            }
        }
    }
    if total > 0 {
        log.info(
            "submit",
            None,
            format!("提交结束：成功 {} · 失败 {}", sum.submitted, sum.failed),
            None,
        );
    }
    Ok(sum)
}

/// 一轮轮询的结果（供事件与测试观察）。
#[derive(Debug, Clone, Default)]
pub struct PollSummary {
    pub finished: i64,
    pub failed: i64,
    pub still_running: i64,
    /// 本轮进度快照：(clip_id, gen_status, queue_idx)。
    pub progress: Vec<(i64, String, Option<i64>)>,
    /// 本轮实际问到答案的条数。
    pub polled: i64,
    /// 本轮因未到退避时间而跳过的条数（不是异常，是设计）。
    pub skipped: i64,
}

/// 超时判定（纯函数，便于测试）。`timeout` 为 `None` 即**不限**，永不判死。
pub fn is_timed_out(submitted_at: Option<i64>, now: i64, timeout: Option<i64>) -> bool {
    let Some(limit) = timeout else {
        return false;
    };
    submitted_at.is_some_and(|t| now - t > limit)
}

/// 幽灵单的宽限期。超过它还没拿到队列位次或计费回执，就不再当成「刚提交」。
///
/// 取 15 分钟是留了两个数量级的余量：实测健康单在提交后 **25 秒**内就同时有了
/// `queue_idx` 与 `credit_count`（`seedance2.0fast` 通道，队列第 4485 位）。
pub const PHANTOM_GRACE_SECS: i64 = 15 * 60;

/// 幽灵单判定（纯函数，便于测试）。
///
/// 「幽灵单」= 即梦接了单、给了 submit_id、`list_task` 里也查得到，但**从未入队、
/// 从未计费**：`queue_info` 与 `credit_count` 双双缺席，`gen_status` 永远停在
/// `querying`。2026-07-27 一次提交 19 条中了 18 条，挂了十几个小时无人察觉 ——
/// 因为在 GenDesk 眼里它和「在排队」长得一模一样，而超时默认不限，于是会一直轮询下去。
///
/// 判据要两个信号同时缺席，而不是只看队列位次：
/// - `credit_count` 是**决定性**的那个。健康单从排队第一秒起就带它（实测排队中的
///   `query_result` 返回 `credit_count: 8`），它缺席意味着这单根本没进计费。
/// - `queue_idx` 单独缺席不足以判死：万一哪天即梦对某些通道不下发 `queue_info`，
///   只看它会把正在排队、已经扣了钱的任务当场标死。
///
/// 判定结果是 `fail(phantom)` 而**不是**自动重投：重投要花钱，那是人的决定。
/// submit_id 照样留着（`额度不可撤回`），万一它哪天真的出片，重跑前还查得到。
pub fn is_phantom(
    gen_status: &str,
    queue_idx: Option<i64>,
    credit_count: Option<i64>,
    submitted_at: Option<i64>,
    now: i64,
) -> bool {
    phantom_suspect(gen_status, credit_count, submitted_at, now) && queue_idx.is_none()
}

/// 「像幽灵单，但还差一个信号才敢下结论」。
///
/// 整表扫描（`list_task`）**不回传 `queue_info`**，所以在那条路径上 `queue_idx` 永远
/// 缺席 —— 只凭它判死，等于把两个信号的规则悄悄降成一个。故扫描路径先用这个宽判据
/// 挑出嫌疑，再单发一次 `query_result` 拿队列位次，由 [`is_phantom`] 下最终结论。
///
/// 决定性的那个信号是 `credit_count`：它一旦出现，这单就确实进了即梦的计费，
/// 从此再不必为它单查一次 —— 这也是扫描机制能把单条查询降到几乎为零的原因。
pub fn phantom_suspect(
    gen_status: &str,
    credit_count: Option<i64>,
    submitted_at: Option<i64>,
    now: i64,
) -> bool {
    dreamina::classify_status(gen_status) == Outcome::Running
        && credit_count.is_none()
        && submitted_at.is_some_and(|t| now - t > PHANTOM_GRACE_SECS)
}

/// 整表扫描一轮 —— **主路径**。
///
/// 一次 `dreamina list_task` 拿回全部在跑任务的状态，逐条落库；只有两种情况才会为
/// 单条再起一个进程：**出片了**（要 `--download_dir` 落盘）与**疑似幽灵单**
/// （要 `queue_info` 才敢判死）。稳态下每轮就是一个进程，与在跑条数无关。
///
/// 扫描里认不出的 submit_id 回落到逐条 `query_result`（带退避）—— 那多半是翻页没覆盖
/// 或 CLI 输出变了，而认不出的条目恰恰最不该被放弃轮询。
pub async fn sweep_once(
    pool: &SqlitePool,
    dirs: &DataDirs,
    bin: &str,
    timeout_secs: Option<i64>,
    log: &Activity,
) -> AppResult<PollSummary> {
    let running = repo::list_running(pool).await?;
    let mut sum = PollSummary::default();
    if running.is_empty() {
        return Ok(sum);
    }
    LAST_SWEEP.store(now_unix(), std::sync::atomic::Ordering::Relaxed);
    std::fs::create_dir_all(dirs.clips())?;

    // 翻页直到把我们关心的 submit_id 都找齐（或翻不动了）。上限 5 页 = 500 条，
    // 远超任何一次实际在跑的量；翻不完的余数走回落路径，不会被漏掉。
    let want: std::collections::HashSet<&str> = running
        .iter()
        .filter_map(|c| c.submit_id.as_deref())
        .collect();
    let mut found: std::collections::HashMap<String, dreamina::QueryResult> =
        std::collections::HashMap::new();
    let mut list_error: Option<String> = None;
    for page in 0..5 {
        match dreamina::list_tasks(bin, dreamina::LIST_PAGE, page * dreamina::LIST_PAGE, log).await
        {
            Ok(items) => {
                let n = items.len();
                for t in items {
                    if want.contains(t.submit_id.as_str()) {
                        found.insert(t.submit_id, t.q);
                    }
                }
                sum.polled += 1;
                if n < dreamina::LIST_PAGE as usize || found.len() >= want.len() {
                    break;
                }
            }
            Err(e) => {
                // 整表扫描挂了不该让这一轮什么都不做：逐条回落还在，最坏退回旧机制。
                list_error = Some(format!("{e}"));
                break;
            }
        }
    }
    if let Some(e) = &list_error {
        log.warn(
            "poll",
            None,
            format!("整表扫描失败，本轮回落逐条查询：{e}"),
            None,
        );
    }

    for clip in running {
        let Some(submit_id) = clip.submit_id.clone() else {
            continue;
        };
        let now = now_unix();
        match found.get(&submit_id) {
            Some(q) => {
                // 扫描里的 queue_idx 恒为 None（list_task 不带 queue_info）→ 用
                // COALESCE 写库，绝不能把已经问到过的位次抹成空。
                let _ = repo::mark_swept(
                    pool,
                    clip.id,
                    &q.gen_status,
                    q.credit_count,
                    q.benefit_type.as_deref(),
                    now,
                )
                .await;
                settle(
                    pool,
                    dirs,
                    bin,
                    &clip,
                    q.clone(),
                    timeout_secs,
                    false,
                    log,
                    &mut sum,
                )
                .await?;
            }
            None => {
                // 回落：这一条扫描里没有。按退避决定要不要现在单查。
                if !is_due(clip.submitted_at, clip.polled_at, now) {
                    sum.still_running += 1;
                    sum.skipped += 1;
                    continue;
                }
                query_and_settle(pool, dirs, bin, &clip, timeout_secs, log, &mut sum).await?;
            }
        }
    }
    Ok(sum)
}

/// 逐条查询一轮（人点「查一次进度」走 `force`；扫描认不出的条目也走这里）。
pub async fn poll_once(
    pool: &SqlitePool,
    dirs: &DataDirs,
    bin: &str,
    timeout_secs: Option<i64>,
    // force：无视退避、这一轮全查一遍（人点了「查一次进度」时）。
    force: bool,
    log: &Activity,
) -> AppResult<PollSummary> {
    let running = repo::list_running(pool).await?;
    let mut sum = PollSummary::default();
    if running.is_empty() {
        return Ok(sum);
    }
    std::fs::create_dir_all(dirs.clips())?;

    for clip in running {
        if clip.submit_id.is_none() {
            continue;
        }
        let now = now_unix();
        if !force && !is_due(clip.submitted_at, clip.polled_at, now) {
            sum.still_running += 1;
            sum.skipped += 1;
            continue;
        }
        query_and_settle(pool, dirs, bin, &clip, timeout_secs, log, &mut sum).await?;
    }
    Ok(sum)
}

/// 单条 `query_result` → 落库 → 定态处置。
async fn query_and_settle(
    pool: &SqlitePool,
    dirs: &DataDirs,
    bin: &str,
    clip: &repo::ClipRow,
    timeout_secs: Option<i64>,
    log: &Activity,
    sum: &mut PollSummary,
) -> AppResult<()> {
    let Some(submit_id) = clip.submit_id.as_deref() else {
        return Ok(());
    };
    let who = Some((clip.id, clip.prompt_code.as_str()));
    let now = now_unix();
    // 下载目录用 clips/ 自身：CLI 会以 submit_id 命名，随后我们改名成 clip{id}.mp4，
    // 让文件名与库里的主键对得上（submit_id 在库里可被重跑覆盖，不适合做文件名）。
    let q = match dreamina::query(bin, submit_id, Some(&dirs.clips()), log, who).await {
        Ok(q) => q,
        Err(e) => {
            // 查询失败不改状态：网络抖动/CLI 临时不可用不该把已付费的任务判死。
            // 但**必须留下痕迹**：连续问不出话是故障，而它与「还在排队」在界面上
            // 长得一模一样 —— 区别只有 polled_at 停在哪一刻（0021）。
            log.warn("poll", who, format!("查询失败，下轮重试：{e}"), None);
            // 失败也要记「问过了」，否则退避对失败路径完全失效：CLI 一旦不可用，
            // 每个 tick 都会为每条起一个必然失败的进程。
            let _ = repo::mark_poll_attempt(pool, clip.id, now).await;
            sum.still_running += 1;
            return Ok(());
        }
    };
    sum.polled += 1;
    // 状态原文落库：切页/重启后看板仍答得出「这条在排队还是在跑」。
    let _ = repo::mark_polled(pool, clip.id, &q.gen_status, q.queue_idx, now).await;
    settle(pool, dirs, bin, clip, q, timeout_secs, true, log, sum).await
}

/// 一条在跑条目的定态处置：出片 / 判死 / 继续等。两条查询路径共用。
///
/// `queue_authoritative`：这份回体里的 `queue_idx` 可信吗。`query_result` 可信；
/// 整表扫描不带 `queue_info`，故为 false —— 那条路径上要判幽灵之前必须先单查确认，
/// 否则「两个信号同时缺席」这条规则会被悄悄降成「只看计费」，
/// 而即梦哪天不在 `list_task` 里回传计费，已经扣过钱的任务就会被当场标死。
#[allow(clippy::too_many_arguments)] // 定态处置本来就要同时看回体、库、磁盘与设置
async fn settle(
    pool: &SqlitePool,
    dirs: &DataDirs,
    bin: &str,
    clip: &repo::ClipRow,
    q: dreamina::QueryResult,
    timeout_secs: Option<i64>,
    queue_authoritative: bool,
    log: &Activity,
    sum: &mut PollSummary,
) -> AppResult<()> {
    let who = Some((clip.id, clip.prompt_code.as_str()));
    let now = now_unix();
    // 只在**状态变了**时记日志。每轮把在跑的「还在 querying」全记一遍，
    // 等于用「一切正常」把真正的报错挤出缓冲窗口。
    if clip.gen_status.as_deref() != Some(q.gen_status.as_str()) {
        let queue = q
            .queue_idx
            .map(|i| format!("（队列第 {i} 位）"))
            .unwrap_or_default();
        log.info(
            "poll",
            who,
            format!("即梦状态 → {}{queue}", q.gen_status),
            None,
        );
    }
    match dreamina::classify_status(&q.gen_status) {
        Outcome::Done => {
            // 整表扫描没有下载动作，故出片的条目一定要在这里单发一次带
            // `--download_dir` 的查询才能拿到本地路径。
            let q = if q.video_path.is_none() {
                let Some(sid) = clip.submit_id.as_deref() else {
                    return Ok(());
                };
                match dreamina::query(bin, sid, Some(&dirs.clips()), log, who).await {
                    Ok(full) => full,
                    Err(e) => {
                        log.warn("media", who, format!("取回成片失败，下轮重试：{e}"), None);
                        sum.still_running += 1;
                        return Ok(());
                    }
                }
            } else {
                q
            };
            let Some(src) = q.video_path.as_deref() else {
                // 报了成功但没落盘：当作还在跑，下轮再查（CLI 下载失败会重试）。
                log.warn("poll", who, "即梦报成功但未返回本地路径，下轮重试", None);
                sum.still_running += 1;
                return Ok(());
            };
            let (video, poster) = clip_paths(dirs, clip.id);
            if let Err(e) = std::fs::rename(src, &video) {
                log.warn(
                    "media",
                    who,
                    format!("成片改名失败，下轮重试：{e}"),
                    Some(format!("{src} → {}", video.display())),
                );
                sum.still_running += 1;
                return Ok(());
            }
            // 封面 = 首帧缩略图的**副本**。image2video 的第一帧就是这张图，
            // 语义上正确，而且不必依赖 ffmpeg 抽帧。
            let poster_out = (!clip.thumb_path.is_empty()
                && std::fs::copy(&clip.thumb_path, &poster).is_ok())
            .then(|| poster.to_string_lossy().to_string());
            repo::mark_ready_for_review(
                pool,
                clip.id,
                &video.to_string_lossy(),
                poster_out.as_deref(),
                q.width,
                q.height,
                q.fps,
                q.duration_sec,
                q.credit_count,
                q.benefit_type.as_deref(),
                now,
            )
            .await?;
            log.info(
                "media",
                who,
                format!(
                    "成片已落盘 · {}×{} · {:.1}s · 扣 {} 额度 · 计费型号 {} · 等了 {} → 待验收",
                    q.width.unwrap_or(0),
                    q.height.unwrap_or(0),
                    q.duration_sec.unwrap_or(0.0),
                    q.credit_count.unwrap_or(0),
                    q.benefit_type.as_deref().unwrap_or("—"),
                    fmt_dur(
                        clip.first_submitted_at
                            .or(clip.submitted_at)
                            .map_or(0, |t| now - t)
                    ),
                ),
                Some(video.to_string_lossy().to_string()),
            );
            sum.finished += 1;
        }
        Outcome::Failed => {
            let reason = if q.fail_reason.is_empty() {
                q.gen_status.clone()
            } else {
                q.fail_reason.clone()
            };
            log.error("poll", who, format!("即梦判定失败：{reason}"), None);
            repo::mark_failed(pool, clip.id, "provider", &reason, now).await?;
            sum.failed += 1;
        }
        Outcome::Running => {
            // 嫌疑升级：扫描路径拿不到队列位次，要判死之前先单查一次拿准。
            // 这一步每条最多发生一次 —— 一旦看到计费回执或队列位次，这条就再不入嫌疑名单。
            let mut q = q;
            let mut authoritative = queue_authoritative;
            if !authoritative
                && phantom_suspect(&q.gen_status, q.credit_count, clip.submitted_at, now)
            {
                if let Some(sid) = clip.submit_id.as_deref() {
                    match dreamina::query(bin, sid, None, log, who).await {
                        Ok(full) => {
                            let _ = repo::mark_polled(
                                pool,
                                clip.id,
                                &full.gen_status,
                                full.queue_idx,
                                now,
                            )
                            .await;
                            q = full;
                            authoritative = true;
                        }
                        // 「问不出话」绝不等于「判死」：确认查询失败就这一轮不判，
                        // 下一轮再来。扫描那份 queue_idx 恒为空，拿它去判等于把
                        // 「两个信号同时缺席」悄悄降成「只看计费」。
                        Err(e) => log.warn(
                            "poll",
                            who,
                            format!("幽灵单确认查询失败，本轮不判死：{e}"),
                            None,
                        ),
                    }
                }
            }
            if authoritative
                && is_phantom(
                    &q.gen_status,
                    q.queue_idx,
                    q.credit_count,
                    clip.submitted_at,
                    now,
                )
            {
                // 与超时是两回事，文案也必须相反：超时说「额度已扣，先继续等待」，
                // 幽灵单说「没扣费，可以直接重跑」。指错方向的代价是真金白银。
                let msg = format!(
                    "即梦接了单但未入队：提交后 {} 仍拿不到队列位次，也没有计费回执\
                     （末次状态 {}）。这单没有扣额度，直接「重跑」即可，不会重复扣费。",
                    fmt_dur(clip.submitted_at.map_or(0, |t| now - t)),
                    q.gen_status
                );
                log.error("poll", who, format!("幽灵单：{msg}"), None);
                repo::mark_failed(pool, clip.id, "phantom", &msg, now).await?;
                sum.failed += 1;
            } else if is_timed_out(clip.submitted_at, now, timeout_secs) {
                // 文案要指向「继续等待」而不是「重跑」：额度已经扣了，
                // 而重跑会清掉 submit_id = 再花一份钱买同一条视频。
                let msg = format!(
                    "提交后 {} 小时仍未出片（末次状态 {}）。额度已扣、即梦那边可能还在跑，\
                     建议先「继续等待」；确认不会出片了再重跑。",
                    timeout_secs.unwrap_or(0) / 3600,
                    q.gen_status
                );
                log.error("poll", who, format!("超时：{msg}"), None);
                repo::mark_failed(pool, clip.id, "timeout", &msg, now).await?;
                sum.failed += 1;
            } else {
                sum.still_running += 1;
                sum.progress
                    .push((clip.id, q.gen_status.clone(), q.queue_idx));
            }
        }
    }
    Ok(())
}

/// 秒数 → 「3 小时 12 分」。日志里的等待时长要一眼能读，不该逼人心算 11520 秒是多久。
pub fn fmt_dur(secs: i64) -> String {
    let s = secs.max(0);
    if s < 60 {
        return format!("{s} 秒");
    }
    if s < 3600 {
        return format!("{} 分钟", s / 60);
    }
    format!("{} 小时 {} 分", s / 3600, (s % 3600) / 60)
}

/// 当前七态计数（事件载荷）。
pub async fn counts(pool: &SqlitePool) -> AppResult<StageCounts> {
    let mut c = StageCounts::from_rows(&repo::stage_counts(pool).await?);
    c.no_asset = repo::count_pass_without_pack(pool).await?;
    Ok(c)
}

/// 后台轮询循环。
///
/// 每轮**重读设置**而不是把配置捕获进闭包：改了 CLI 路径或关掉轮询开关应当立刻生效，
/// 而不是要求重启应用（那种「改了没反应」正是 v0.11.1 那个 bug 的手感）。
pub fn spawn(
    pool: SqlitePool,
    dirs: std::sync::Arc<DataDirs>,
    app: tauri::AppHandle,
    log: Activity,
) {
    use crate::v2v::events::{V2vProgress, V2vTick};
    use tauri_specta::Event;

    tauri::async_runtime::spawn(async move {
        // 启动恢复：提交过程中被杀的（run 但无 submit_id）退回 ready 让人重提。
        // 有 submit_id 的一条都不动——额度已扣，重提等于花两份钱买同一条视频。
        match repo::recover_orphan_submits(&pool, now_unix()).await {
            Ok(n) if n > 0 => log.warn(
                "poll",
                None,
                format!("中断恢复：{n} 条提交到一半的条目退回待提交（未产生额度消耗）"),
                None,
            ),
            Err(e) => log.error("poll", None, format!("中断恢复失败：{e}"), None),
            _ => {}
        }
        // 上一轮整表扫描的时刻。心跳 6 秒一次（界面那句「12 秒前」要跟得上），
        // 扫描 30 秒一次（那才是真正起进程的动作）—— 两个节拍分开。
        let mut last_sweep = 0i64;
        // 上一次「队列告急」通知的时刻。放在循环局部而不是库里：重启应用后重发一次
        // 是可以接受的（那时人本来就在看界面），而每 30 秒弹一次通知不可接受。
        let mut last_refill = super::autofill::Memo::default();
        loop {
            tokio::time::sleep(TICK).await;
            // 心跳与日志是两件事：日志只在有事发生时增长，而「轮询器还活着吗」
            // 恰恰要在什么都没发生时也答得出 —— 静默的界面和卡死的轮询器长得一样。
            let mut tick = V2vTick {
                at: now_unix(),
                running: 0,
                enabled: true,
                finished: 0,
                failed: 0,
                error: None,
            };
            let settings = match crate::commands::v2v::load_settings(&pool).await {
                Ok(s) => s,
                Err(e) => {
                    log.error(
                        "poll",
                        None,
                        format!("读图生视频设置失败，跳过本轮：{e}"),
                        None,
                    );
                    tick.error = Some(format!("{e}"));
                    let _ = tick.emit(&app);
                    continue;
                }
            };
            tick.running = repo::stage_counts(&pool)
                .await
                .map(|rows| StageCounts::from_rows(&rows).run)
                .unwrap_or(0);
            if !settings.poll_enabled {
                tick.enabled = false;
                let _ = tick.emit(&app);
                continue;
            }
            // 心跳每 6 秒发一次，但真正去问即梦是 30 秒一次。分开的理由是成本结构：
            // 心跳是一次内存读，扫描是一个进程。
            if tick.at - last_sweep >= SWEEP_SECS {
                last_sweep = tick.at;
                match sweep_once(&pool, &dirs, &settings.bin, settings.timeout_secs(), &log).await {
                    Ok(sum) => {
                        tick.finished = sum.finished;
                        tick.failed = sum.failed;
                        for (clip_id, gen_status, queue_idx) in &sum.progress {
                            let _ = V2vProgress {
                                clip_id: *clip_id,
                                gen_status: gen_status.clone(),
                                queue_idx: *queue_idx,
                                polled_at: tick.at,
                            }
                            .emit(&app);
                        }
                        if sum.finished > 0 || sum.failed > 0 {
                            crate::commands::v2v::emit_changed(&pool, &app, None).await;
                            if sum.finished > 0 {
                                use crate::engine::events::EventSink;
                                crate::engine::events::TauriSink::new(app.clone()).notify(
                                    "视频已出片".into(),
                                    format!("{} 条成片待验收", sum.finished),
                                );
                            }
                        }
                    }
                    Err(e) => {
                        log.error("poll", None, format!("本轮轮询失败：{e}"), None);
                        tick.error = Some(format!("{e}"));
                    }
                }
                // 自动补单跟着扫描的节拍走：它要看的「在跑几条」正是扫描刚更新过的。
                super::autofill::tick(&pool, &settings, &app, &log, &mut last_refill).await;
            }
            let _ = tick.emit(&app);
        }
    });
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即失败
mod tests {
    use super::*;

    fn clip(model: Option<&str>, dur: Option<i64>, res: Option<&str>) -> repo::ClipRow {
        repo::ClipRow {
            id: 1,
            work_id: 1,
            group_id: None,
            group_name: String::new(),
            batch_id: None,
            stage: "ready".into(),
            source_prompt: String::new(),
            variable_part: String::new(),
            video_prompt: Some("p".into()),
            model_version: model.map(|s| s.to_string()),
            duration: dur,
            video_resolution: res.map(|s| s.to_string()),
            submit_id: None,
            credit_count: None,
            video_path: None,
            poster_path: None,
            width: None,
            height: None,
            fps: None,
            duration_sec: None,
            attempt: 0,
            error_type: None,
            error_message: None,
            submitted_at: None,
            gen_status: None,
            queue_idx: None,
            polled_at: None,
            benefit_type: None,
            first_submitted_at: None,
            submit_credit: None,
            submit_status: None,
            created_at: 0,
            rewrote_at: None,
            finished_at: None,
            reviewed_at: None,
            auto_submitted: 0,
            asset_pack_id: None,
            in_asset_lib: 0,
            updated_at: 0,
            prompt_code: "GG-0001".into(),
            image_path: "/img.jpg".into(),
            thumb_path: "/thumb.jpg".into(),
            accepted_at: 0,
        }
    }

    // skill 对某一条给的建议优先于全局默认：它是看过图之后的判断
    // （这条动势大就给 8 秒），全局默认只是兜底。
    #[test]
    fn per_clip_params_win_over_defaults() {
        let defaults = GenOpts {
            model_version: Some("seedance2.0fast".into()),
            duration: Some(4),
            video_resolution: Some("720p".into()),
            session: Some(0),
        };
        let o = opts_for(
            &clip(Some("seedance2.0_vip"), Some(8), Some("1080p")),
            &defaults,
        );
        assert_eq!(o.model_version.as_deref(), Some("seedance2.0_vip"));
        assert_eq!(o.duration, Some(8));
        assert_eq!(o.video_resolution.as_deref(), Some("1080p"));
        assert_eq!(o.session, Some(0), "session 只由设置决定，不由 skill 指定");
    }

    // 没给的项回落到默认值，逐项回落而非整组回落。
    #[test]
    fn missing_per_clip_params_fall_back_field_by_field() {
        let defaults = GenOpts {
            model_version: Some("seedance2.0fast".into()),
            duration: Some(4),
            video_resolution: Some("720p".into()),
            session: None,
        };
        let o = opts_for(&clip(None, Some(9), None), &defaults);
        assert_eq!(o.model_version.as_deref(), Some("seedance2.0fast"));
        assert_eq!(o.duration, Some(9), "只覆盖它给了的那一项");
        assert_eq!(o.video_resolution.as_deref(), Some("720p"));
    }

    // 封面必须是 clip 自己的文件，绝不能是作品缩略图路径本身：
    // 清空废纸篓会物理删 file_paths 里的路径，指着作品缩略图就会删掉还活着的作品的图。
    #[test]
    fn poster_path_is_owned_by_the_clip_not_shared_with_the_work() {
        let dirs = DataDirs::new("/data");
        let (video, poster) = clip_paths(&dirs, 42);
        assert!(video.ends_with("clip42.mp4"));
        assert!(poster.ends_with("clip42.jpg"));
        assert_ne!(
            poster.to_string_lossy(),
            "/thumb.jpg",
            "封面不得与作品缩略图同路径"
        );
        assert!(
            poster.starts_with("/data/clips"),
            "封面须落在 clips/ 自己的目录里"
        );
    }

    // 成片文件名锚在 clip id 而不是 submit_id：重跑会换 submit_id，
    // 文件名跟着换就会在 clips/ 里堆出认不出主人的孤儿。
    #[test]
    fn media_filenames_are_anchored_to_clip_id() {
        let dirs = DataDirs::new("/data");
        let (a, _) = clip_paths(&dirs, 7);
        let (b, _) = clip_paths(&dirs, 7);
        assert_eq!(a, b, "同一 clip 的路径必须恒定（重跑覆盖旧文件）");
    }

    // 超时判定：设了上限就按上限判，**没设就永不判死**。
    #[test]
    fn timeout_respects_the_configured_limit() {
        let three_hours = Some(3 * 3600);
        assert!(!is_timed_out(Some(1000), 1000 + 10, three_hours));
        assert!(is_timed_out(Some(1000), 1000 + 3 * 3600 + 1, three_hours));
        assert!(
            !is_timed_out(None, 999_999, three_hours),
            "没有提交时间的条目不该被判超时"
        );
    }

    // 「不限」必须真的是不限：默认值就是它，而用户要的正是「睡一觉起来再收」。
    // 原来硬编码 45 分钟，把 19 条还在 querying 的任务全判死了（额度已扣）。
    #[test]
    fn no_timeout_never_kills_a_running_task() {
        assert_eq!(DEFAULT_TIMEOUT_HOURS, None, "默认必须是不限");
        assert!(!is_timed_out(Some(0), 999_999_999, None));
    }

    // 幽灵单：即梦接了单却没入队，`queue_idx` 与 `credit_count` 双双缺席。
    // 2026-07-27 一次提交 19 条中了 18 条，而「超时默认不限」意味着没人会去打断它。
    #[test]
    fn phantom_needs_both_signals_missing_and_the_grace_period_over() {
        let late = PHANTOM_GRACE_SECS + 1;
        assert!(
            is_phantom("querying", None, None, Some(0), late),
            "两个信号都缺 + 过了宽限期 = 幽灵单"
        );
    }

    // 宽限期内不判：健康单实测 25 秒内就拿到位次，但网络慢一点也不该被当场标死。
    #[test]
    fn phantom_is_not_judged_inside_the_grace_period() {
        assert!(!is_phantom(
            "querying",
            None,
            None,
            Some(0),
            PHANTOM_GRACE_SECS
        ));
    }

    // **单看队列位次不足以判死**。万一哪天即梦对某些通道不下发 queue_info，
    // 只凭它会把正在排队、钱已经扣了的任务当场标死 —— 那是不可逆的误伤。
    #[test]
    fn credit_receipt_alone_saves_a_clip_from_being_judged_phantom() {
        assert!(
            !is_phantom("querying", None, Some(8), Some(0), 999_999),
            "有计费回执就说明真进了即梦，不得判幽灵"
        );
        assert!(
            !is_phantom("querying", Some(4485), None, Some(0), 999_999),
            "有队列位次同样不得判幽灵"
        );
    }

    // 终态不归幽灵管：出片的走 Done、判失败的走 Failed，各有各的处置。
    #[test]
    fn phantom_only_applies_to_running_clips() {
        assert!(!is_phantom("success", None, None, Some(0), 999_999));
        assert!(!is_phantom("expired", None, None, Some(0), 999_999));
    }

    // 退避是「能不能睡一觉再来收」的成本前提：每条每 6 秒查一次，19 条跑一夜
    // 就是九万多次进程启动。等得越久问得越稀，且必须单调不减 —— 否则等到某个
    // 时长反而突然变密，就成了「越晚越费」。
    #[test]
    fn poll_interval_backs_off_monotonically_with_age() {
        let steps = [
            0, 60, 119, 120, 599, 600, 3599, 3600, 21_599, 21_600, 86_400,
        ];
        let mut last = 0;
        for age in steps {
            let cur = poll_interval_for(age);
            assert!(cur >= last, "间隔不得回缩：age={age} 时 {cur} < {last}");
            last = cur;
        }
        assert_eq!(poll_interval_for(0), 10, "刚提交要快，尽快确认进没进队列");
        assert_eq!(poll_interval_for(86_400), 600, "过夜十分钟一次足够");
    }

    // 退避带来的实际节省：19 条跑 8 小时，从九万次降到两千次量级。
    // 这条测的不是某个具体数字，而是「量级确实降下来了」。
    #[test]
    fn backoff_cuts_overnight_polling_by_orders_of_magnitude() {
        let clips = 19i64;
        let hours = 8i64;
        let naive = clips * (hours * 3600 / 6); // 原实现：每条每 6 秒
        let mut backed_off = 0i64;
        let mut t = 0i64;
        while t < hours * 3600 {
            t += poll_interval_for(t);
            backed_off += clips;
        }
        assert!(
            backed_off * 20 < naive,
            "退避后应至少省 20 倍：{backed_off} vs {naive}"
        );
    }

    /// 整表扫描的进程数与在跑条数**脱钩** —— 这才是它取代退避的理由。
    ///
    /// 退避靠「问得少」省钱，代价是出片延迟最长十分钟；扫描靠「一次问全部」省钱，
    /// 于是频率可以调高，成本与延迟同时变好。条数越多，差距越大。
    #[test]
    fn sweeping_decouples_process_count_from_clip_count() {
        let hours = 8i64;
        let sweeps = hours * 3600 / SWEEP_SECS;
        let backoff_calls = |clips: i64| {
            let mut n = 0i64;
            let mut t = 0i64;
            while t < hours * 3600 {
                t += poll_interval_for(t);
                n += clips;
            }
            n
        };
        // 19 条：两者同量级，但扫描的出片延迟只有 30 秒。
        assert!(sweeps < backoff_calls(19) * 2);
        // 100 条：退避随条数线性涨，扫描一动不动。
        assert!(
            sweeps * 5 < backoff_calls(100),
            "100 条在跑时扫描应至少省 5 倍：{sweeps} vs {}",
            backoff_calls(100)
        );
        assert!(
            SWEEP_SECS < poll_interval_for(3600),
            "扫描节拍必须比退避到达的稳态间隔更密，否则换机制换了个寂寞"
        );
    }

    // 扫描路径拿不到队列位次，故只凭「没有计费回执」不得判死 ——
    // 那会把「两个信号同时缺席」这条规则悄悄降成一个信号。
    #[test]
    fn a_suspect_is_not_yet_a_verdict() {
        let late = PHANTOM_GRACE_SECS + 1;
        // 嫌疑成立：过了宽限期还没有计费回执。
        assert!(phantom_suspect("querying", None, Some(0), late));
        // 但只有同时拿到「队列位次也缺席」这一条，才是判决。
        assert!(is_phantom("querying", None, None, Some(0), late));
        assert!(
            !is_phantom("querying", Some(4485), None, Some(0), late),
            "确认查询问到了位次 → 它在排队，只是即梦这一路没回传计费"
        );
        // 有计费就连嫌疑都不成立 —— 这条正是扫描能把单条查询降到几乎为零的原因：
        // 一旦看到计费，这一条永远不再需要为「它是不是幽灵」单独问一次。
        assert!(!phantom_suspect("querying", Some(8), Some(0), late));
    }

    // 队列面板那句「下次查询还有 N 秒」必须与真正跑的循环同源。
    #[test]
    fn next_sweep_countdown_tracks_the_loop() {
        // 还没跑过第一轮 → 不编一个数字出来。
        LAST_SWEEP.store(0, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(next_sweep_in(1000), None);
        LAST_SWEEP.store(1000, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(next_sweep_in(1000), Some(SWEEP_SECS));
        assert_eq!(next_sweep_in(1000 + SWEEP_SECS), Some(0));
        assert_eq!(next_sweep_in(9999), Some(0), "过点了就是 0，不是负数");
    }

    // 从没查过的立刻查（刚提交那一刻就想知道进没进队列）；查过的按退避等到点。
    #[test]
    fn due_check_polls_new_clips_immediately_then_waits() {
        assert!(is_due(Some(1000), None, 1000), "没查过的必须立刻查");
        // 刚提交 30 秒，上次查是 5 秒前 → 10 秒间隔还没到。
        assert!(!is_due(Some(1000), Some(1025), 1030));
        assert!(is_due(Some(1000), Some(1020), 1030));
        // 等了两小时的，间隔是 300 秒：60 秒前查过 → 不到点。
        let two_h = 1000 + 7200;
        assert!(!is_due(Some(1000), Some(two_h - 60), two_h));
        assert!(is_due(Some(1000), Some(two_h - 301), two_h));
    }

    // 等待时长要一眼能读，不该逼人心算 11520 秒是多久。
    #[test]
    fn duration_formatting_is_human_readable() {
        assert_eq!(fmt_dur(45), "45 秒");
        assert_eq!(fmt_dur(600), "10 分钟");
        assert_eq!(fmt_dur(11_520), "3 小时 12 分");
        assert_eq!(fmt_dur(-5), "0 秒", "时钟回拨不该显示负数");
    }
}
