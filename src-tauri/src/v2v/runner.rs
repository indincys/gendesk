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
use crate::error::{AppError, AppResult};
use crate::files::DataDirs;

use super::activity::Activity;
use super::dreamina::{self, GenOpts, Outcome};
use super::events::StageCounts;

/// 循环的心跳节拍。**不等于查询频率**：心跳快是为了界面上那句「12 秒前」跟得上。
pub const TICK: Duration = Duration::from_secs(6);

/// 整表扫描的间隔（秒），**按通道分档**。
///
/// ## 为什么只能轮询
///
/// 实跑 `dreamina -h` 确认过：CLI 没有任何推送机制（无 watch / stream / webhook /
/// subscribe）。`image2video --poll=N` 只是把 1 秒一次的轮询搬进子进程，进程被杀即丢，
/// 比自建轮询器更差（`dreamina::command_line` 已显式 `--poll=0` 关掉它）。
/// 所以「事件驱动不轮询」在这条链路上做不到，能改的只有频率。
///
/// ## `list_task` 是**本机缓存**，不是服务端状态（0031 实测推翻了旧结论）
///
/// 旧注释写的是「一个进程就回一整页全部在跑任务的状态，进程数与在跑条数脱钩（O(1)）」。
/// **前半句在字面上成立，但它回的是过期状态**：`dreamina list_task` 读的是
/// `~/.dreamina_cli/tasks.db`（本机 SQLite，表 `aigc_task`），而那张表里的 `gen_status`
/// **只有对该 `submit_id` 单独跑过 `query_result` 才会被写回**。
///
/// 实证：5 条 2026-07-27 14:04–14:05 提交的任务，在本机表里一直是 `querying`；
/// 2026-07-28 16:48 我逐条 `query_result` 之后，它们的 `gen_status` 变成 `success`，
/// 而 `update_time` 恰好停在 16:48:30–16:48:51 —— 整整 26 小时里 `list_task` 一次都没有
/// 自己去问过服务端。
///
/// 所以真正推进状态机的是本函数里那段**逐条** `query_result`，它受
/// [`POSITION_QUERY_BUDGET`] 限制。成本模型因此是：
///
/// - `list_task` 翻页 = 本地读，**不走网络**，可以忽略；
/// - `query_result` = 唯一的网络调用，每轮 ≤ [`POSITION_QUERY_BUDGET`] 个进程。
///
/// 即 O(min(n, 8)) 而不是 O(1)。频率仍然是成本旋钮，只是旋的是后者。
///
/// ## 分档的依据是「这条要等多久」，而那由通道决定
///
/// 实测：非 VIP 排在第 4485 位、要等几小时；VIP 直接 `Generating`、1–3 分钟出片。
/// 拿同一个节拍去问这两种任务，对谁都不合适。故：
///
/// | 在跑集合 | 间隔 | 8 小时过夜的进程数 | 相对 30 秒常数 |
/// |---|---|---|---|
/// | 含 VIP | 5 分钟 | 96 | 少 10× |
/// | 全非 VIP | 600 秒 = 10 分钟 | 48 | 少 20× |
/// | 空 | 不扫 | 0 | —— |
///
/// **代价写在这里，免得下次有人以为是 bug**：VIP 1–3 分钟就出片，而 5 分钟一档意味着
/// 一条早已出片的 VIP 单最多可以在界面上「已提交」着躺 5 分钟。对策是
/// [`SWEEP_AFTER_SUBMIT_SECS`] 的补扫，不是把档位调回去。
pub const SWEEP_VIP_SECS: i64 = 300;
pub const SWEEP_PLAIN_SECS: i64 = 600;

/// 每批提交后多久补扫一次（秒）。
///
/// 按**批**不按条：20 条 VIP 一起提交只多这一个进程。它存在的唯一理由是让上面那张表
/// 里 VIP 那行的代价（最多 5 分钟看不到已出的片）在最常见的场景里不发生 ——
/// 人刚点完提交，正盯着屏幕。
pub const SWEEP_AFTER_SUBMIT_SECS: i64 = 60;

/// 在跑集合该用哪个档位。
///
/// 「含 VIP 就走快档」而不是按多数派：慢档会让那几条快单白等，而快档对慢单的额外
/// 代价只是每 8 小时多 48 个进程。判错方向的代价不对等，就往便宜的那边错。
pub fn sweep_interval(any_vip: bool) -> i64 {
    if any_vip {
        SWEEP_VIP_SECS
    } else {
        SWEEP_PLAIN_SECS
    }
}

/// 上一次整表扫描的时刻（进程内）。
///
/// 队列面板要显示「下次查询还有几秒」，而那个数字由扫描节拍决定 —— 与其让命令层
/// 照着公式重算一遍（必然与实际跑的循环分叉），不如让循环自己记下来。
static LAST_SWEEP: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

/// 当前生效的扫描间隔（秒）。循环每轮按在跑集合更新它，面板读它。
static SWEEP_EVERY: std::sync::atomic::AtomicI64 =
    std::sync::atomic::AtomicI64::new(SWEEP_PLAIN_SECS);

/// 请求一次提前扫描：把「上次扫描时刻」往前挪，使下一轮在 N 秒后到点。
///
/// 提交成功后调用。**不是立刻扫**：提交那一刻即梦还没来得及给位次，实测健康单
/// 25 秒内才拿到 `queue_idx`，立刻问只会得到一份什么都没有的回体。
pub fn request_sweep_soon(now: i64) {
    let every = SWEEP_EVERY.load(std::sync::atomic::Ordering::Relaxed);
    LAST_SWEEP.store(
        now - (every - SWEEP_AFTER_SUBMIT_SECS).max(0),
        std::sync::atomic::Ordering::Relaxed,
    );
}

/// 距下一次整表扫描还有多少秒（`None` = 还没跑过第一轮）。
pub fn next_sweep_in(now: i64) -> Option<i64> {
    let last = LAST_SWEEP.load(std::sync::atomic::Ordering::Relaxed);
    let every = SWEEP_EVERY.load(std::sync::atomic::Ordering::Relaxed);
    (last > 0).then(|| (last + every - now).max(0))
}

/// 上一次**真的去问过即梦**的时刻（`None` = 这个进程还没问过）。
///
/// 顶栏那个刷新按钮要显示的是这个，**不是心跳时刻**。心跳 6 秒一次、纯内存读，
/// 拿它写「3 秒前」会让人以为数据是三秒前的新鲜货，而真实查询可能已经是十分钟前 ——
/// 这正是它取代的那颗胶囊最误导的地方。
pub fn last_sweep_at() -> Option<i64> {
    let last = LAST_SWEEP.load(std::sync::atomic::Ordering::Relaxed);
    (last > 0).then_some(last)
}

/// 单条 clip 距上次查询要隔多久才再查一次（秒），按它已经等了多久递增。
///
/// **只用于回落路径**：整表扫描里认不出的 submit_id（翻页没覆盖到、CLI 输出变了）
/// 仍走逐条 `query_result`，那时退避照旧生效。主路径已经不需要它了，
/// 但这条路必须留着 —— 认不出的条目恰恰是最不该被放弃轮询的那些。
///
/// 下限抬到 [`SWEEP_VIP_SECS`]：回落路径是**逐条**起进程的（O(n)），它比整表扫描贵
/// 得多，绝没有道理比扫描问得还勤。原来那两档 10s/30s 是在 30 秒常数扫描下定的，
/// 分档之后它们会让「扫描认不出的那几条」反过来成为进程数的大头。
pub fn poll_interval_for(age_secs: i64) -> i64 {
    match age_secs {
        a if a < 600 => SWEEP_VIP_SECS, // 前 10 分钟：跟快档同步，快单可能已出片
        a if a < 6 * 3600 => SWEEP_PLAIN_SECS, // 数小时内：十分钟一次
        _ => 1800,                      // 过夜：半小时一次足够
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
    /// 这一批里被在跑上限挡在本地、排队等空位的条数（0028）。**不是失败**。
    pub queued: i64,
    /// 第一条失败的原因（批量提交时逐条上报太吵，给一条代表 + 计数）。
    pub first_error: Option<String>,
}

// ─────────────────────── 在跑上限（即梦的账户级并发闸门）───────────────────────

/// 每条通道默认同时在跑几条。
///
/// **1，因为那是实测到的真值**：2026-07-28 一批 9 条同时提交（全部走 `seedance2.0fast`），
/// 即梦逐条给了 submit_id，随后 8 条回来 `ret=1310 ExceedConcurrencyLimit`，
/// 只有 1 条真的进了队列。
///
/// 猜大猜小的代价不对等：猜小只是让后面那些多等一会儿（而「等」在非 VIP 通道上本来
/// 就是免费的，那正是常驻队列成立的前提）；猜大则是一批片子集体躺进「处理异常」，
/// 还得人一条条辨认哪些是真失败。所以默认往小了猜，由 [`observe_concurrency_reject`]
/// 在撞墙时自己收敛。
pub const DEFAULT_MAX_IN_FLIGHT: i64 = 1;

/// 配置值的取值范围。上限 20 不是技术限制，是「一次性把余额烧光」的护栏。
pub const MAX_IN_FLIGHT_CAP: i64 = 20;

/// 这一次运行里**逐通道实测**到的并发上限（缺席 = 这条通道还没撞过墙）。
///
/// ## 为什么是「逐通道」而不是一个账户级的数（0031）
///
/// v0.24.0 的结论是「即梦的并发上限是账户级的」，那是**从单通道样本上做的过度归纳**：
/// 撞墙那一批 9 条全部走 `seedance2.0fast`，所以它证明的只是「2.0fast 同时跑得下 1 条」。
///
/// 反证有三条，都来自这个账户的真实回体：
///
/// 1. `query_result` 的 `queue_info.debug_info` 里有 `dreamina_matrix_queue_name`，
///    逐通道不同 —— 1.5pro 是 `dreamina_fusion_video35_pro_i2v_720p`、
///    2.0 是 `dreamina_fusion_video40_pro`、2.0mini 是 `dreamina_fusion_video40_mini`、
///    2.0_vip 是 `dreamina_fusion_video40_pro_vision`。**每条通道各有一条队**。
/// 2. 2026-07-27 逐通道实拍价格时，5 条不同通道的单子同时下出去，**全部**被收下并计费，
///    一条 `ExceedConcurrencyLimit` 都没有。
/// 3. 同日 14:45–14:47 的 18 条 `_vip` 单在 90 秒内全部提交、全部收下、全部出片；
///    若上限真是账户级的 1，那 17 条会当场被弹回来。
///
/// 按账户级口径记的代价是实打实的：2.0fast 那条长队一占上，2.0mini 上的 6 条
/// **一条都发不出去**，而它们本可以并行跑完。
///
/// 进程内、不落库：即梦随时可以按账户等级调整这些数，而一个被写进库的旧观测会在上限
/// 放宽之后永远把队列压在低位，且没人知道为什么。重启即重新试探，代价只是再撞一次墙
/// —— 而撞墙不花钱也不丢条目。
static OBSERVED: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, i64>>> =
    std::sync::OnceLock::new();

fn observed() -> std::sync::MutexGuard<'static, std::collections::HashMap<String, i64>> {
    // 中毒的锁照用：里面只有一张「这条通道最多几条」的观测表，一个写者 panic
    // 不会让这些数字失去意义，而为它把整条提交链路停掉才是真的损失。
    OBSERVED
        .get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// 某条通道生效的在跑上限 = 配置值与该通道实测值取小。
pub fn effective_in_flight(channel: &str, configured: i64) -> i64 {
    let cfg = configured.clamp(1, MAX_IN_FLIGHT_CAP);
    match observed().get(channel) {
        Some(o) => cfg.min((*o).max(1)),
        None => cfg,
    }
}

/// 某条通道撞上 `ExceedConcurrencyLimit` 了 —— 把该通道的实测上限收敛到
/// 「即梦当时在这条通道上确实收下的条数」。
///
/// `accepted` 取此刻**同通道**在跑、且有计费回执或队列位次的条数
/// （`repo::count_running_accepted_on`）：被拒的那些两样都没有，所以剩下的正是
/// 即梦愿意在这条通道上同时跑的量。
///
/// 只降不升，且下限为 1；升回去交给下次重启。这条路径要防的是「设了 5、真值是 1」时
/// 每轮提交 4 条、每轮被弹回 4 条的空转，一轮之内就该收敛。
pub fn observe_concurrency_reject(channel: &str, accepted: i64) -> i64 {
    let v = accepted.max(1);
    let mut map = observed();
    let slot = map.entry(channel.to_string()).or_insert(v);
    *slot = (*slot).min(v).max(1);
    *slot
}

/// 某条通道实测到的上限（`None` = 这次运行还没在这条通道上撞过墙）。
/// 界面据此说明「为什么这条通道只跑了 1 条」。
pub fn observed_in_flight_limit(channel: &str) -> Option<i64> {
    observed().get(channel).map(|v| (*v).max(1))
}

/// 这条通道现在还能往即梦发几条。
pub async fn free_slots(
    pool: &SqlitePool,
    default_model: &str,
    channel: &str,
    configured: i64,
) -> AppResult<i64> {
    let used = repo::count_in_flight_on(pool, default_model, channel).await?;
    Ok((effective_in_flight(channel, configured) - used).max(0))
}

/// 一条 clip 实际走哪条通道：它自己写死的型号优先，没写就落到设置里的默认型号。
///
/// 与 `repo::CHANNEL_OF` 那段 SQL **必须**同口径 —— 一边按型号分桶、另一边按别的口径
/// 数空位，结果就是数着 A 通道的空位往 B 通道发单。
pub fn channel_of<'a>(clip_model: Option<&'a str>, default_model: &'a str) -> &'a str {
    clip_model
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(default_model)
}

/// 放行一批：人点了「确认提交」。
///
/// **能发几条发几条，其余留在本地队列**（0028），由轮询循环在空位腾出来时自动补上。
/// 这是这一版的核心改动：在此之前，选 9 条就是 9 条一起砸向即梦，而即梦只接得住 1 条。
///
/// 全部 id 先进本地队列再取队首去发 —— 而不是「发 N 条、剩下的另行标记」：
/// 两条路径会在「发到一半失败了」时对同一条给出不同的归属，而队列是唯一真相时不会。
pub async fn release_and_submit(
    pool: &SqlitePool,
    bin: &str,
    ids: &[i64],
    defaults: &GenOpts,
    configured: i64,
    log: &Activity,
) -> AppResult<SubmitSummary> {
    let now = now_unix();
    repo::mark_submit_queued(pool, ids, now).await?;
    let mut sum = drain_queue(pool, bin, defaults, configured, log).await?;
    sum.queued = repo::count_submit_queued(pool).await?;
    Ok(sum)
}

/// 把本地队列往即梦补到满 —— **逐通道各补各的**（0031）。
///
/// 早前这里只有一个全局空位数与一个全局队首：2.0fast 那条长队一把位子占满，队列里
/// 那 6 条 2.0mini 就再也发不出去，尽管 2.0mini 那条队一条都没有在跑。即梦按模型通道
/// 各排各的队（见 [`OBSERVED`] 的实测记录），所以闸门也必须逐通道算。
pub async fn drain_queue(
    pool: &SqlitePool,
    bin: &str,
    defaults: &GenOpts,
    configured: i64,
    log: &Activity,
) -> AppResult<SubmitSummary> {
    let default_model = defaults.model_version.as_deref().unwrap_or("");
    let mut out = SubmitSummary::default();
    for ch in repo::channel_stats(pool, default_model).await? {
        if ch.queued <= 0 {
            continue;
        }
        let slots = free_slots(pool, default_model, &ch.model_version, configured).await?;
        if slots <= 0 {
            continue;
        }
        let ids =
            repo::pick_submit_queued_on(pool, default_model, &ch.model_version, slots).await?;
        if ids.is_empty() {
            continue;
        }
        // 一条通道提交失败不该连坐其余通道：它们各排各的队，凭什么一起停。
        match submit_batch(pool, bin, &ids, defaults, log).await {
            Ok(s) => {
                out.submitted += s.submitted;
                out.failed += s.failed;
                if out.first_error.is_none() {
                    out.first_error = s.first_error;
                }
            }
            Err(e) => {
                log.error(
                    "submit",
                    None,
                    format!(
                        "通道 {} 补位失败：{e}",
                        dreamina::short_label(&ch.model_version)
                    ),
                    None,
                );
                out.failed += ids.len() as i64;
                if out.first_error.is_none() {
                    out.first_error = Some(format!("{e}"));
                }
            }
        }
    }
    Ok(out)
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
    // **先认领再提交**。人点「确认提交」与常驻队列补单器跑在不同任务里，中间隔着整个
    // CLI 网络往返；两边都读到同一条 `ready`，就会为同一张图下两次单、扣两份钱，
    // 而 `UNIQUE(work_id)` 拦不住（第二次只是覆盖 submit_id，第一张片子当场变成孤儿）。
    let rows = repo::claim_ready(pool, ids, now_unix()).await?;
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
            // 这一条根本没提交、一分钱没花 → 放回待提交，不让认领把它卡在 run 里
            // （那要等下次启动的孤儿恢复才回得来）。
            let _ = repo::release_claim(pool, clip.id, now_unix()).await;
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
                if let Err(e) = persist_submit(pool, clip.id, &receipt, log, who).await {
                    // 落库彻底失败：钱已经扣了，而 submit_id 只剩日志这一处凭证。
                    // 绝不 `?` 冒泡 —— 那会连带中止整批（后面每条都还没提交，
                    // 白白挡住），而这一条的 submit_id 会随内存一起消失。
                    log.error(
                        "submit",
                        who,
                        format!(
                            "已提交但落库失败 · submit_id {} · {e}。\
                             这一单的额度已经扣了，凭证只剩这条日志 —— \
                             重跑会再花一份钱，先用这个 id 去即梦查一次。",
                            receipt.submit_id
                        ),
                        None,
                    );
                    sum.failed += 1;
                    if sum.first_error.is_none() {
                        sum.first_error =
                            Some(format!("{} 提交回执落库失败：{e}", clip.prompt_code));
                    }
                    continue;
                }
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
                // 明确拿到失败、且没有 submit_id → 没花钱。判 fail 而不是悄悄放回
                // ready：错误原文得有地方落，而「处理异常」那一档就是给人看它的。
                // 认领因此不会留下孤儿（run 且无 submit_id 只可能是进程被杀）。
                //
                // **超时是这里唯一的例外**，见 [`submit_error_type`]：那一支的前提
                // （对方说没做成，所以没花钱）在超时上根本不成立。
                let kind = submit_error_type(&e);
                let msg = format!("{e}");
                log.error(
                    "submit",
                    who,
                    if kind == SUBMIT_TIMEOUT {
                        format!("提交超时，已终止 CLI：{msg} 这一条已判到「处理异常」等你核对 —— 请勿直接重跑。")
                    } else {
                        format!("提交失败：{msg}")
                    },
                    None,
                );
                repo::mark_failed(pool, clip.id, kind, &msg, now_unix()).await?;
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

/// 排队采样的保留期。**观测窗口，不是业务真相** —— 排产看的是「最近这段时间队列多快」，
/// 半年前那一周对今晚提交与否毫无参考价值。
pub const QUEUE_SAMPLE_RETENTION_DAYS: i64 = 30;

/// 今天（本地时区）的 `YYYY-MM-DD`。
///
/// 「每天」是**人的**每天。用 UTC 切窗会让北京时间早八点算进前一天，于是每日进账
/// 会周期性地落在错误的格子里，而这份台账的全部意义就是看那个数字稳不稳定
/// （`and_utc()` 把北京时间当 UTC 这条坑，CLAUDE.md 里已经记过一次）。
fn local_day(now: i64) -> String {
    use chrono::{Local, TimeZone};
    match Local.timestamp_opt(now, 0) {
        chrono::LocalResult::Single(dt) => dt.format("%Y-%m-%d").to_string(),
        // 夏令时折叠等边界拿第一个解；拿不到就退回 UTC，宁可错一格也不要漏一天。
        chrono::LocalResult::Ambiguous(dt, _) => dt.format("%Y-%m-%d").to_string(),
        chrono::LocalResult::None => chrono::DateTime::from_timestamp(now, 0)
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_default(),
    }
}

/// 今天还没记过快照就跑一次 `user_credit` 记一条。一天一个进程。
///
/// ## 它在验证什么
///
/// 「即梦每天登录送 80，能不能靠 CLI 自动领」——CLI 的命令面里没有任何领取/签到命令，
/// 所以这件事只剩一个可证伪的假设：**服务端在检测到有效登录态时自动发放**。
/// 这条快照就是那个实验：`delta = 余额涨了多少 + 这期间我们自己花了多少`，
/// 连着几天稳定 ≈ +80 就说明假设成立，那么「保持后台常驻 + 每天调一次 CLI」本身
/// 就是全部实现；≈ 0 则说明必须走网页领取。
///
/// 失败一律静默跳过（`user_credit` 自己会把原因记进执行日志）：掉线/未登录是常态，
/// 而这条台账缺一天只是少一个数据点，不该在界面上变成一个错误。
async fn snapshot_credit_if_new_day(pool: &SqlitePool, bin: &str, log: &Activity) {
    let now = now_unix();
    let day = local_day(now);
    match repo::credit_day(pool, &day).await {
        Ok(Some(_)) => return, // 今天记过了
        Ok(None) => {}
        Err(e) => {
            tracing::warn!(error = %e, "读每日额度台账失败");
            return;
        }
    }
    let Ok(info) = dreamina::user_credit(bin, log).await else {
        return;
    };
    let prev = repo::latest_credit_day(pool).await.ok().flatten();
    // 花掉的那一半从我们自己的账里取（扣费回执），窗口就是「上一条快照到现在」。
    let spent = match &prev {
        Some(p) => repo::credit_since(pool, p.at).await.unwrap_or(0),
        None => 0,
    };
    // 首条没有上一条可比 → delta 留空。**不是 0**：0 是「没进账」这个结论，
    // 而首日我们没有结论。
    let delta = prev.as_ref().map(|p| info.total_credit - p.balance + spent);
    let row = repo::CreditDay {
        day: day.clone(),
        at: now,
        balance: info.total_credit,
        spent_since_prev: spent,
        delta,
    };
    match repo::insert_credit_day(pool, &row).await {
        Ok(true) => log.info(
            "cli",
            None,
            match delta {
                Some(d) => format!(
                    "{day} 额度快照：余额 {}（较上次快照 {}{d} · 期间本机花掉 {spent}）",
                    info.total_credit,
                    if d >= 0 { "+" } else { "" }
                ),
                None => format!(
                    "{day} 额度快照：余额 {}（首条，暂无对比）",
                    info.total_credit
                ),
            },
            None,
        ),
        Ok(false) => {}
        Err(e) => tracing::warn!(error = %e, "写每日额度台账失败"),
    }
}

/// 落一个排队位次采样点，并**只在转折处**写一行日志。
///
/// ## 采样进表，不进日志
///
/// 位次是时序：要用它排产，人问的是「这条队多快」，而那是位次对时间的导数
/// （`queue_trend`）。执行日志答不了这个问题 —— 它是 500 条环形缓冲、重启即清空，
/// 而非 VIP 一轮 600 秒，一条排 14 小时的单光自己就要占掉 84 行，把真正的报错挤出窗口
/// （同 `dreamina::run` 里那条「成功也记 = 每分钟 190 条」的教训）。
///
/// ## 那日志里还剩什么
///
/// 只剩两件**看曲线看不出来**的事：
///
/// - **首次拿到位次** = 这一单确实入队了。它同时是幽灵判定的分水岭，值得留一行时刻。
/// - **位次倒退** = 被重排/重挤了队，罕见且异常，不会刷屏。
///
/// 至于「位次半天不动」，曲线上一条平线比一行字清楚得多，而「队列整体停了」在队列面板
/// 已经由「上次出片 N 前」答过一遍了 —— 同一个信号开第三条通道，就是在制造噪音。
async fn record_position(
    pool: &SqlitePool,
    clip: &repo::ClipRow,
    q: &dreamina::QueryResult,
    now: i64,
    log: &Activity,
) {
    let Some(idx) = q.queue_idx else {
        return;
    };
    // 0 = 已经排到头（实测完成态回 `{queue_idx: 0, queue_status: "Finish"}`）。
    // 它不是队列里的一个位置，画进轨迹会在末尾拖一条冲到底的假斜线。
    if idx <= 0 {
        return;
    }
    let _ = repo::record_queue_sample(pool, clip.id, now, idx, q.queue_length).await;

    let who = Some((clip.id, clip.prompt_code.as_str()));
    match clip.queue_idx {
        None => log.info(
            "poll",
            who,
            match q.queue_length {
                Some(total) if total > 0 => {
                    format!("已入队 · 第 {idx} 位（整条队 {total}）")
                }
                _ => format!("已入队 · 第 {idx} 位"),
            },
            None,
        ),
        Some(prev) if idx > prev => log.warn(
            "poll",
            who,
            format!("排队位次倒退：第 {prev} 位 → 第 {idx} 位（被重排或重新入队）"),
            None,
        ),
        _ => {}
    }
}

/// 提交超时落库时用的 `error_type`。与普通失败分开是**钱的问题**，不是措辞问题。
pub const SUBMIT_TIMEOUT: &str = "submit_timeout";

/// 提交失败 → 落库的 `error_type`。
///
/// 只有两个取值，而它们的差别是这条链路上最贵的一处：
///
/// - `submit`：即梦**明确回了失败**，没有 submit_id，一分钱没扣 → 重跑是安全的。
/// - `submit_timeout`：我们等不下去、把 CLI 杀了，**根本没拿到回答**。那一单可能已经
///   下出去并扣了费，而 submit_id 随进程一起没了 → 直接重跑就是再花一份钱买同一条视频。
///
/// 把两者混成一个 `submit`，界面上就只剩「失败了，重跑吧」这一句话可说 ——
/// 而那正是错的那一半。
fn submit_error_type(e: &AppError) -> &'static str {
    match e {
        AppError::Timeout(_) => SUBMIT_TIMEOUT,
        _ => "submit",
    }
}

/// 落库提交回执，失败带退避重试。
///
/// ## 为什么这一句不能用 `?`
///
/// 拿到回执那一刻钱就已经扣了，而 `submit_id` 是这笔钱**唯一**的凭证：没有它，
/// 那条片子在即梦那边跑完也认不出主人，重跑就是再花一份钱。原来这里是
/// `mark_submitted(...).await?` —— 一次写库失败（盘满、库被锁住）会同时做两件坏事：
/// 把整批剩下的条目全挡住（它们连提交都还没提交），以及让这一条的 submit_id
/// 随着内存一起消失。
///
/// 所以：**先把 submit_id 以 error 级喊出来**（`Activity::error` 同时进环形缓冲与
/// tracing 日志文件，那是失败之后仅剩的凭证），再退避重试几次，仍失败就交给调用方
/// 记账并**继续下一条**。
async fn persist_submit(
    pool: &SqlitePool,
    id: i64,
    receipt: &dreamina::SubmitReceipt,
    log: &Activity,
    who: Option<(i64, &str)>,
) -> Result<(), String> {
    const TRIES: u32 = 3;
    let mut last = String::new();
    for attempt in 1..=TRIES {
        match repo::mark_submitted(pool, id, receipt, now_unix()).await {
            Ok(()) => return Ok(()),
            Err(e) => {
                last = format!("{e}");
                // 第一次就喊，不等重试跑完 —— 进程若在重试途中被杀，这句话就是全部。
                log.error(
                    "submit",
                    who,
                    format!(
                        "提交回执落库失败（第 {attempt}/{TRIES} 次）· submit_id {} · {last}",
                        receipt.submit_id
                    ),
                    None,
                );
                if attempt < TRIES {
                    tokio::time::sleep(Duration::from_millis(200 * u64::from(attempt))).await;
                }
            }
        }
    }
    Err(last)
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
    /// 本轮被即梦以「同时在跑的太多了」弹回本地队列的条数（0028）。**没花钱、不是失败**。
    pub requeued: i64,
    /// 本轮**首次**为某条落库计费证据（扣费额度 / 计费型号）的条数。
    ///
    /// 它必须能触发一次界面刷新：这条数字一变，那一行的「额度」列就从预估变成实收，
    /// 「情况」列也可能从「疑幽灵单」变回「即梦在跑」—— 而在跑条目本身没有阶段变化，
    /// 于是原来这份新证据要一直躺到出片那一刻才会被人看见。
    pub evidence: i64,
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

/// 一条在跑条目的**全部计费证据**：本次回体里的 + 已经落库的。
///
/// ## 为什么必须是一个结构体，而不是「回体里那两个 Option」
///
/// 证据是**累积**的：`mark_swept` 特意用 `COALESCE` 保住已问到的 `credit_count`，
/// 提交回执里的 `submit_credit` 也在 0024 特意落了库 —— 可判定那一侧只看本次回体，
/// 于是攒下来的证据一处都没被读。结果是一条已经扣过钱、只是这一轮回体恰好没带计费
/// 字段的单子会被判成「从未计费」的幽灵单，而界面还会告诉人「重跑不花钱」。
///
/// 规则一句话：**任一处非空 = 这单确实进了即梦，永久免疫幽灵判定**。它只会从
/// 「没证据」变成「有证据」，绝不会反向 —— 所以一条单最多为幽灵嫌疑单查一次。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Evidence {
    /// 本次回体里的计费额度。
    pub fresh_credit: Option<i64>,
    /// 本次回体里的队列位次。
    pub fresh_queue: Option<i64>,
    /// 已落库的计费额度（`v2v_clips.credit_count`，由扫描/出片写入）。
    pub credit: Option<i64>,
    /// 提交回执里的计费额度（`v2v_clips.submit_credit`，0024）。
    pub submit_credit: Option<i64>,
    /// 历史上问到过的队列位次（`v2v_clips.queue_idx`）。
    pub queue: Option<i64>,
}

impl Evidence {
    /// 取库里那一行已经攒下的三处证据（回体那两处由 [`Self::with_fresh`] 补）。
    pub fn from_clip(c: &repo::ClipRow) -> Self {
        Self {
            fresh_credit: None,
            fresh_queue: None,
            credit: c.credit_count,
            submit_credit: c.submit_credit,
            queue: c.queue_idx,
        }
    }

    /// 叠上本次回体带来的两处。
    pub fn with_fresh(mut self, credit: Option<i64>, queue: Option<i64>) -> Self {
        self.fresh_credit = credit;
        self.fresh_queue = queue;
        self
    }

    /// 计费那一路 —— **决定性**信号。健康单从排队第一秒起就带 `credit_count`。
    pub fn billed(&self) -> bool {
        self.fresh_credit.is_some() || self.credit.is_some() || self.submit_credit.is_some()
    }

    /// 队列那一路。单独出现不足以判死，但单独出现足以**免死**。
    pub fn queued(&self) -> bool {
        self.fresh_queue.is_some() || self.queue.is_some()
    }

    /// 有没有任何一处证明这单进了即梦。
    pub fn any(&self) -> bool {
        self.billed() || self.queued()
    }
}

/// 幽灵单判定（纯函数，便于测试）。
///
/// 「幽灵单」= 即梦接了单、给了 submit_id、`list_task` 里也查得到，但**从未入队、
/// 从未计费**：`queue_info` 与 `credit_count` 双双缺席，`gen_status` 永远停在
/// `querying`。2026-07-27 一次提交 19 条中了 18 条，挂了十几个小时无人察觉 ——
/// 因为在 GenDesk 眼里它和「在排队」长得一模一样，而超时默认不限，于是会一直轮询下去。
///
/// 判据要两个信号同时缺席，而不是只看队列位次：
/// - 计费是**决定性**的那个（[`Evidence::billed`]）。健康单从排队第一秒起就带它
///   （实测排队中的 `query_result` 返回 `credit_count: 8`），缺席意味着没进计费。
/// - 队列位次单独缺席不足以判死：万一哪天即梦对某些通道不下发 `queue_info`，
///   只看它会把正在排队、已经扣了钱的任务当场标死。
///
/// 两路都读 [`Evidence`] 而不是只读本次回体 —— 见那个结构体的说明。
///
/// 判定结果是 `fail(phantom)` 而**不是**自动重投：重投要花钱，那是人的决定。
/// submit_id 照样留着（`额度不可撤回`），万一它哪天真的出片，重跑前还查得到。
pub fn is_phantom(gen_status: &str, ev: &Evidence, submitted_at: Option<i64>, now: i64) -> bool {
    phantom_suspect(gen_status, ev, submitted_at, now) && !ev.queued()
}

/// 「像幽灵单，但还差一个信号才敢下结论」。
///
/// 整表扫描（`list_task`）**不回传 `queue_info`**，所以在那条路径上本次回体的
/// `queue_idx` 永远缺席 —— 只凭它判死，等于把两个信号的规则悄悄降成一个。故扫描路径
/// 先用这个宽判据挑出嫌疑，再单发一次 `query_result` 拿队列位次，由 [`is_phantom`]
/// 下最终结论。
///
/// 一旦 [`Evidence::billed`] 成立，这单就确实进了即梦的计费，从此再不入嫌疑名单 ——
/// 这也是扫描机制能把单条查询降到几乎为零的原因。
pub fn phantom_suspect(
    gen_status: &str,
    ev: &Evidence,
    submitted_at: Option<i64>,
    now: i64,
) -> bool {
    dreamina::classify_status(gen_status) == Outcome::Running
        && !ev.billed()
        && submitted_at.is_some_and(|t| now - t > PHANTOM_GRACE_SECS)
}

/// 「这一行现在看着像幽灵单吗」—— 供界面用（没有回体，只看已落库的证据）。
///
/// 界面与判定必须**同一个函数说了算**。前端原来自己抄了一份判据（三个字段 + 一个
/// 手抄的宽限期常量），而它用 `firstSubmittedAt` 算等待时长、Rust 用 `submitted_at`
/// —— 「继续等待」按过一次之后两边就会对同一条给出不同结论。
pub fn clip_looks_phantom(c: &repo::ClipRow, now: i64) -> bool {
    c.stage == "run"
        && !Evidence::from_clip(c).any()
        && c.submitted_at.is_some_and(|t| now - t > PHANTOM_GRACE_SECS)
}

/// 存量修复：把升级前被并发上限判死的条目救回本地队列（一次性，启动时跑）。
///
/// 0028 之前 `ExceedConcurrencyLimit` 一律记成 `fail(provider)`。用户那一批 9 条里
/// 有 8 条就这么躺在「处理异常」——**一分钱没扣、任务从没跑过**，却要人一条条去点
/// 重跑，而重跑还会撞上同一堵墙。只改新逻辑不管存量，等于让这个 bug 的后果留在原地。
///
/// 两道闸与实时路径完全一致：认得出是并发拒收（`dreamina::is_concurrency_reject`），
/// 且没有任何计费证据（`Evidence::billed`）。有回执的那条说明它真花过钱，
/// 那是另一回事，交给人判断。
///
/// 救回来的条目直接进**本地队列**而不是退回「待放行」：这些正是人已经点过确认的那批，
/// 而 0028 的全部意思就是「你点一次，剩下的自动排队接上」。反悔的出口也有了 ——
/// 界面上的「撤回放行」。
pub async fn heal_concurrency_rejects(pool: &SqlitePool, log: &Activity) -> AppResult<i64> {
    let mut healed = 0;
    for clip in repo::list_by_stages(pool, &["fail"]).await? {
        if clip.error_type.as_deref() != Some("provider") {
            continue;
        }
        let reason = clip.error_message.as_deref().unwrap_or_default();
        if !dreamina::is_concurrency_reject(reason) || Evidence::from_clip(&clip).billed() {
            continue;
        }
        let queued_at = clip.first_submitted_at.or(clip.submitted_at).unwrap_or(0);
        if repo::revive_rejected_fail(pool, clip.id, queued_at, now_unix()).await? {
            healed += 1;
        }
    }
    if healed > 0 {
        log.warn(
            "submit",
            None,
            format!(
                "{healed} 条曾被即梦以「同时在跑的太多了」拒收、并被误记成失败的条目已救回本地队列                 （它们从未扣费）。有空位会自动逐条发出去，不想跑就在表格里选中它们「撤回放行」。"
            ),
            None,
        );
    }
    Ok(healed)
}

/// 一轮扫描里最多为几条单独跑一次 `query_result`。
///
/// `list_task` 不带 `queue_info`，且它的 `gen_status` 是本机缓存（见 [`SWEEP_VIP_SECS`]
/// 的实证），所以**位次与状态都只能逐条问**（O(n) 个进程）。并发闸门把在跑条数压到
/// 个位数之后这笔开销可以忽略，但历史库里可能攒着一堆 `run` 条目 —— 这个预算就是那种
/// 情况下的止损线，剩下的下一轮再问。
pub const POSITION_QUERY_BUDGET: i64 = 8;

/// 扫描起点的轮转游标（进程内）。
///
/// ## 没有它，第 9 条起会被永久饿死
///
/// 预算只在「这一条看起来还在跑」时才递减，而 `list_running` 是 `ORDER BY id` 的**稳定**
/// 顺序。于是只要 id 最小的那 8 条一直在跑（非 VIP 排队几小时是常态），它们每一轮都把
/// 预算吃干净，第 9 条起**一次都轮不到** —— 而 `list_task` 给的是过期状态，
/// 它们于是永远停在「已提交」。那些条目**已经扣过费**，片子却永远取不回来。
///
/// 每轮把起点往后挪一个预算的量，任何一条最多等 ⌈n / 预算⌉ 轮必定轮到。
static SWEEP_CURSOR: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

/// 这一轮从在跑列表的第几条开始问（并据此环绕遍历全表）。
///
/// 抽成纯函数是为了能测「轮转确实覆盖得全」—— 那正是它存在的唯一理由，
/// 而在 `sweep_once` 里测要有一个会回话的 CLI。
pub fn sweep_start(cursor: i64, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    cursor.rem_euclid(len as i64) as usize
}

/// 整表扫描一轮 —— **主路径**。
///
/// 一次 `dreamina list_task` 拿回全部在跑任务的状态，逐条落库；只有两种情况才会为
/// 单条再起一个进程：**出片了**（要 `--download_dir` 落盘）与**疑似幽灵单**
/// （要 `queue_info` 才敢判死）。稳态下每轮就是一个进程，与在跑条数无关。
///
/// 扫描里认不出的 submit_id 回落到逐条 `query_result`（带退避）—— 那多半是翻页没覆盖
/// 或 CLI 输出变了，而认不出的条目恰恰最不该被放弃轮询。
///
/// `default_model` = 设置里的默认型号。判「被并发上限弹回来」时要拿它算出这一条走的是
/// 哪条通道 —— 上限是**逐通道**的（0031），认错通道就会把 A 通道的观测记到 B 头上。
pub async fn sweep_once(
    pool: &SqlitePool,
    dirs: &DataDirs,
    bin: &str,
    default_model: &str,
    timeout_secs: Option<i64>,
    log: &Activity,
) -> AppResult<PollSummary> {
    let running = repo::list_running(pool).await?;
    let mut sum = PollSummary::default();
    // **先记这一轮跑过了，再判空**。反过来的话，一个空的在跑集合会让 `LAST_SWEEP`
    // 永远停在 0，于是循环里那句「到点了吗」恒为真：每 6 秒进一次这个分支，
    // 顺带把跟在它后面的自动补单也拉成 6 秒一轮，而队列面板那句「下次查询还有 N 秒」
    // 则永远没有答案（`next_sweep_in` 拿 0 当「还没跑过第一轮」）。
    LAST_SWEEP.store(now_unix(), std::sync::atomic::Ordering::Relaxed);
    if running.is_empty() {
        return Ok(sum);
    }
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

    let mut position_budget = POSITION_QUERY_BUDGET;
    // 环绕遍历，起点每轮往后挪一个预算的量：预算只够问前几条，而固定从 id 最小的那条
    // 开始会把后面的永久饿死（见 [`SWEEP_CURSOR`]）。
    let total = running.len();
    let start = sweep_start(
        SWEEP_CURSOR.fetch_add(POSITION_QUERY_BUDGET, std::sync::atomic::Ordering::Relaxed),
        total,
    );
    for clip in running.iter().cycle().skip(start).take(total) {
        let Some(submit_id) = clip.submit_id.clone() else {
            continue;
        };
        let now = now_unix();
        match found.get(&submit_id) {
            Some(q) => {
                // 「这一轮才第一次拿到计费证据」——库里那行还是空的，回体给了。
                // 记下来是为了让这一轮结束后推一次刷新（见 `PollSummary::evidence`）。
                if (clip.credit_count.is_none() && q.credit_count.is_some())
                    || (clip.benefit_type.is_none() && q.benefit_type.is_some())
                {
                    sum.evidence += 1;
                }
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
                // 排队位次只有 `query_result` 给得出，而它恰恰是排队几小时时**唯一**
                // 有意义的进度：「第 4485 位」与「第 12 位」是两件完全不同的事，
                // 而在此之前界面上两者都只说得出一句「即梦在跑」。
                //
                // 之所以现在负担得起：在跑条数已被并发闸门压到个位数。仍留预算上限 ——
                // 历史上攒下的一堆在跑条目不该把一轮扫描重新变成 O(n) 个进程。
                let mut q = q.clone();
                let mut authoritative = false;
                if position_budget > 0
                    && dreamina::classify_status(&q.gen_status) == Outcome::Running
                {
                    position_budget -= 1;
                    let who = Some((clip.id, clip.prompt_code.as_str()));
                    if let Ok(full) = dreamina::query(bin, &submit_id, None, log, who).await {
                        // 采样要在 `mark_polled` **之前**：那一句会把 clip 行里的
                        // 上一个位次覆盖掉，而「与上次比是不是倒退了」正要拿它作比较。
                        record_position(pool, clip, &full, now, log).await;
                        let _ =
                            repo::mark_polled(pool, clip.id, &full.gen_status, full.queue_idx, now)
                                .await;
                        // 位次问到了 = 这份回体是权威的，settle 里那次幽灵确认查询
                        // 就不必再发一遍。
                        q = full;
                        authoritative = true;
                    }
                }
                settle(
                    pool,
                    dirs,
                    bin,
                    default_model,
                    clip,
                    q,
                    timeout_secs,
                    authoritative,
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
                query_and_settle(
                    pool,
                    dirs,
                    bin,
                    default_model,
                    clip,
                    timeout_secs,
                    log,
                    &mut sum,
                )
                .await?;
            }
        }
    }
    Ok(sum)
}

/// 人点了顶栏那个「刷新」—— 把**全部依赖即梦回传的数值**现在就问一遍。
///
/// ## 为什么它不能是「跑一次 `sweep_once`」
///
/// `sweep_once` 的主路径是 `list_task`，而那玩意儿**读的是本机缓存**
/// （`~/.dreamina_cli/tasks.db`，见 [`SWEEP_VIP_SECS`] 上那段实证），并且**不带
/// `queue_info`**。人按这个按钮想知道的三件事恰好全在它给不出的那一侧：
///
/// - 这条到底跑完了还是还在排队 → `gen_status` 的**服务端**真相
/// - 即梦队列里前面还有几个 → `queue_idx`
/// - 到底扣了多少 → `credit_count`
///
/// 三者都只有逐条 `query_result` 才有。所以这里是 O(n) 个进程，**没有退避、没有
/// [`POSITION_QUERY_BUDGET`]**：预算是给后台循环省钱的，人按下按钮就是要现在全问一遍。
/// n 是**在跑条数**（受逐通道并发上限压着，稳态是个位数），不是本地队列长度 ——
/// 本地队列里那些即梦压根不知道，没什么可问的。
///
/// ## 进度用回调而不是 `AppHandle`
///
/// 这一轮可能要跑几十秒，期间界面必须能显示「正在查 12/78」并逐条走字，否则跟死机
/// 没有区别。但把 `AppHandle` 收进来会让这个函数没法脱离 Tauri 测试，故收一个回调，
/// 由命令层负责把它翻译成事件。
///
/// 结束时重置 [`LAST_SWEEP`]：手动问过一整轮之后，「下次查询还有 N 秒」理应重新计时，
/// 否则面板上那个倒计时会与刚刚发生的事对不上。
pub async fn refresh_now<F>(
    pool: &SqlitePool,
    dirs: &DataDirs,
    bin: &str,
    default_model: &str,
    timeout_secs: Option<i64>,
    log: &Activity,
    mut on_step: F,
) -> AppResult<PollSummary>
where
    F: FnMut(i64, i64, &PollSummary),
{
    let running = repo::list_running(pool).await?;
    let mut sum = PollSummary::default();
    // 与 `sweep_once` 同样的理由：**先记跑过了再判空**，否则空集合会把 `LAST_SWEEP`
    // 永远留在 0，循环里那句「到点了吗」恒为真。
    LAST_SWEEP.store(now_unix(), std::sync::atomic::Ordering::Relaxed);
    let total = running.len() as i64;
    on_step(0, total, &sum);
    if running.is_empty() {
        return Ok(sum);
    }
    std::fs::create_dir_all(dirs.clips())?;

    let mut done = 0;
    for clip in &running {
        if clip.submit_id.is_some() {
            query_and_settle(
                pool,
                dirs,
                bin,
                default_model,
                clip,
                timeout_secs,
                log,
                &mut sum,
            )
            .await?;
        }
        done += 1;
        on_step(done, total, &sum);
    }
    Ok(sum)
}

/// 单条 `query_result` → 落库 → 定态处置。
#[allow(clippy::too_many_arguments)] // 只是 settle 的转发层，参数表跟着它走
async fn query_and_settle(
    pool: &SqlitePool,
    dirs: &DataDirs,
    bin: &str,
    default_model: &str,
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
    // 采样在落库之前（理由同 `sweep_once` 里那处：要拿库里的上一个位次做对比）。
    record_position(pool, clip, &q, now, log).await;
    // 状态原文落库：切页/重启后看板仍答得出「这条在排队还是在跑」。
    let _ = repo::mark_polled(pool, clip.id, &q.gen_status, q.queue_idx, now).await;
    settle(
        pool,
        dirs,
        bin,
        default_model,
        clip,
        q,
        timeout_secs,
        true,
        log,
        sum,
    )
    .await
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
    default_model: &str,
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
            // 「同时在跑的太多了」不是失败，是**没排上**：一分钱没扣、任务从没跑过。
            // 判死它等于让一条只是排在后面的片子躺进「处理异常」，而人点重跑又会撞
            // 同一堵墙。放回本地队列，由空位腾出来时自动补上。
            //
            // `billed()` 是硬闸门：万一哪天即梦收了钱又回这个 reason，那就是一条
            // 真花过钱的单，清掉它的 submit_id 等于把凭证扔了 —— 交给人判断。
            let ev = Evidence::from_clip(clip);
            if dreamina::is_concurrency_reject(&reason) && !ev.billed() {
                // 上限是**逐通道**的（0031）：这一单撞的是它自己那条通道的墙，
                // 别的通道有没有空位与此无关。收敛与文案都必须点名通道，否则
                // 「即梦同时只跑得下 1 条」会被读成账户级的结论 —— 那正是上一版
                // 把 2.0mini 上 6 条一起锁死的那个误解。
                let channel = channel_of(clip.model_version.as_deref(), default_model);
                let accepted =
                    repo::count_running_accepted_on(pool, default_model, channel).await?;
                let limit = observe_concurrency_reject(channel, accepted);
                let queued_at = clip
                    .submit_queued_at
                    .or(clip.first_submitted_at)
                    .unwrap_or(now);
                repo::requeue_after_reject(pool, clip.id, queued_at, now).await?;
                log.warn(
                    "submit",
                    who,
                    format!(
                        "{} 通道同时只跑得下 {limit} 条，这一单没排上（未扣费）→ 已放回本地队列，\
                         有空位就自动补上。其它通道不受影响。",
                        dreamina::short_label(channel)
                    ),
                    None,
                );
                sum.requeued += 1;
                return Ok(());
            }
            log.error("poll", who, format!("即梦判定失败：{reason}"), None);
            repo::mark_failed(pool, clip.id, "provider", &reason, now).await?;
            sum.failed += 1;
        }
        Outcome::Running => {
            // 嫌疑升级：扫描路径拿不到队列位次，要判死之前先单查一次拿准。
            // 这一步每条最多发生一次 —— 一旦看到计费回执或队列位次，这条就再不入嫌疑名单。
            let mut q = q;
            let mut authoritative = queue_authoritative;
            // 证据 = 已落库的三处 + 本次回体的两处。只读回体那两处，等于把
            // `mark_swept` 特意保住的、`0024` 特意落库的证据全都白攒了。
            let mut ev = Evidence::from_clip(clip).with_fresh(q.credit_count, q.queue_idx);
            if !authoritative && phantom_suspect(&q.gen_status, &ev, clip.submitted_at, now) {
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
                            ev = ev.with_fresh(
                                full.credit_count.or(q.credit_count),
                                full.queue_idx.or(q.queue_idx),
                            );
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
            if authoritative && is_phantom(&q.gen_status, &ev, clip.submitted_at, now) {
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
    let phantom = repo::count_phantom_suspects(pool, now_unix() - PHANTOM_GRACE_SECS).await?;
    let mut c = StageCounts::from_rows(&repo::stage_counts(pool).await?).with_phantom(phantom);
    c.undelivered = repo::count_pass_undelivered(pool).await?;
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
        // 存量修复：0028 之前被并发上限误判成失败的条目救回队列。
        if let Err(e) = heal_concurrency_rejects(&pool, &log).await {
            log.error("submit", None, format!("并发拒收存量修复失败：{e}"), None);
        }
        // 「上一轮扫描的时刻」用全局 `LAST_SWEEP` 而不是循环局部变量：提交成功后会调
        // `request_sweep_soon` 把它往前挪来请求补扫，局部变量收不到那个请求。
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
                last_sweep_at: last_sweep_at(),
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
            // 每日额度快照。**在 `poll_enabled` 判断之前**：它与轮询是两件事，
            // 关掉轮询（比如今天不想跑批）不该把台账也停掉 —— 那正是「不跑批的那一天
            // 有没有照样进账 80」这个对照组最需要的数据。
            snapshot_credit_if_new_day(&pool, &settings.bin, &log).await;
            if !settings.poll_enabled {
                tick.enabled = false;
                let _ = tick.emit(&app);
                continue;
            }
            // 档位由**在跑集合里有没有 VIP** 决定，每轮重算：VIP 1–3 分钟出片，
            // 非 VIP 排队几小时，拿同一个节拍问这两种任务对谁都不合适。
            // 每 6 秒一次的 SELECT 是内存级开销，与起一个进程不在一个量级。
            let any_vip = repo::running_models(&pool)
                .await
                .map(|ms| {
                    ms.iter().any(|m| {
                        dreamina::is_vip(
                            m.as_deref()
                                .filter(|s| !s.trim().is_empty())
                                .unwrap_or(settings.model_version.as_str()),
                        )
                    })
                })
                .unwrap_or(false);
            let every = sweep_interval(any_vip);
            SWEEP_EVERY.store(every, std::sync::atomic::Ordering::Relaxed);

            // 心跳每 6 秒发一次（内存读），真正去问即梦是 5/10 分钟一次（一个进程）。
            let last_sweep = LAST_SWEEP.load(std::sync::atomic::Ordering::Relaxed);
            if last_sweep == 0 || tick.at - last_sweep >= every {
                match sweep_once(
                    &pool,
                    &dirs,
                    &settings.bin,
                    &settings.model_version,
                    settings.timeout_secs(),
                    &log,
                )
                .await
                {
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
                        // 计费证据首次落库也要刷新：那一行的额度列会从「预估」变成
                        // 「实收」，「情况」列也可能从「疑幽灵单」变回「即梦在跑」，
                        // 而在跑条目本身没有阶段变化，不推就要躺到出片那一刻才被看见。
                        if sum.finished > 0
                            || sum.failed > 0
                            || sum.evidence > 0
                            || sum.requeued > 0
                        {
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
                // 本地待发队列往前推 —— **排在自动补单之前**：人已经放行过的那些
                // 是明确的意图，理应先于补单器随手挑的那些占用空位。
                //
                // 放在扫描之后：扫描刚刚才把出片/判死/被弹回的条目挪出 `run`，
                // 此刻数出来的空位才是准的。
                match drain_queue(
                    &pool,
                    &settings.bin,
                    &settings.defaults(),
                    settings.max_in_flight,
                    &log,
                )
                .await
                {
                    Ok(s) if s.submitted > 0 => {
                        request_sweep_soon(now_unix());
                        crate::commands::v2v::emit_changed(&pool, &app, None).await;
                    }
                    Err(e) => log.error("submit", None, format!("本地队列补位失败：{e}"), None),
                    _ => {}
                }
                // 自动补单跟着扫描的节拍走：它要看的「在跑几条」正是扫描刚更新过的。
                super::autofill::tick(&pool, &settings, &app, &log, &mut last_refill).await;
            }
            // 重读一次：这一轮若真跑了扫描，`LAST_SWEEP` 已经被推到当下，而心跳载荷是在
            // 扫描**之前**填的。不重读的话，顶栏那句「上次查询 N 前」会整整慢一拍
            // ——刚扫完却显示「10 分钟前」，正是这颗按钮要消灭的那种误导。
            tick.last_sweep_at = last_sweep_at();
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
            export_path: None,
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
            submit_queued_at: None,
            updated_at: 0,
            prompt_code: "GG-0001".into(),
            image_path: "/img.jpg".into(),
            thumb_path: "/thumb.jpg".into(),
            accepted_at: 0,
        }
    }

    /// 一个带 v2v 表的临时库 + 一条走到 `ready` 的 clip。
    ///
    /// 这几条测试要跑的是**放行链路本身**（闸门、队列、失败归属），而不是 CLI ——
    /// 故 `bin` 一律给一个不存在的路径：`dreamina::resolve_bin` 会在花掉任何钱之前
    /// 就失败，于是提交那一步的错误分支被完整走一遍，而且绝不会真的发出请求。
    const NO_SUCH_BIN: &str = "/nonexistent/dreamina-for-tests";

    async fn seed_ready(pool: &SqlitePool, n: i64) -> Vec<i64> {
        for w in 1..=n {
            sqlx::query(
                "INSERT INTO accepted_works (id,image_path,thumb_path,prompt_text,accepted_at,prompt_code,group_name)
                 VALUES (?1,'/img.jpg','/thumb.jpg','原文',100,'GG-0001','g')",
            )
            .bind(w)
            .execute(pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO v2v_clips (work_id, group_name, stage, source_prompt, variable_part,
                                        video_prompt, created_at, updated_at)
                 VALUES (?1,'g','ready','原文','',  '视频提示词', 100, 100)",
            )
            .bind(w)
            .execute(pool)
            .await
            .unwrap();
        }
        repo::list_by_stages(pool, &["ready"])
            .await
            .unwrap()
            .iter()
            .map(|c| c.id)
            .collect()
    }

    fn opts() -> GenOpts {
        GenOpts {
            model_version: Some("seedance2.0fast".into()),
            duration: Some(4),
            video_resolution: Some("720p".into()),
            session: None,
        }
    }

    // 这一版的核心行为：**放行 9 条 ≠ 发出去 9 条**。
    //
    // 事故就是这么来的 —— 9 条一起砸向即梦，即梦只跑得下 1 条，其余 8 条回来
    // `ExceedConcurrencyLimit` 被判死进「处理异常」。闸门必须在**发出去之前**生效。
    #[tokio::test]
    async fn releasing_a_batch_sends_only_what_fits_and_queues_the_rest() {
        let (pool, _d) = crate::db::test_support::test_pool().await;
        let ids = seed_ready(&pool, 9).await;
        let log = Activity::silent();

        let sum = release_and_submit(&pool, NO_SUCH_BIN, &ids, &opts(), 1, &log)
            .await
            .unwrap();

        // 只动了 1 条（CLI 不存在 → 它提交失败），其余 8 条一条都没被碰过。
        assert_eq!(sum.submitted, 0, "假 CLI 发不出去");
        assert_eq!(sum.failed, 1, "闸门只放行了一条，所以只有一条会失败");
        assert_eq!(repo::count_submit_queued(&pool).await.unwrap(), 8);
        assert_eq!(sum.queued, 8);

        // 发不出去的绝不能卡在 `run`：认领了就要对它负责到底，
        // 否则它要等下次启动的孤儿恢复才回得来。
        assert!(repo::list_by_stages(&pool, &["run"])
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            repo::list_by_stages(&pool, &["fail"]).await.unwrap().len(),
            1
        );
    }

    // 位次采样必须**只进表、不进日志**，转折处才写一行。
    //
    // 反过来做（每轮记一条）的代价在 `dreamina::run` 的注释里已经算过一次：
    // 非 VIP 一轮 600 秒，一条排 14 小时的单光自己就要占掉 84 行，
    // 而执行日志只有 500 条 —— 真正的报错会被「一切正常」挤出窗口。
    #[tokio::test]
    async fn positions_go_into_the_table_and_only_turning_points_go_into_the_log() {
        let (pool, _d) = crate::db::test_support::test_pool().await;
        let ids = seed_ready(&pool, 1).await;
        let id = ids[0];
        let log = Activity::silent();

        let q = |idx: Option<i64>| dreamina::QueryResult {
            gen_status: "Queueing".into(),
            queue_idx: idx,
            queue_length: Some(574_522),
            ..Default::default()
        };
        // 只有 id 与「上一次问到的位次」参与判断，其余字段用现成的空壳。
        let clip_at = |queue_idx: Option<i64>| repo::ClipRow {
            id,
            queue_idx,
            ..clip(None, None, None)
        };

        // ① 首次拿到位次 → 一行「已入队」。
        record_position(&pool, &clip_at(None), &q(Some(4485)), 1_000, &log).await;
        // ② 正常推进的后续三轮 → 一行都不该多。
        record_position(&pool, &clip_at(Some(4485)), &q(Some(4200)), 1_600, &log).await;
        record_position(&pool, &clip_at(Some(4200)), &q(Some(3900)), 2_200, &log).await;
        record_position(&pool, &clip_at(Some(3900)), &q(Some(3600)), 2_800, &log).await;

        let samples = repo::queue_samples_of(&pool, id).await.unwrap();
        assert_eq!(samples.len(), 4, "四轮四个采样点，全部进表");
        assert_eq!(samples[0].queue_idx, 4485);
        assert_eq!(samples[0].queue_length, Some(574_522));
        assert_eq!(
            log.snapshot().len(),
            1,
            "只有「首次入队」那一行，正常推进不写日志：{:?}",
            log.snapshot()
        );
        assert!(log.snapshot()[0].message.contains("已入队"));

        // ③ 位次倒退是真异常（被重排），罕见且必须出声。
        record_position(&pool, &clip_at(Some(3600)), &q(Some(9000)), 3_400, &log).await;
        let snap = log.snapshot();
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[1].level, "warn");
        assert!(snap[1].message.contains("倒退"), "{}", snap[1].message);
    }

    // 完成态的 `queue_idx: 0` 不是「排在第 0 位」，它是「已经不在队里了」。
    // 混进轨迹会在末尾拖出一条冲到底的假斜线，而那条线会让速度估算凭空翻几倍。
    #[tokio::test]
    async fn the_zero_position_of_a_finished_task_is_not_a_queue_position() {
        let (pool, _d) = crate::db::test_support::test_pool().await;
        let id = seed_ready(&pool, 1).await[0];
        let log = Activity::silent();
        let finished = dreamina::QueryResult {
            gen_status: "Finish".into(),
            queue_idx: Some(0),
            queue_length: Some(0),
            ..Default::default()
        };
        let row = repo::ClipRow {
            id,
            ..clip(None, None, None)
        };
        record_position(&pool, &row, &finished, 1_000, &log).await;
        assert!(repo::queue_samples_of(&pool, id).await.unwrap().is_empty());
        assert!(log.snapshot().is_empty());
    }

    // 采样是诊断数据，不是业务真相：同一秒重复写（补扫与定时扫在边界上撞到一起）
    // 必须是 no-op，而不是一个要往上冒泡、把这一轮轮询打断的错误。
    #[tokio::test]
    async fn duplicate_samples_at_the_same_second_are_ignored_not_fatal() {
        let (pool, _d) = crate::db::test_support::test_pool().await;
        let id = seed_ready(&pool, 1).await[0];
        repo::record_queue_sample(&pool, id, 500, 4485, None)
            .await
            .unwrap();
        repo::record_queue_sample(&pool, id, 500, 4400, None)
            .await
            .unwrap();
        let s = repo::queue_samples_of(&pool, id).await.unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].queue_idx, 4485, "先写的那个留下");
    }

    // 「每天」是**人的**每天。用 UTC 切会让北京时间早八点算进前一天，
    // 于是每日进账会周期性地落进错误的格子，而这份台账的全部意义就是看那个数稳不稳。
    #[test]
    fn the_day_boundary_follows_the_local_clock() {
        use chrono::{Local, TimeZone};
        // 本地时间当天 00:00:30 与 23:59:30 必须是同一天，且等于本地日历上的那一天。
        let base = Local
            .with_ymd_and_hms(2026, 7, 28, 0, 0, 30)
            .single()
            .expect("本地时间应可构造");
        let late = Local
            .with_ymd_and_hms(2026, 7, 28, 23, 59, 30)
            .single()
            .expect("本地时间应可构造");
        assert_eq!(local_day(base.timestamp()), "2026-07-28");
        assert_eq!(local_day(late.timestamp()), "2026-07-28");
        // 跨过本地零点就必须换一天。
        assert_eq!(local_day(late.timestamp() + 60), "2026-07-29");
    }

    // 提交超时与提交失败**必须落成两种 error_type**。
    //
    // 它们在界面上指挥的是相反的动作：失败 → 重跑（没花钱）；超时 → 先去即梦核对
    // （可能已经扣了费，而 submit_id 随被杀的进程没了）。混成一个 `submit`，
    // 「处理异常」那一档就只说得出「失败了，重跑吧」——对超时那一半恰好是最贵的建议。
    #[test]
    fn a_submit_timeout_is_not_filed_as_an_ordinary_failure() {
        assert_eq!(
            submit_error_type(&AppError::Timeout("卡住了".into())),
            SUBMIT_TIMEOUT
        );
        for e in [
            AppError::Internal("即梦 CLI 返回失败（1）".into()),
            AppError::InvalidInput("首帧图不存在".into()),
            AppError::Io("启动即梦 CLI 失败".into()),
        ] {
            assert_eq!(submit_error_type(&e), "submit", "{e}");
        }
    }

    // 位子被占满时一条都不发 —— 包括「占位的是别人放行的」这种情况。
    #[tokio::test]
    async fn a_full_pipe_sends_nothing_but_still_queues_everything() {
        let (pool, _d) = crate::db::test_support::test_pool().await;
        let ids = seed_ready(&pool, 3).await;
        let log = Activity::silent();
        // 先让一条进 `run`（模拟已经在即梦手上的那一条）。
        repo::mark_submitted(
            &pool,
            ids[0],
            &dreamina::SubmitReceipt::healthy("busy", 8),
            400,
        )
        .await
        .unwrap();

        let ch = opts().model_version.unwrap_or_default();
        assert_eq!(free_slots(&pool, &ch, &ch, 1).await.unwrap(), 0);
        let sum = release_and_submit(&pool, NO_SUCH_BIN, &ids[1..], &opts(), 1, &log)
            .await
            .unwrap();
        assert_eq!(
            (sum.submitted, sum.failed),
            (0, 0),
            "一条都不发，也不算失败"
        );
        assert_eq!(sum.queued, 2, "两条原样排着，等那一条出片");

        // 空位为 0 时 drain 直接返回，不去动库里任何一行。
        let again = drain_queue(&pool, NO_SUCH_BIN, &opts(), 1, &log)
            .await
            .unwrap();
        assert_eq!(again.submitted + again.failed, 0);
        assert_eq!(repo::count_submit_queued(&pool).await.unwrap(), 2);
    }

    /// 一条通道排满了，**另一条通道照发**（0031）。
    ///
    /// 这条测试守的是这一版的核心不变量，而它正是上一版真实发生的故障：库里 78 条
    /// 2.0fast 排着队、1 条在跑，另有 6 条 2.0mini 一条都发不出去 —— 而 2.0mini
    /// 那条队从头到尾是空的。判据来自即梦自己的回体：`queue_info.debug_info` 里的
    /// `dreamina_matrix_queue_name` 逐通道不同，2.0fast 与 2.0mini 是两条队。
    #[tokio::test]
    async fn a_full_channel_does_not_block_a_different_one() {
        let (pool, _d) = crate::db::test_support::test_pool().await;
        let ids = seed_ready(&pool, 3).await;
        let log = Activity::silent();
        let default_model = opts().model_version.unwrap_or_default();

        // 第三条走另一条通道。
        repo::set_params(&pool, &[ids[2]], Some("seedance2.0mini"), None, None, 300)
            .await
            .unwrap();
        // 默认通道上占满唯一那个位子。
        repo::mark_submitted(
            &pool,
            ids[0],
            &dreamina::SubmitReceipt::healthy("busy", 8),
            400,
        )
        .await
        .unwrap();
        repo::mark_submit_queued(&pool, &[ids[1], ids[2]], 401)
            .await
            .unwrap();

        assert_eq!(
            free_slots(&pool, &default_model, &default_model, 1)
                .await
                .unwrap(),
            0,
            "默认通道满了"
        );
        assert_eq!(
            free_slots(&pool, &default_model, "seedance2.0mini", 1)
                .await
                .unwrap(),
            1,
            "mini 那条通道一条都没在跑，必须还有空位"
        );

        // drain 会为 mini 那条真的去调 CLI（这里的 bin 不存在，故记一次失败而不是
        // 静静跳过）—— 断言的是「它确实动了 mini 那条」，而不是别的通道。
        let sum = drain_queue(&pool, NO_SUCH_BIN, &opts(), 1, &log)
            .await
            .unwrap();
        assert_eq!(sum.failed, 1, "只该动 mini 那一条：默认通道没空位，mini 有");
        assert_eq!(
            repo::pick_submit_queued_on(&pool, &default_model, &default_model, 9)
                .await
                .unwrap(),
            vec![ids[1]],
            "默认通道那条原样排着，没被牵连"
        );
    }

    // 存量修复：升级前被并发上限误判成 fail 的条目要能自己回到队列。
    //
    // 只改新逻辑不管存量，等于让这个 bug 的后果留在原地 —— 用户那 8 条会一直
    // 躺在「处理异常」，而它们从未扣费。
    #[tokio::test]
    async fn old_rejects_are_healed_back_into_the_queue_but_billed_ones_are_left_alone() {
        let (pool, _d) = crate::db::test_support::test_pool().await;
        let ids = seed_ready(&pool, 3).await;
        let log = Activity::silent();

        // ① 典型受害者：被并发上限拒掉、没有任何计费回执。
        repo::mark_submitted(&pool, ids[0], &dreamina::SubmitReceipt::bare("a"), 400)
            .await
            .unwrap();
        // ② 同样的错误原文，但**扣过钱** —— 那是另一回事，交给人判断。
        repo::mark_submitted(
            &pool,
            ids[1],
            &dreamina::SubmitReceipt::healthy("b", 8),
            400,
        )
        .await
        .unwrap();
        // ③ 真失败：内容不合规。碰它就等于把一条坏片子塞回队列无限重投。
        repo::mark_submitted(&pool, ids[2], &dreamina::SubmitReceipt::bare("c"), 400)
            .await
            .unwrap();
        let reject = "api error: ret=1310, message=ExceedConcurrencyLimit, logid=x";
        repo::mark_failed(&pool, ids[0], "provider", reject, 500)
            .await
            .unwrap();
        repo::mark_failed(&pool, ids[1], "provider", reject, 500)
            .await
            .unwrap();
        repo::mark_failed(&pool, ids[2], "provider", "content policy violation", 500)
            .await
            .unwrap();

        assert_eq!(heal_concurrency_rejects(&pool, &log).await.unwrap(), 1);
        assert_eq!(
            repo::pick_submit_queued_on(&pool, "seedance2.0fast", "seedance2.0fast", 9)
                .await
                .unwrap(),
            vec![ids[0]]
        );
        let still_failed: Vec<i64> = repo::list_by_stages(&pool, &["fail"])
            .await
            .unwrap()
            .iter()
            .map(|c| c.id)
            .collect();
        assert_eq!(still_failed, vec![ids[1], ids[2]]);

        // 幂等：再跑一遍没有第二个受害者（启动时每次都会跑）。
        assert_eq!(heal_concurrency_rejects(&pool, &log).await.unwrap(), 0);
    }

    // 在跑上限：配置值与实测值取小，实测值只降不升，且**逐通道各记各的**。
    //
    // **整条链路写在一个测试里**是有意的：`OBSERVED` 是进程级静态量，拆成几个并行跑的
    // 测试会互相污染 —— 一个把它压到 1，另一个正好在断言 5。通道名也为此取得独一无二，
    // 免得与别处的测试用例撞车。
    #[test]
    fn in_flight_limit_clamps_to_both_the_setting_and_what_dreamina_actually_allows() {
        let a = "test-channel-a";
        let b = "test-channel-b";
        // 还没撞过墙 → 完全听配置的，但受硬护栏夹取。
        assert_eq!(effective_in_flight(a, 5), 5);
        assert_eq!(effective_in_flight(a, 0), 1, "0 条在跑等于这条链路停摆");
        assert_eq!(effective_in_flight(a, 9999), MAX_IN_FLIGHT_CAP);
        assert_eq!(observed_in_flight_limit(a), None);

        // 撞墙：即梦当时在 a 上只收下了 1 条 → 从此 a 的上限就是 1，配置再大也没用。
        assert_eq!(observe_concurrency_reject(a, 1), 1);
        assert_eq!(observed_in_flight_limit(a), Some(1));
        assert_eq!(effective_in_flight(a, 5), 1);

        // **通道之间互不相干**（0031 的核心）：a 撞了墙，b 该照跑不误。
        // 反过来（一处观测压住全部通道）正是 2.0fast 那条长队把 2.0mini 锁死的成因。
        assert_eq!(observed_in_flight_limit(b), None);
        assert_eq!(effective_in_flight(b, 5), 5);

        // 只降不升：又一次观测到 3 不该把上限放宽回去（那会让空转重新开始）。
        observe_concurrency_reject(a, 3);
        assert_eq!(effective_in_flight(a, 5), 1);

        // 0 条被收下时兜底为 1：判成 0 会让这条通道永久停摆，
        // 而「一条都跑不了」几乎一定是别的故障，不该由这里下结论。
        assert_eq!(observe_concurrency_reject(a, 0), 1);
    }

    /// 扫描起点的轮转必须**覆盖全表**，否则预算之外的条目永远问不到。
    ///
    /// 这条测试守的是一个会花钱的故障：预算只在「看起来还在跑」时递减，而在跑列表是
    /// `ORDER BY id` 的稳定顺序 —— 固定从头开始的话，只要前 8 条一直在排队（非 VIP 排
    /// 几小时是常态），第 9 条起一次都轮不到。而 `list_task` 给的是本机缓存里的过期状态，
    /// 于是那些**已经扣过费**的条目会永远停在「已提交」，片子取不回来。
    #[test]
    fn the_scan_cursor_rotates_so_no_clip_is_starved() {
        let len = 21usize; // 21 条在跑，预算 8 → 三轮该覆盖完
        let mut seen = std::collections::HashSet::new();
        let mut cursor = 0i64;
        for _ in 0..3 {
            let start = sweep_start(cursor, len);
            for k in 0..POSITION_QUERY_BUDGET as usize {
                seen.insert((start + k) % len);
            }
            cursor += POSITION_QUERY_BUDGET;
        }
        assert_eq!(seen.len(), len, "三轮之后每一条都该被问到过一次");

        // 空表不得取模除零。
        assert_eq!(sweep_start(7, 0), 0);
        // 游标一直增长也不会跑出界（`rem_euclid` 对负数同样安全，防的是将来溢出回绕）。
        assert!(sweep_start(i64::MAX, 5) < 5);
        assert!(sweep_start(-3, 5) < 5);
    }

    // 通道归属：条目自己写死的型号优先，没写就落到设置里的默认型号。
    // 与 `repo::CHANNEL_OF` 那段 SQL 同口径 —— 两边分叉就会数着 A 通道的空位往 B 发单。
    #[test]
    fn channel_falls_back_to_the_configured_default_only_when_unset() {
        assert_eq!(
            channel_of(Some("seedance2.0mini"), "seedance2.0fast"),
            "seedance2.0mini"
        );
        assert_eq!(channel_of(None, "seedance2.0fast"), "seedance2.0fast");
        assert_eq!(channel_of(Some("  "), "seedance2.0fast"), "seedance2.0fast");
    }

    // 默认往小了猜：猜小只是让后面那些多等一会儿（非 VIP 通道上「等」本来就免费），
    // 猜大是一批片子集体躺进「处理异常」。
    #[test]
    fn default_in_flight_is_the_measured_truth() {
        assert_eq!(DEFAULT_MAX_IN_FLIGHT, 1);
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

    /// 本次回体带来的证据（库里那行什么都没有）—— 旧签名的等价物。
    fn fresh(credit: Option<i64>, queue: Option<i64>) -> Evidence {
        Evidence::default().with_fresh(credit, queue)
    }

    // 幽灵单：即梦接了单却没入队，`queue_idx` 与 `credit_count` 双双缺席。
    // 2026-07-27 一次提交 19 条中了 18 条，而「超时默认不限」意味着没人会去打断它。
    #[test]
    fn phantom_needs_both_signals_missing_and_the_grace_period_over() {
        let late = PHANTOM_GRACE_SECS + 1;
        assert!(
            is_phantom("querying", &fresh(None, None), Some(0), late),
            "两个信号都缺 + 过了宽限期 = 幽灵单"
        );
    }

    // 宽限期内不判：健康单实测 25 秒内就拿到位次，但网络慢一点也不该被当场标死。
    #[test]
    fn phantom_is_not_judged_inside_the_grace_period() {
        assert!(!is_phantom(
            "querying",
            &fresh(None, None),
            Some(0),
            PHANTOM_GRACE_SECS
        ));
    }

    // **单看队列位次不足以判死**。万一哪天即梦对某些通道不下发 queue_info，
    // 只凭它会把正在排队、钱已经扣了的任务当场标死 —— 那是不可逆的误伤。
    #[test]
    fn credit_receipt_alone_saves_a_clip_from_being_judged_phantom() {
        assert!(
            !is_phantom("querying", &fresh(Some(8), None), Some(0), 999_999),
            "有计费回执就说明真进了即梦，不得判幽灵"
        );
        assert!(
            !is_phantom("querying", &fresh(None, Some(4485)), Some(0), 999_999),
            "有队列位次同样不得判幽灵"
        );
    }

    // 终态不归幽灵管：出片的走 Done、判失败的走 Failed，各有各的处置。
    #[test]
    fn phantom_only_applies_to_running_clips() {
        assert!(!is_phantom("success", &fresh(None, None), Some(0), 999_999));
        assert!(!is_phantom("expired", &fresh(None, None), Some(0), 999_999));
    }

    /// **已落库的计费证据同样免死**。这是「证据落库了但没人读」那个系统性缺口的核心：
    /// `mark_swept` 特意 COALESCE 保住的 `credit_count`、0024 特意落库的 `submit_credit`、
    /// 以及历史上问到过的 `queue_idx`，判定那一侧原来一处都没读。
    ///
    /// 一正一反并排：同样是「本次回体双双缺席 + 过了宽限期」，只差库里有没有攒下证据。
    #[test]
    fn persisted_evidence_is_as_good_as_a_fresh_receipt() {
        let late = PHANTOM_GRACE_SECS + 1;
        // 反：从来没有任何一处证据 → 判幽灵。
        assert!(
            is_phantom("querying", &Evidence::default(), Some(0), late),
            "五处证据全空才是幽灵单"
        );
        // 正：三处已落库的证据，各自单独都足以免死。
        for ev in [
            Evidence {
                credit: Some(8),
                ..Default::default()
            },
            Evidence {
                submit_credit: Some(8),
                ..Default::default()
            },
            Evidence {
                queue: Some(4485),
                ..Default::default()
            },
        ] {
            assert!(
                !is_phantom("querying", &ev, Some(0), late),
                "已落库的证据 {ev:?} 必须与新鲜回执等效"
            );
        }
    }

    /// 计费是决定性信号，队列位次不是：有计费就连**嫌疑**都不成立，
    /// 而只有队列位次时仍是嫌疑（扫描路径要为它单查一次拿准），只是最终判不死。
    #[test]
    fn billing_evidence_clears_the_suspicion_queue_evidence_only_clears_the_verdict() {
        let late = PHANTOM_GRACE_SECS + 1;
        let billed = Evidence {
            submit_credit: Some(8),
            ..Default::default()
        };
        assert!(!phantom_suspect("querying", &billed, Some(0), late));
        let queued = Evidence {
            queue: Some(4485),
            ..Default::default()
        };
        assert!(
            phantom_suspect("querying", &queued, Some(0), late),
            "没有计费回执 → 仍是嫌疑"
        );
        assert!(
            !is_phantom("querying", &queued, Some(0), late),
            "但进过队列就不得判死"
        );
    }

    /// 界面读的判据必须与真正会去判死的那条同源 —— 这是把 `phantomSuspect` 从前端
    /// 挪到 Rust 的全部理由。取样覆盖三处已落库证据，任一非空即不再显示为幽灵。
    #[test]
    fn the_view_predicate_agrees_with_the_verdict_predicate() {
        let late = PHANTOM_GRACE_SECS + 1;
        let mut c = clip(None, None, None);
        c.stage = "run".into();
        c.submitted_at = Some(0);
        assert!(clip_looks_phantom(&c, late));
        assert!(
            !clip_looks_phantom(&c, PHANTOM_GRACE_SECS),
            "宽限期内不显示为幽灵"
        );

        for patch in [
            |c: &mut repo::ClipRow| c.credit_count = Some(8),
            |c: &mut repo::ClipRow| c.submit_credit = Some(8),
            |c: &mut repo::ClipRow| c.queue_idx = Some(4485),
        ] {
            let mut with_evidence = c.clone();
            patch(&mut with_evidence);
            assert!(!clip_looks_phantom(&with_evidence, late));
            assert!(!is_phantom(
                "querying",
                &Evidence::from_clip(&with_evidence),
                with_evidence.submitted_at,
                late
            ));
        }

        // 只对在跑的条目成立：已经判死的 fail 行由 error_type 说话，不再重判。
        let mut done = c.clone();
        done.stage = "fail".into();
        assert!(!clip_looks_phantom(&done, late));
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
        assert_eq!(
            poll_interval_for(0),
            SWEEP_VIP_SECS,
            "刚提交时跟最密的扫描档同步，不比它更密（回落路径是逐条起进程的）"
        );
        assert_eq!(poll_interval_for(86_400), 1800, "过夜半小时一次足够");
    }

    // 退避带来的实际节省：19 条跑 8 小时，从九万次降到几百次量级。
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

    /// 整表扫描的进程数与在跑条数**脱钩** —— 这才是它取代逐条轮询的理由。
    ///
    /// 逐条靠「问得少」省钱，代价是出片延迟；扫描靠「一次问全部」省钱，于是频率
    /// 是纯粹的成本旋钮。条数越多，差距越大。
    #[test]
    fn sweeping_decouples_process_count_from_clip_count() {
        let hours = 8i64;
        let per_clip_calls = |clips: i64| {
            let mut n = 0i64;
            let mut t = 0i64;
            while t < hours * 3600 {
                t += poll_interval_for(t);
                n += clips;
            }
            n
        };
        for every in [SWEEP_VIP_SECS, SWEEP_PLAIN_SECS] {
            let sweeps = hours * 3600 / every;
            assert!(
                sweeps < per_clip_calls(19),
                "19 条在跑时扫描就该更省：{sweeps} vs {}",
                per_clip_calls(19)
            );
            // 100 条：逐条随条数线性涨，扫描一动不动 —— 这是「脱钩」的全部意思。
            assert_eq!(sweeps, hours * 3600 / every);
            assert!(sweeps * 5 < per_clip_calls(100));
        }
    }

    /// 分档：有 VIP 走快档，全非 VIP 走慢档。
    ///
    /// 依据是实测的两种等待时长：VIP 直接 Generating、1–3 分钟出片；非 VIP 排在
    /// 第 4485 位、要等几小时。「含 VIP 就走快档」而不是按多数派 —— 慢档会让那几条
    /// 快单白等，而快档对慢单的额外代价只是每 8 小时多几十个进程，两边不对等。
    #[test]
    fn sweep_tier_follows_the_channel() {
        assert_eq!(sweep_interval(true), SWEEP_VIP_SECS);
        assert_eq!(sweep_interval(false), SWEEP_PLAIN_SECS);
        assert!(
            sweep_interval(true) < sweep_interval(false),
            "VIP 档必须更密"
        );

        // 降幅要对得上写在文档里的那张表（8 小时过夜）。
        let per_night = |every: i64| 8 * 3600 / every;
        assert_eq!(per_night(SWEEP_VIP_SECS), 96);
        assert_eq!(per_night(SWEEP_PLAIN_SECS), 48);
    }

    /// 回落路径（逐条 `query_result`）绝不能比整表扫描问得还勤。
    ///
    /// 它是 O(n) 的：扫描认不出的那几条若每 10 秒各起一个进程，反而会成为进程数的
    /// 大头 —— 那正是把主路径换成扫描之后最容易留下的一处旧参数。
    #[test]
    fn per_clip_fallback_is_never_denser_than_the_sweep() {
        for age in [0, 60, 599, 600, 3600, 6 * 3600, 24 * 3600] {
            assert!(
                poll_interval_for(age) >= SWEEP_VIP_SECS,
                "age={age} 的回落间隔比最密的扫描档还密"
            );
        }
    }

    /// 提交后的补扫：把「上次扫描时刻」往前挪，使下一轮在 60 秒后到点。
    ///
    /// 它存在的理由是分档的那处代价：VIP 1–3 分钟出片，而快档 5 分钟一问，
    /// 于是刚提交完盯着屏幕的那几分钟恰恰是最可能什么都看不到的。
    #[test]
    fn submit_requests_a_catch_up_sweep() {
        SWEEP_EVERY.store(SWEEP_VIP_SECS, std::sync::atomic::Ordering::Relaxed);
        request_sweep_soon(10_000);
        assert_eq!(
            next_sweep_in(10_000),
            Some(SWEEP_AFTER_SUBMIT_SECS),
            "补扫应在 60 秒后到点，而不是立刻 —— 提交那一刻即梦还没给位次"
        );
        // 慢档下同样是 60 秒后，不是「慢档的十分之一」这种随档位漂移的值。
        SWEEP_EVERY.store(SWEEP_PLAIN_SECS, std::sync::atomic::Ordering::Relaxed);
        request_sweep_soon(20_000);
        assert_eq!(next_sweep_in(20_000), Some(SWEEP_AFTER_SUBMIT_SECS));
    }

    // 扫描路径拿不到队列位次，故只凭「没有计费回执」不得判死 ——
    // 那会把「两个信号同时缺席」这条规则悄悄降成一个信号。
    #[test]
    fn a_suspect_is_not_yet_a_verdict() {
        let late = PHANTOM_GRACE_SECS + 1;
        // 嫌疑成立：过了宽限期还没有计费回执。
        assert!(phantom_suspect(
            "querying",
            &fresh(None, None),
            Some(0),
            late
        ));
        // 但只有同时拿到「队列位次也缺席」这一条，才是判决。
        assert!(is_phantom("querying", &fresh(None, None), Some(0), late));
        assert!(
            !is_phantom("querying", &fresh(None, Some(4485)), Some(0), late),
            "确认查询问到了位次 → 它在排队，只是即梦这一路没回传计费"
        );
        // 有计费就连嫌疑都不成立 —— 这条正是扫描能把单条查询降到几乎为零的原因：
        // 一旦看到计费，这一条永远不再需要为「它是不是幽灵」单独问一次。
        assert!(!phantom_suspect(
            "querying",
            &fresh(Some(8), None),
            Some(0),
            late
        ));
    }

    /// 空的在跑集合**照样算跑过一轮**。
    ///
    /// 反过来的话 `LAST_SWEEP` 永远停在 0，循环里那句「到点了吗」恒为真：每 6 秒
    /// 进一次扫描分支，顺带把跟在它后面的自动补单也拉成 6 秒一轮 —— 而队列空恰恰
    /// 正是补单器最活跃的时候。队列面板那句「下次查询还有 N 秒」也会永远没有答案。
    #[tokio::test]
    async fn an_empty_queue_still_counts_as_one_sweep() {
        let (pool, _d) = crate::db::test_support::test_pool().await;
        let dirs = DataDirs::new("/tmp/gd-sweep-test-never-written");
        LAST_SWEEP.store(0, std::sync::atomic::Ordering::Relaxed);
        SWEEP_EVERY.store(SWEEP_PLAIN_SECS, std::sync::atomic::Ordering::Relaxed);

        let sum = sweep_once(
            &pool,
            &dirs,
            "dreamina",
            "seedance2.0fast",
            None,
            &Activity::silent(),
        )
        .await
        .unwrap();
        assert_eq!(sum.polled, 0, "没有在跑条目 → 一个进程都不该起");
        let after = LAST_SWEEP.load(std::sync::atomic::Ordering::Relaxed);
        assert!(after > 0, "空表也要记下「这一轮跑过了」");
        assert!(
            next_sweep_in(after).is_some_and(|s| s > 0),
            "下一轮必须等满一个间隔，而不是下个 tick 立刻再来"
        );
    }

    /// 手动刷新在空表上也要**把倒计时重置掉**，并且一个进程都不起。
    ///
    /// 同 `sweep_once` 的理由：`LAST_SWEEP` 留在 0 会让循环里那句「到点了吗」恒为真。
    /// 另外 `on_step` 必须**至少回调一次**（哪怕 total=0）—— 前端那个按钮靠它从
    /// 「正在刷新」回到空闲态，一次都不回调的话按钮会一直转下去。
    #[tokio::test]
    async fn a_manual_refresh_on_an_empty_table_still_resets_the_countdown() {
        let (pool, _d) = crate::db::test_support::test_pool().await;
        let dirs = DataDirs::new("/tmp/gd-refresh-test-never-written");
        LAST_SWEEP.store(0, std::sync::atomic::Ordering::Relaxed);
        SWEEP_EVERY.store(SWEEP_PLAIN_SECS, std::sync::atomic::Ordering::Relaxed);

        let mut steps: Vec<(i64, i64)> = Vec::new();
        let sum = refresh_now(
            &pool,
            &dirs,
            "dreamina",
            "seedance2.0fast",
            None,
            &Activity::silent(),
            |done, total, _| steps.push((done, total)),
        )
        .await
        .unwrap();

        assert_eq!(sum.polled, 0, "没有在跑条目 → 一个 query_result 都不该起");
        assert_eq!(steps, vec![(0, 0)], "空表也要回一次进度，否则按钮一直转");
        let after = LAST_SWEEP.load(std::sync::atomic::Ordering::Relaxed);
        assert!(after > 0, "手动问过一轮之后，「下次查询」理应重新计时");
        assert!(last_sweep_at().is_some_and(|t| t == after));
    }

    /// 「上次查询」是**真实查询时刻**，不是心跳时刻。
    ///
    /// 顶栏那颗按钮读的就是它。此前那颗胶囊读的是心跳（6 秒一次），于是它显示的
    /// 「3 秒前」与「数据有多新」完全无关 —— 真实查询可能已经是十分钟前的事。
    #[test]
    fn last_sweep_at_is_absent_until_something_actually_asked() {
        LAST_SWEEP.store(0, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(last_sweep_at(), None, "没问过就说没问过，不拿 0 当时刻");
        LAST_SWEEP.store(1_700_000_000, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(last_sweep_at(), Some(1_700_000_000));
    }

    // 队列面板那句「下次查询还有 N 秒」必须与真正跑的循环同源。
    #[test]
    fn next_sweep_countdown_tracks_the_loop() {
        // 还没跑过第一轮 → 不编一个数字出来。
        LAST_SWEEP.store(0, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(next_sweep_in(1000), None);
        SWEEP_EVERY.store(SWEEP_PLAIN_SECS, std::sync::atomic::Ordering::Relaxed);
        LAST_SWEEP.store(1000, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(next_sweep_in(1000), Some(SWEEP_PLAIN_SECS));
        assert_eq!(next_sweep_in(1000 + SWEEP_PLAIN_SECS), Some(0));
        assert_eq!(next_sweep_in(9999), Some(0), "过点了就是 0，不是负数");
    }

    // 从没查过的立刻查（刚提交那一刻就想知道进没进队列）；查过的按退避等到点。
    #[test]
    fn due_check_polls_new_clips_immediately_then_waits() {
        assert!(is_due(Some(1000), None, 1000), "没查过的必须立刻查");
        // 刚提交 30 秒，上次查是 5 秒前 → 300 秒间隔还没到。
        assert!(!is_due(Some(1000), Some(1025), 1030));
        assert!(is_due(Some(1000), Some(1030 - SWEEP_VIP_SECS), 1030));
        // 等了两小时的，间隔是 600 秒：60 秒前查过 → 不到点。
        let two_h = 1000 + 7200;
        assert!(!is_due(Some(1000), Some(two_h - 60), two_h));
        assert!(is_due(
            Some(1000),
            Some(two_h - SWEEP_PLAIN_SECS - 1),
            two_h
        ));
    }

    /// 落库失败绝不能把 submit_id 吞掉：钱已经扣了，那串 id 是唯一的凭证。
    ///
    /// 用「表没了」来制造一次必然失败的写入 —— 重点不是错误长什么样，而是失败之后
    /// 日志里**还找不找得到那个 submit_id**，以及它有没有把整批一起带走
    /// （返回 `Err` 而不是 `?` 冒泡，调用方据此记账并继续下一条）。
    #[tokio::test]
    async fn a_failed_receipt_write_still_shouts_the_submit_id() {
        let (pool, _d) = crate::db::test_support::test_pool().await;
        sqlx::query("DROP TABLE v2v_clips")
            .execute(&pool)
            .await
            .unwrap();
        let log = Activity::silent();
        let receipt = dreamina::SubmitReceipt::healthy("sub-evidence-only", 8);
        let err = persist_submit(&pool, 1, &receipt, &log, Some((1, "GG-0001")))
            .await
            .unwrap_err();
        assert!(!err.is_empty());
        let shouted: Vec<_> = log
            .snapshot()
            .into_iter()
            .filter(|e| e.level == "error" && e.message.contains("sub-evidence-only"))
            .collect();
        assert!(
            !shouted.is_empty(),
            "落库失败后 submit_id 必须留在 error 级日志里 —— 那是最后的凭证"
        );
        assert_eq!(shouted.len(), 3, "退避重试三次，每次都留一条凭证");
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
