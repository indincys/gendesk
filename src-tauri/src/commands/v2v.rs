//! 视频流水线域命令（图生视频）。
//!
//! 边界：**GenDesk 全程持有流水线状态**，Claude Code / Codex 侧的 skill 只做一件事——
//! 把生图提示词改写成图生视频提示词。提交/轮询/下载/重试/验收都在这里，因为它们不是
//! 智能任务，而 GenDesk 已经有状态机、崩溃恢复和 UI。

use serde::{Deserialize, Serialize};
use specta::Type;
use sqlx::SqlitePool;
use tauri::{AppHandle, State};
use tauri_specta::Event;

use crate::db::now_unix;
use crate::db::repo::{settings as settings_repo, trash as trash_repo, v2v as repo};
use crate::error::{AppError, AppResult};
use crate::files;
use crate::state::AppState;
use crate::v2v::activity::ActivityEntry;
use crate::v2v::autofill::AutofillCfg;
use crate::v2v::dreamina::{self, CreditInfo, GenOpts, ModelInfo, SessionInfo};
use crate::v2v::events::{StageCounts, V2vChanged};
use crate::v2v::handoff::{self, IngestSummary, MaterializeSummary};
use crate::v2v::runner::{self, SubmitSummary};

const KEY: &str = "v2v";

/// 图生视频设置（`settings` 表 key='v2v' 单行 JSON）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct V2vSettings {
    /// 交接根目录。skill 侧把它写死才能做到「什么都不用输入」，故默认值必须可预测。
    #[serde(default = "d_root")]
    pub handoff_root: String,
    /// 即梦 CLI 可执行（默认走 PATH 的 `dreamina`；可填绝对路径）。
    #[serde(default = "d_bin")]
    pub bin: String,
    /// 默认模型。空 = 不发高级控制，走 CLI 自己的默认路径。
    ///
    /// **默认值是 `seedance2.0fast`，不是空**。「跟随 CLI 默认」看着最稳，实际上是把
    /// 「这批片子按什么价钱生成」交给了一个我们不控制、会随版本变的选择：实测同为
    /// 4s/720p，`seedance2.0fast` 走 `dreamina_fusion_video40` 收 8 额度，而
    /// `seedance2.0fast_vip` 走 `..._vision` 收 44 —— 5.5 倍差价，画幅时长完全一样。
    /// 花钱的选择必须是显式的。
    #[serde(default = "d_model")]
    pub model_version: String,
    #[serde(default)]
    pub duration: Option<i64>,
    #[serde(default)]
    pub video_resolution: String,
    /// 即梦会话 id（`--session`）。
    #[serde(default)]
    pub session: Option<i64>,
    /// 后台轮询开关。关掉后已提交的条目不再自动取回（排查问题时用）。
    #[serde(default = "d_true")]
    pub poll_enabled: bool,
    /// 判超时的小时数。`None` = **不限**（默认），永远等下去。
    ///
    /// 之所以默认不限：判死一条还在跑的任务代价是实打实的钱（额度已扣、即梦那边照跑），
    /// 而多等的代价只是看板上多几条「已提交」。实测原来那个 45 分钟硬编码把 19 条
    /// 还在 `querying` 的任务全判死了。退避轮询让「永远等」的开销低到可以接受
    /// （等满一小时后每 10 分钟才问一次）。
    #[serde(default)]
    pub timeout_hours: Option<i64>,
    /// 常驻的非 VIP 队列（自动补单）。默认关 —— 见 `v2v::autofill` 的四道闸。
    #[serde(default)]
    pub autofill: AutofillCfg,
    /// 成片交付目录。空 = 默认 `{app_data}/outputs/视频`。
    ///
    /// 成片是 B-roll 素材，下游是剪辑而不是发布链，故它必须落在用户自己的工作目录里
    /// （素材库、剪辑工程旁边），而不是应用数据目录深处 —— 那个位置人在 Finder 里
    /// 根本找不到，等于交付了个寂寞。
    ///
    /// 形制照 `handoff_root`（空串回落到默认）而**不是** `Settings::output_dir`：
    /// 后者有字段、有选择器、有设置页 UI，却没有任何一个消费者读它 ——
    /// 那是「选了目录却不生效」，正是这一版要根治的那类失信。
    #[serde(default)]
    pub clips_output_dir: String,
}

fn d_root() -> String {
    handoff::default_root().to_string_lossy().to_string()
}
fn d_bin() -> String {
    dreamina::DEFAULT_BIN.to_string()
}
/// 默认模型 = 最便宜的够用档（8 额度 · 4s · 720p）。见 `V2vSettings::model_version`。
fn d_model() -> String {
    dreamina::DEFAULT_MODEL.to_string()
}
fn d_true() -> bool {
    true
}

impl Default for V2vSettings {
    fn default() -> Self {
        Self {
            handoff_root: d_root(),
            bin: d_bin(),
            model_version: d_model(),
            duration: None,
            video_resolution: String::new(),
            session: None,
            poll_enabled: true,
            timeout_hours: runner::DEFAULT_TIMEOUT_HOURS,
            autofill: AutofillCfg::default(),
            clips_output_dir: String::new(),
        }
    }
}

impl V2vSettings {
    /// 设置里的默认生成参数。空串一律折成 None —— 前端的空输入框不该变成
    /// `--model_version=` 这种必被拒的空 flag。
    pub fn defaults(&self) -> GenOpts {
        let blank = |s: &String| (!s.trim().is_empty()).then(|| s.trim().to_string());
        GenOpts {
            model_version: blank(&self.model_version),
            duration: self.duration,
            video_resolution: blank(&self.video_resolution),
            session: self.session,
        }
    }
    /// 超时上限（秒）。`None` = 不限。0 或负数也当作不限 —— 前端把输入框清空时
    /// 传 0 比传 null 常见，两种都该是「不限」而不是「立刻超时」。
    pub fn timeout_secs(&self) -> Option<i64> {
        self.timeout_hours.filter(|h| *h > 0).map(|h| h * 3600)
    }
    pub fn root(&self) -> std::path::PathBuf {
        if self.handoff_root.trim().is_empty() {
            handoff::default_root()
        } else {
            std::path::PathBuf::from(self.handoff_root.trim())
        }
    }
    /// 成片交付目录。空/仅空白 → 默认 `{app_data}/outputs/视频`。
    pub fn clips_dir(&self, dirs: &crate::files::DataDirs) -> std::path::PathBuf {
        if self.clips_output_dir.trim().is_empty() {
            dirs.outputs().join(DEFAULT_CLIPS_SUBDIR)
        } else {
            std::path::PathBuf::from(self.clips_output_dir.trim())
        }
    }
}

/// 默认交付目录在 `outputs/` 下的子目录名。与图片输出同级，一眼看得出是视频。
pub const DEFAULT_CLIPS_SUBDIR: &str = "视频";

/// 读设置（缺失/损坏都回默认值，绝不让整页打不开）。
pub async fn load_settings(pool: &SqlitePool) -> AppResult<V2vSettings> {
    let raw = settings_repo::get_by_key(pool, KEY).await?;
    Ok(raw
        .and_then(|j| serde_json::from_str::<V2vSettings>(&j).ok())
        .unwrap_or_default())
}

#[tauri::command]
#[specta::specta]
pub async fn get_v2v_settings(state: State<'_, AppState>) -> AppResult<V2vSettings> {
    load_settings(&state.db).await
}

#[tauri::command]
#[specta::specta]
pub async fn update_v2v_settings(
    state: State<'_, AppState>,
    settings: V2vSettings,
) -> AppResult<V2vSettings> {
    // 校验默认参数组合：设置页存下一个非法组合，会让之后每一次提交都在花钱之后才报错。
    dreamina::normalize_opts(&settings.defaults())?;
    // 常驻队列同理，且更要紧：它自动花钱，配一个 VIP 模型等于每晚烧 5.5 倍。
    settings.autofill.validate()?;
    let json = serde_json::to_string(&settings)
        .map_err(|e| AppError::Internal(format!("设置序列化失败：{e}")))?;
    settings_repo::set_by_key(&state.db, KEY, &json).await?;
    // 交接根可能被改到别处 → 立刻在新位置重建工单，否则 skill 会对着旧目录干活。
    let _ = handoff::materialize(&state.db, &settings.root()).await;
    load_settings(&state.db).await
}

#[tauri::command]
#[specta::specta]
pub async fn pick_handoff_root(app: AppHandle) -> AppResult<Option<String>> {
    use tauri_plugin_dialog::DialogExt;
    Ok(app
        .dialog()
        .file()
        .blocking_pick_folder()
        .and_then(|p| p.into_path().ok())
        .map(|p| p.to_string_lossy().to_string()))
}

/// 选成片交付目录。
#[tauri::command]
#[specta::specta]
pub async fn pick_clips_output_dir(app: AppHandle) -> AppResult<Option<String>> {
    use tauri_plugin_dialog::DialogExt;
    Ok(app
        .dialog()
        .file()
        .blocking_pick_folder()
        .and_then(|p| p.into_path().ok())
        .map(|p| p.to_string_lossy().to_string()))
}

/// 当前生效的成片交付目录（绝对路径）。设置里留空时回落到默认，界面直接摆出来 ——
/// 「片子交付到哪儿了」不该靠猜。
#[tauri::command]
#[specta::specta]
pub async fn v2v_clips_dir(state: State<'_, AppState>) -> AppResult<String> {
    let settings = load_settings(&state.db).await?;
    Ok(settings
        .clips_dir(&state.dirs)
        .to_string_lossy()
        .to_string())
}

/// 选即梦 CLI 可执行文件。
///
/// 「怎么知道路径填什么」不该由用户回答：给个文件选择器，再不济也有 [`resolve_v2v_bin`]
/// 的自动探测兜着。
#[tauri::command]
#[specta::specta]
pub async fn pick_dreamina_bin(app: AppHandle) -> AppResult<Option<String>> {
    use tauri_plugin_dialog::DialogExt;
    Ok(app
        .dialog()
        .file()
        .blocking_pick_file()
        .and_then(|p| p.into_path().ok())
        .map(|p| p.to_string_lossy().to_string()))
}

/// 当前设置能解析到的 CLI 绝对路径（设置页直接显示「实际会执行哪个文件」）。
///
/// 回 `None` 而不是报错：设置页每次打开都调它，探测失败是常态（还没装），
/// 不该每次进设置都弹一个红条。
#[tauri::command]
#[specta::specta]
pub async fn resolve_v2v_bin(state: State<'_, AppState>) -> AppResult<Option<String>> {
    let s = load_settings(&state.db).await?;
    Ok(dreamina::detect_bin(&s.bin))
}

/// 受控模型清单（前端选择器渲染源，单点在 `v2v::dreamina`）。
#[tauri::command]
#[specta::specta]
pub async fn v2v_models() -> AppResult<Vec<ModelInfo>> {
    Ok(dreamina::models())
}

/// 查即梦余额与账号（设置页 / 参数面板显示 + 批量提交前预检）。
#[tauri::command]
#[specta::specta]
pub async fn v2v_credit(state: State<'_, AppState>) -> AppResult<CreditInfo> {
    let s = load_settings(&state.db).await?;
    let info = dreamina::user_credit(&s.bin, &state.v2v_log).await?;
    state.v2v_log.info(
        "cli",
        None,
        format!(
            "账号余额 {} 额度（等级 {}）",
            info.total_credit,
            if info.vip_level.is_empty() {
                "—"
            } else {
                &info.vip_level
            }
        ),
        None,
    );
    Ok(info)
}

/// 列出即梦会话（用户口中的「通道」）。
///
/// 原先设置里只有一个裸数字输入框 —— 那个数字对应哪条会话，在应用里根本无从得知。
#[tauri::command]
#[specta::specta]
pub async fn v2v_sessions(state: State<'_, AppState>) -> AppResult<Vec<SessionInfo>> {
    let s = load_settings(&state.db).await?;
    dreamina::sessions(&s.bin, &state.v2v_log).await
}

/// 额度台账：余额（远端）+ 已消耗（本地库）。
///
/// **两个数字来自两个地方，故不合并成一个「已用/总额」百分比**：余额是即梦那边的账户
/// 真相（别处也可能在花它），消耗是本机这条流水线出片时收到的扣费回执。
/// 编一个百分比出来会让两者的差异变得无法解释。
#[derive(Debug, Clone, Default, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CreditStats {
    /// 远端余额；查不到时为 None，原因在 `balanceError`（未登录 / 找不到 CLI）。
    pub balance: Option<i64>,
    pub balance_error: Option<String>,
    pub user_id: Option<i64>,
    pub vip_level: String,
    /// 本机流水线累计消耗（只算收到扣费回执的条目）。
    pub spent_total: i64,
    /// 近 7 天 / 近 24 小时（按出片时刻切窗）。
    pub spent_week: i64,
    pub spent_day: i64,
    /// 分账：成片 / 未通过（= 白花的）/ 待验收（还没定论）。
    pub spent_pass: i64,
    pub spent_rej: i64,
    pub spent_pending: i64,
    /// 计入统计的条数（有 credit_count 的）。
    pub counted_clips: i64,
}

#[tauri::command]
#[specta::specta]
pub async fn v2v_credit_stats(state: State<'_, AppState>) -> AppResult<CreditStats> {
    let s = load_settings(&state.db).await?;
    let mut out = CreditStats::default();
    // 余额查不到不该让整个面板打不开：没装 CLI / 没登录是常态，而本地消耗照样算得出。
    match dreamina::user_credit(&s.bin, &state.v2v_log).await {
        Ok(info) => {
            out.balance = Some(info.total_credit);
            out.user_id = info.user_id;
            out.vip_level = info.vip_level;
        }
        Err(e) => out.balance_error = Some(format!("{e}")),
    }
    for row in repo::credit_by_stage(&state.db).await? {
        out.spent_total += row.spent;
        out.counted_clips += row.clips;
        match row.stage.as_str() {
            "pass" => out.spent_pass += row.spent,
            "rej" => out.spent_rej += row.spent,
            // rev（待验收）与重跑后仍留着回执的条目都算「还没定论」。
            _ => out.spent_pending += row.spent,
        }
    }
    let now = now_unix();
    out.spent_day = repo::credit_since(&state.db, now - 24 * 3600).await?;
    out.spent_week = repo::credit_since(&state.db, now - 7 * 24 * 3600).await?;
    Ok(out)
}

/// 队列观测。
///
/// ## 为什么这里**没有**「前面还有多少人在排队」
///
/// 因为即梦不给。实测排队中的 `query_result` 只回 submit_id / prompt / logid /
/// gen_status 四个字段，`list_task` 也只有状态；`queue_info.queue_idx` 只在**已完成**
/// 的回体里出现过（值 0、Finish）。解析代码留着，它哪天开始回传就自动显示，
/// 但界面上不能凭空造一个「第 N 位」——编出来的排队位次比没有更糟。
///
/// ## 那么「第二天醒来判断还在排队还是卡住了」靠什么
///
/// 靠**我们自己就能测准**的两件事：
/// - 最久那条已经等了多久（`oldest_wait`）——绝对进度。
/// - 这批的出片速度（`since_last_finish` + 逐小时直方图）——**相对进度**，
///   也是真正的判据：「上次出片 20 分钟前」说明队列在动，「上次出片 9 小时前」说明该查了。
#[derive(Debug, Clone, Default, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct QueueStats {
    pub running: i64,
    /// 在跑条目里等得最久 / 最短的那条，已等待秒数。
    pub oldest_wait: i64,
    pub newest_wait: i64,
    /// 上次出片距今秒数；None = 这条流水线还没出过任何片。
    pub since_last_finish: Option<i64>,
    /// 最近 12 小时逐小时出片数，`[0]` 是最近一小时。趋势比总数有用。
    pub hourly: Vec<i64>,
    /// 按最近 6 小时的实测速度估算：把当前在跑的全部收完还要多久（秒）。
    /// 速度为 0 时是 None —— **不编数字**，「还需 ∞」不如老实说估不出来。
    pub eta_secs: Option<i64>,
    /// 下一条到点查询还有多少秒（让人知道界面不是死的，只是在省着问）。
    pub next_poll_in: Option<i64>,
    /// 超时上限小时数；None = 不限。
    pub timeout_hours: Option<i64>,
}

#[tauri::command]
#[specta::specta]
pub async fn v2v_queue_stats(state: State<'_, AppState>) -> AppResult<QueueStats> {
    let now = now_unix();
    let s = load_settings(&state.db).await?;
    let (running, oldest, newest) = repo::running_waits(&state.db).await?;
    let hourly = repo::finish_histogram(&state.db, now, 12).await?;
    // 速度取最近 6 小时：太短会被一条零星出片带偏，太长会把几小时前那波算进来。
    let recent: i64 = hourly.iter().take(6).sum();
    let eta_secs = (recent > 0 && running > 0).then(|| running * 6 * 3600 / recent);
    // 「下次查询还有几秒」现在由整表扫描的节拍决定，不再由每条各自的退避决定 ——
    // 循环自己记着上一次扫描的时刻，这里读它，两边不会分叉。
    let next_poll_in = runner::next_sweep_in(now);
    Ok(QueueStats {
        running,
        oldest_wait: oldest.map_or(0, |t| (now - t).max(0)),
        newest_wait: newest.map_or(0, |t| (now - t).max(0)),
        since_last_finish: repo::last_finished_at(&state.db)
            .await?
            .map(|t| (now - t).max(0)),
        hourly,
        eta_secs,
        next_poll_in,
        timeout_hours: s.timeout_hours,
    })
}

/// 常驻队列此刻的样子（看板顶部那条 pill 的数据源）。
///
/// 它要回答的问题只有一个：**这条队列现在是在跑，还是停了，停在哪一步**。
/// 「开着」不等于「在跑」——没料了、日限满了、余额不够都会让它安静地停下来，
/// 而一条安静停摆的常驻队列与一条正常运转的在界面上长得一模一样。
#[derive(Debug, Clone, Default, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AutofillStatus {
    pub enabled: bool,
    pub depth: i64,
    /// 补单器自己放出去、此刻在跑的条数。
    pub running: i64,
    /// 待提交存量（有视频提示词的）。
    pub stock: i64,
    pub low_water: i64,
    /// 今日（近 24 小时）**已提交**掉的额度与上限。0 上限 = 不限。
    pub spent_today: i64,
    pub daily_credits: i64,
    pub model_version: String,
    /// 单条预估额度；查不到单价为 None。
    pub unit_cost: Option<i64>,
    /// 停下来的原因（补满了就是 None）。
    pub blocked: Option<String>,
    /// 配置非法时的原因 —— 有值时这条队列一条都不会跑。
    pub error: Option<String>,
}

#[tauri::command]
#[specta::specta]
pub async fn v2v_autofill_status(state: State<'_, AppState>) -> AppResult<AutofillStatus> {
    let s = load_settings(&state.db).await?;
    let cfg = &s.autofill;
    let now = now_unix();
    let mut out = AutofillStatus {
        enabled: cfg.enabled,
        depth: cfg.depth,
        running: repo::count_auto_running(&state.db).await?,
        stock: repo::count_autofill_pool(&state.db).await?,
        low_water: cfg.low_water,
        spent_today: repo::credit_submitted_since(&state.db, now - 24 * 3600).await?,
        daily_credits: cfg.daily_credits,
        model_version: cfg.model_version.clone(),
        unit_cost: None,
        blocked: None,
        error: None,
    };
    match cfg.validate() {
        Ok(opts) => {
            out.unit_cost = match (
                opts.model_version.as_deref(),
                opts.video_resolution.as_deref(),
                opts.duration,
            ) {
                (Some(m), Some(r), Some(d)) => dreamina::estimate_credits(m, r, d),
                _ => None,
            };
            // 余额这里**不查**：这个命令随每次事件刷新调用，而查余额要跑一次 CLI。
            // 真正补单那一刻才查（`autofill::tick`），那是唯一必须准的时刻。
            let p = crate::v2v::autofill::plan(
                cfg,
                out.running,
                out.stock,
                out.spent_today,
                None,
                out.unit_cost,
                0,
            );
            out.blocked = p.blocked.map(|b| b.label().to_string());
        }
        Err(e) => out.error = Some(format!("{e}")),
    }
    Ok(out)
}

/// 「你离开的这段时间」发生了什么。
///
/// 视频是**过夜跑**的：睡前提交、早上回来。回来那一刻真正要知道的不是「现在有多少条」，
/// 而是「我不在的时候出了什么事」—— 出了几条片、判死了几条、花了多少钱。
/// 这两个问题的答案完全不同：待验收 46 条既可能是昨晚新出的 46 条，也可能是三天没看了。
///
/// 只在**确实离开过**（`away_secs` 够长）且**确实发生过事**时才由前端显示。
#[derive(Debug, Clone, Default, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AwayDigest {
    /// 距上次看这一页多久（秒）。首次使用（没有 last_seen）为 0。
    pub away_secs: i64,
    pub finished: i64,
    pub failed: i64,
    /// 判死里的幽灵单 —— 它们**没扣费**，文案必须与超时相反（直接重跑，不是继续等待）。
    pub phantom: i64,
    pub credits: i64,
    /// 此刻待验收总数（横幅那句「待验收涨到 N 条」）。
    pub rev_now: i64,
}

/// 「上次看过视频流水线」的时刻。放 settings 表而不是前端 localStorage：
/// 它要参与 Rust 侧的聚合查询（按 `finished_at` 切），来回传一个前端持有的时间戳
/// 只会让「摘要统计的是哪一段」这件事有两处说法。
const SEEN_KEY: &str = "v2v_seen";

#[tauri::command]
#[specta::specta]
pub async fn v2v_away_digest(state: State<'_, AppState>) -> AppResult<AwayDigest> {
    let now = now_unix();
    let seen: Option<i64> = settings_repo::get_by_key(&state.db, SEEN_KEY)
        .await?
        .and_then(|s| s.trim().parse::<i64>().ok());
    let counts = runner::counts(&state.db).await?;
    let Some(since) = seen else {
        // 头一次打开：没有「离开」这回事，不该拿全部历史冒充昨夜的战果。
        return Ok(AwayDigest {
            rev_now: counts.rev,
            ..Default::default()
        });
    };
    let row = repo::away_digest(&state.db, since).await?;
    Ok(AwayDigest {
        away_secs: (now - since).max(0),
        finished: row.finished,
        failed: row.failed,
        phantom: row.phantom,
        credits: row.credits,
        rev_now: counts.rev,
    })
}

/// 记下「看过了」。摘要横幅显示过一次就该记，否则同一份战报会在每次切页时重放。
#[tauri::command]
#[specta::specta]
pub async fn v2v_mark_seen(state: State<'_, AppState>) -> AppResult<()> {
    settings_repo::set_by_key(&state.db, SEEN_KEY, &now_unix().to_string()).await?;
    Ok(())
}

/// 交接目录的当前状态（「交接：42 条已物化 · 3 分钟前收录」）。
///
/// 待改写那一列是**唯一一段不在本机手里**的流程：工单写出去了没有、skill 写回来过没有，
/// 界面上原先一个字都没有 —— 于是「skill 到底跑没跑」只能靠去开文件夹看。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct HandoffStatus {
    /// 待改写工单根目录绝对路径。
    pub pending_dir: String,
    /// 已物化的组数 / 条数。
    pub groups: i64,
    pub items: i64,
    /// 缩略图缺失而写不进工单的条数（父图被清理过）。
    pub skipped: i64,
    /// 最近一次收录到改写结果的时刻；None = 从来没收到过。
    pub last_ingest_at: Option<i64>,
    /// 物化失败的原因（磁盘满 / 目录不可写）。有值时界面必须报出来，
    /// 否则 skill 那边看到的是一个空目录，而这边显示一切正常。
    pub error: Option<String>,
}

#[tauri::command]
#[specta::specta]
pub async fn v2v_handoff_status(state: State<'_, AppState>) -> AppResult<HandoffStatus> {
    let s = load_settings(&state.db).await?;
    let root = s.root();
    let mut out = HandoffStatus {
        pending_dir: root
            .join(handoff::V2V)
            .join(handoff::PENDING)
            .to_string_lossy()
            .to_string(),
        groups: 0,
        items: 0,
        skipped: 0,
        last_ingest_at: repo::last_rewrote_at(&state.db).await?,
        error: None,
    };
    match handoff::materialize(&state.db, &root).await {
        Ok(m) => {
            out.pending_dir = m.pending_dir;
            out.groups = m.groups;
            out.items = m.items;
            out.skipped = m.skipped;
        }
        Err(e) => out.error = Some(format!("{e}")),
    }
    Ok(out)
}

/// 执行日志快照（打开日志面板时取一次，之后靠 `v2v://activity` 事件增量追加）。
#[tauri::command]
#[specta::specta]
pub async fn v2v_activity(state: State<'_, AppState>) -> AppResult<Vec<ActivityEntry>> {
    Ok(state.v2v_log.snapshot())
}

#[tauri::command]
#[specta::specta]
pub async fn clear_v2v_activity(state: State<'_, AppState>) -> AppResult<()> {
    state.v2v_log.clear();
    Ok(())
}

/// 当前**实际生效**的生成参数（参数面板的唯一数据源）。
///
/// 「走哪个模型、什么分辨率」这个问题，设置页那几个下拉框其实回答不了：留空意味着
/// 「跟随 CLI 默认」，而那个默认值是什么、发出去的到底是哪几个 flag，界面上都看不出来。
/// 故这里直接给出**归一化之后**的三件套，外加一条示例命令行 —— 与真正 exec 的
/// `command_line` 同源，只把图片与提示词换成占位符。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct EffectiveParams {
    /// 设置里填的原文（可能是空串 = 自动探测）。
    pub bin: String,
    /// 解析出来的绝对路径；None = 没探测到。
    pub resolved_bin: Option<String>,
    /// 归一化后的三件套。None 表示「不发这个 flag，由 CLI 自己决定」。
    pub model_version: Option<String>,
    pub duration: Option<i64>,
    pub video_resolution: Option<String>,
    pub session: Option<i64>,
    /// 是否一个高级 flag 都不发（三者全空）。
    pub uses_cli_defaults: bool,
    /// 与真正 exec 同源的示例命令行。
    pub sample_command: String,
    /// 参数组合非法时的原因（设置里存了坏组合 → 每次提交都在花钱之后才报错）。
    pub error: Option<String>,
}

#[tauri::command]
#[specta::specta]
pub async fn v2v_effective_params(state: State<'_, AppState>) -> AppResult<EffectiveParams> {
    let s = load_settings(&state.db).await?;
    let resolved_bin = dreamina::detect_bin(&s.bin);
    let mut out = EffectiveParams {
        bin: s.bin.clone(),
        resolved_bin: resolved_bin.clone(),
        model_version: None,
        duration: None,
        video_resolution: None,
        session: s.session,
        uses_cli_defaults: true,
        sample_command: String::new(),
        error: None,
    };
    match dreamina::normalize_opts(&s.defaults()) {
        Ok(opts) => {
            out.uses_cli_defaults = opts.model_version.is_none();
            out.model_version = opts.model_version.clone();
            out.duration = opts.duration;
            out.video_resolution = opts.video_resolution.clone();
            let argv = dreamina::command_line(
                resolved_bin.as_deref().unwrap_or(dreamina::DEFAULT_BIN),
                "<首帧图>",
                "<改写后的视频提示词>",
                &opts,
            );
            out.sample_command = dreamina::display_command(&argv);
        }
        Err(e) => out.error = Some(format!("{e}")),
    }
    Ok(out)
}

/// 看板条目视图。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ClipView {
    pub id: i64,
    pub work_id: i64,
    pub group_id: Option<i64>,
    pub group_name: String,
    pub batch_id: Option<i64>,
    pub stage: String,
    pub prompt_code: String,
    /// 首帧图（父作品）原图与缩略图。
    pub image_path: String,
    pub thumb_path: String,
    pub source_prompt: String,
    pub variable_part: String,
    pub video_prompt: Option<String>,
    pub model_version: Option<String>,
    pub duration: Option<i64>,
    pub video_resolution: Option<String>,
    pub submit_id: Option<String>,
    pub credit_count: Option<i64>,
    pub video_path: Option<String>,
    pub poster_path: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub fps: Option<f64>,
    pub duration_sec: Option<f64>,
    pub attempt: i64,
    pub error_type: Option<String>,
    pub error_message: Option<String>,
    /// 即梦状态原文 + 队列位次 + 我们最后一次问到答案的时刻（0021）。
    /// 落库而非只走事件：切页/重启后「这条在排队还是在跑」仍要答得出。
    pub gen_status: Option<String>,
    pub queue_idx: Option<i64>,
    pub polled_at: Option<i64>,
    /// 即梦实际计费的型号（回执，非我们的输入）。
    pub benefit_type: Option<String>,
    /// 提交时刻。退避轮询与超时判定读它；「继续等待」会把它重置成当下。
    pub submitted_at: Option<i64>,
    /// **首次**提交时刻（0024）。卡片上的「已等 3 小时 12 分」要用它算 ——
    /// 用 `submitted_at` 算的话，按过一次「继续等待」的条目会把已经等掉的时间抹掉，
    /// 事故当天就是这样把等了十几小时的一批显示成「10 小时 54 分」。
    pub first_submitted_at: Option<i64>,
    /// 提交回执里的计费额度与状态（0024）。`submitCredit` 为空 = 即梦没给计费回执，
    /// 配合 `queueIdx` 为空即幽灵单；界面据此可以明说「这条没扣费，重跑不会重复扣」。
    pub submit_credit: Option<i64>,
    pub submit_status: Option<String>,
    /// 「这一条的历程」四个时刻（详情栏）。入队 → 改写写回 → 出片落盘 → 人工定态。
    /// 提交时刻用 `first_submitted_at`（见上）。
    pub created_at: i64,
    pub rewrote_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub reviewed_at: Option<i64>,
    /// 是不是常驻队列（自动补单）替人放行的（0026）。
    pub auto_submitted: bool,
    /// 历史上打进过哪个素材包（0025）。
    ///
    /// v0.22.0 起成片**不再入资产库**（它们是 B-roll 素材，不适合直接发布），
    /// 这条路径已整个拆掉。列保留是因为迁移 forward-only，且老数据里的值仍是事实；
    /// 但没有任何逻辑再读它，界面上也不再出现。
    pub asset_pack_id: Option<i64>,
    /// 验收通过后交付到 `{交付目录}/{组}/` 的那份拷贝（0027）。
    ///
    /// 成片页据此回答「这条片子在哪」——`clips/clip{id}.mp4` 那个名字人在 Finder 里
    /// 认不出谁是谁。为空 = 交付失败（验收时的拷贝错误不回滚验收），可「重新交付」。
    pub export_path: Option<String>,
    /// 这一条现在看着像不像幽灵单（`runner::clip_looks_phantom`）。
    ///
    /// **由 Rust 下发而不是前端自己算**。前端原来抄了一份判据（三个字段 + 一个手抄的
    /// 15 分钟常量），而它按 `firstSubmittedAt` 算等待时长、Rust 按 `submittedAt` 算
    /// —— 「继续等待」按过一次之后，两边就会对同一条给出不同结论。而这两个结论指向
    /// 相反的动作：幽灵单重跑不花钱，正在排队的重跑要再花一份。
    pub phantom_suspect: bool,
    pub accepted_at: i64,
    pub updated_at: i64,
}

impl From<repo::ClipRow> for ClipView {
    fn from(r: repo::ClipRow) -> Self {
        // 视图是一份快照，判定要一个「现在」。取当前时刻而不是让调用方传：
        // 每个列表命令各传一次，就等于给这条规则开了 N 个改错的机会。
        let phantom_suspect = runner::clip_looks_phantom(&r, crate::db::now_unix());
        Self {
            phantom_suspect,
            id: r.id,
            work_id: r.work_id,
            group_id: r.group_id,
            group_name: r.group_name,
            batch_id: r.batch_id,
            stage: r.stage,
            prompt_code: r.prompt_code,
            image_path: r.image_path,
            thumb_path: r.thumb_path,
            source_prompt: r.source_prompt,
            variable_part: r.variable_part,
            video_prompt: r.video_prompt,
            model_version: r.model_version,
            duration: r.duration,
            video_resolution: r.video_resolution,
            submit_id: r.submit_id,
            credit_count: r.credit_count,
            video_path: r.video_path,
            poster_path: r.poster_path,
            width: r.width,
            height: r.height,
            fps: r.fps,
            duration_sec: r.duration_sec,
            attempt: r.attempt,
            error_type: r.error_type,
            error_message: r.error_message,
            gen_status: r.gen_status,
            queue_idx: r.queue_idx,
            polled_at: r.polled_at,
            benefit_type: r.benefit_type,
            submitted_at: r.submitted_at,
            // 存量行（0024 之前提交的）没有这一列，回落到 submitted_at：
            // 它是个已知偏小的下界，好过让卡片上的等待时长整个消失。
            first_submitted_at: r.first_submitted_at.or(r.submitted_at),
            submit_credit: r.submit_credit,
            submit_status: r.submit_status,
            created_at: r.created_at,
            rewrote_at: r.rewrote_at,
            finished_at: r.finished_at,
            reviewed_at: r.reviewed_at,
            auto_submitted: r.auto_submitted != 0,
            asset_pack_id: r.asset_pack_id,
            export_path: r.export_path,
            accepted_at: r.accepted_at,
            updated_at: r.updated_at,
        }
    }
}

/// 全部在流水线内的条目（看板一次取完；量级是几十到几百，不必分页）。
#[tauri::command]
#[specta::specta]
pub async fn list_v2v_clips(
    state: State<'_, AppState>,
    stages: Vec<String>,
) -> AppResult<Vec<ClipView>> {
    let all = ["rewrite", "ready", "run", "rev", "pass", "rej", "fail"];
    let want: Vec<&str> = if stages.is_empty() {
        all.to_vec()
    } else {
        all.into_iter()
            .filter(|s| stages.iter().any(|x| x == s))
            .collect()
    };
    Ok(repo::list_by_stages(&state.db, &want)
        .await?
        .into_iter()
        .map(ClipView::from)
        .collect())
}

#[tauri::command]
#[specta::specta]
pub async fn v2v_counts(state: State<'_, AppState>) -> AppResult<StageCounts> {
    runner::counts(&state.db).await
}

/// 手动把作品加入流水线（用途只是筛选默认值，不是门禁 —— 堵死了就得改代码）。
#[tauri::command]
#[specta::specta]
pub async fn enqueue_works_v2v(
    state: State<'_, AppState>,
    app: AppHandle,
    work_ids: Vec<i64>,
) -> AppResult<i64> {
    let n = enqueue_works(&state.db, &work_ids).await?;
    refresh_handoff(&state.db, &app).await;
    Ok(n)
}

/// 入队一条所需的作品侧信息（组名冗余进 clip，作品删了看板仍能归组显示）。
#[derive(sqlx::FromRow)]
struct QueueSeed {
    id: i64,
    group_id: Option<i64>,
    group_name: Option<String>,
    batch_id: Option<i64>,
    prompt_text: String,
}

/// 入队若干作品，返回真正新增的条数。验收命令与手动入队共用。
///
/// **一次 IN 查询 + 一个事务**：验收页一次性通过一整页是常态（几十上百张），
/// 而原来是每张一次 SELECT 加一次 BEGIN/COMMIT —— 上百次事务提交，
/// 每次都是一轮 fsync，而它们本来就属于同一个动作。
pub async fn enqueue_works(pool: &SqlitePool, work_ids: &[i64]) -> AppResult<i64> {
    if work_ids.is_empty() {
        return Ok(0);
    }
    let holes = vec!["?"; work_ids.len()].join(",");
    let sql = format!(
        "SELECT w.id, w.group_id, g.name AS group_name, w.batch_id, w.prompt_text
           FROM accepted_works w LEFT JOIN prompt_groups g ON g.id = w.group_id
          WHERE w.id IN ({holes})"
    );
    let mut q = sqlx::query_as::<_, QueueSeed>(&sql);
    for wid in work_ids {
        q = q.bind(*wid);
    }
    let seeds = q.fetch_all(pool).await?;

    let now = now_unix();
    let mut added = 0i64;
    let mut tx = pool.begin().await?;
    for seed in seeds {
        if repo::enqueue(
            &mut tx,
            seed.id,
            seed.group_id,
            seed.group_name.as_deref().unwrap_or(""),
            seed.batch_id,
            &seed.prompt_text,
            now,
        )
        .await?
        {
            added += 1;
        }
    }
    tx.commit().await?;
    Ok(added)
}

/// 队列变化后重写磁盘工单并推事件。
///
/// 这是「验收通过后不需要点导出」成立的关键：物化由状态变化触发，而不是由按钮触发。
/// 失败只记日志不打断调用方——验收本身已经成功了，工单没写出去下一次还会重写。
pub async fn refresh_handoff(pool: &SqlitePool, app: &AppHandle) {
    match load_settings(pool).await {
        Ok(s) => {
            if let Err(e) = handoff::materialize(pool, &s.root()).await {
                tracing::warn!(error = %e, "物化改写工单失败");
            }
        }
        Err(e) => tracing::warn!(error = %e, "读图生视频设置失败"),
    }
    emit_changed(pool, app, None).await;
}

/// 推 `v2v://changed`。
pub async fn emit_changed(pool: &SqlitePool, app: &AppHandle, clip_id: Option<i64>) {
    match runner::counts(pool).await {
        Ok(counts) => {
            let _ = V2vChanged { counts, clip_id }.emit(app);
        }
        Err(e) => tracing::warn!(error = %e, "计算视频流水线计数失败"),
    }
}

/// 立刻重写工单（用户点「重新物化」，或想确认路径时）。
#[tauri::command]
#[specta::specta]
pub async fn materialize_v2v_handoff(
    state: State<'_, AppState>,
    app: AppHandle,
) -> AppResult<MaterializeSummary> {
    let s = load_settings(&state.db).await?;
    let sum = handoff::materialize(&state.db, &s.root()).await?;
    emit_changed(&state.db, &app, None).await;
    Ok(sum)
}

/// 手动收录改写结果（watcher 之外的兜底：用户等不及 2 秒防抖，或 watcher 没起来）。
#[tauri::command]
#[specta::specta]
pub async fn ingest_v2v_rewrites(
    state: State<'_, AppState>,
    app: AppHandle,
) -> AppResult<IngestSummary> {
    let s = load_settings(&state.db).await?;
    let sum = handoff::ingest(&state.db, &s.root()).await?;
    state.v2v_log.info(
        "handoff",
        None,
        format!(
            "手动收录：应用 {} · 认不出 {} · 已越过待提交 {}",
            sum.applied, sum.unmatched, sum.stale
        ),
        Some(s.root().to_string_lossy().to_string()),
    );
    refresh_handoff(&state.db, &app).await;
    Ok(sum)
}

#[tauri::command]
#[specta::specta]
pub async fn open_handoff_dir(state: State<'_, AppState>, app: AppHandle) -> AppResult<()> {
    use tauri_plugin_opener::OpenerExt;
    let s = load_settings(&state.db).await?;
    let dir = s.root().join(handoff::V2V).join(handoff::PENDING);
    std::fs::create_dir_all(&dir)?;
    app.opener()
        .open_path(dir.to_string_lossy().to_string(), None::<&str>)
        .map_err(|e| AppError::Io(format!("打开交接目录失败：{e}")))
}

/// 人工编辑待提交条目的视频提示词与参数。
#[tauri::command]
#[specta::specta]
pub async fn update_v2v_clip(
    state: State<'_, AppState>,
    app: AppHandle,
    id: i64,
    video_prompt: String,
    model_version: Option<String>,
    duration: Option<i64>,
    video_resolution: Option<String>,
) -> AppResult<bool> {
    let trimmed = video_prompt.trim();
    if trimmed.is_empty() {
        return Err(AppError::InvalidInput("视频提示词不能为空".into()));
    }
    // 就地校验参数组合：让人在编辑框里当场知道错，而不是提交时才报。
    dreamina::normalize_opts(&GenOpts {
        model_version: model_version.clone(),
        duration,
        video_resolution: video_resolution.clone(),
        session: None,
    })?;
    let ok = repo::update_ready(
        &state.db,
        id,
        trimmed,
        model_version.as_deref(),
        duration,
        video_resolution.as_deref(),
        now_unix(),
    )
    .await?;
    // 手写完提示词即离开待改写队列 → 必须重写工单，否则下一次物化又把它写进去，
    // skill 会把人写的那份覆盖掉。
    refresh_handoff(&state.db, &app).await;
    Ok(ok)
}

/// 批量覆盖选中条目的生成参数（不动提示词、不动阶段）。
///
/// 「有效编辑参数」在原来的界面里只能一条一条开详情弹窗改 —— 19 条就是 19 次。
/// 而参数恰恰是最常整批改的东西（「这一组都换成 vip 1080p」）。
///
/// 三项一起传：`None` 表示清掉该项（回落到设置里的默认值），不是「保持不变」。
/// 半套组合在这里就拦住，理由同 `normalize_opts` —— 报错必须发生在花钱之前。
#[tauri::command]
#[specta::specta]
pub async fn set_v2v_clip_params(
    state: State<'_, AppState>,
    app: AppHandle,
    ids: Vec<i64>,
    model_version: Option<String>,
    duration: Option<i64>,
    video_resolution: Option<String>,
) -> AppResult<i64> {
    let opts = GenOpts {
        model_version: model_version.clone(),
        duration,
        video_resolution: video_resolution.clone(),
        session: None,
    };
    // 归一化后再写：只给了模型时把时长/分辨率补成该模型的合法值，
    // 免得库里躺着一份「看起来只设了模型」而提交时才补出来的参数。
    let norm = dreamina::normalize_opts(&opts)?;
    let n = repo::set_params(
        &state.db,
        &ids,
        norm.model_version.as_deref(),
        norm.duration,
        norm.video_resolution.as_deref(),
        now_unix(),
    )
    .await?;
    state.v2v_log.info(
        "submit",
        None,
        format!(
            "批量设置参数：{n} 条 → 模型 {} · 时长 {} · 分辨率 {}",
            norm.model_version.as_deref().unwrap_or("CLI 默认"),
            norm.duration.map_or("CLI 默认".into(), |d| format!("{d}s")),
            norm.video_resolution.as_deref().unwrap_or("CLI 默认"),
        ),
        None,
    );
    emit_changed(&state.db, &app, None).await;
    Ok(n)
}

/// 提交确认卡的全部内容：真实命令行 + 预计额度消耗 + 当前余额。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SubmitPreview {
    /// 每条一行，与真正 exec 的 argv 同源。
    pub commands: Vec<String>,
    /// 已知单价那部分的合计。**不含** `unpriced` 里的条目，所以它是**下限**不是总数。
    pub estimated_credits: i64,
    /// 查不到单价的组合（`model/res`，去重）—— 有值时预估必须标成「≥」。
    pub unpriced: Vec<String>,
    /// 提交前实拉的余额；拉不到（掉线/未登录）为 None，此时不拦人，只是不显示。
    pub balance: Option<i64>,
}

/// 提交前给人看的**真实命令行 + 这一下要花多少额度**。
///
/// 「我设了却没生效」这类怀疑只能靠把真实请求摆到确认之前来消除；与真正 exec 的 argv
/// 同源（`dreamina::command_line`），不是另写一份格式化字符串。
///
/// 额度预估同理，且更要紧：即梦**提交那一刻就扣费且不可撤回**，而通道之间差 5.5 倍
/// （4s/720p：`seedance2.0fast` 8 vs `seedance2.0fast_vip` 44）。18 条一批就是 144 与
/// 792 的区别 —— 这个数必须出现在「确认提交」按钮**旁边**，不是事后在报告里。
#[tauri::command]
#[specta::specta]
pub async fn preview_v2v_commands(
    state: State<'_, AppState>,
    ids: Vec<i64>,
) -> AppResult<SubmitPreview> {
    let s = load_settings(&state.db).await?;
    let defaults = s.defaults();
    // 展示的就是即将 exec 的那一串，所以这里也要解析成绝对路径 —— 顺带让「CLI 找不到」
    // 在花钱之前就报出来，而不是点了提交才发现。
    let bin = dreamina::resolve_bin(&s.bin)?;
    let mut commands = Vec::new();
    let mut estimated_credits = 0;
    let mut unpriced: Vec<String> = Vec::new();
    for clip in repo::list_ready(&state.db, &ids).await? {
        let opts = dreamina::normalize_opts(&runner::opts_for(&clip, &defaults))?;
        let argv = dreamina::command_line(
            &bin,
            &clip.image_path,
            clip.video_prompt.as_deref().unwrap_or(""),
            &opts,
        );
        commands.push(dreamina::display_command(&argv));
        // 三件套为空 = 走 CLI 默认路径，发什么模型我们不知道，价也就无从谈起。
        match (
            opts.model_version.as_deref(),
            opts.video_resolution.as_deref(),
            opts.duration,
        ) {
            (Some(m), Some(r), Some(d)) => match dreamina::estimate_credits(m, r, d) {
                Some(c) => estimated_credits += c,
                None => {
                    let key = format!("{m}/{r}");
                    if !unpriced.contains(&key) {
                        unpriced.push(key);
                    }
                }
            },
            _ => {
                let key = "跟随 CLI 默认".to_string();
                if !unpriced.contains(&key) {
                    unpriced.push(key);
                }
            }
        }
    }
    // 余额是**尽力而为**：网络抖一下不该挡住提交，少显示一行而已。
    let balance = dreamina::user_credit(&s.bin, &state.v2v_log)
        .await
        .ok()
        .map(|c| c.total_credit);
    Ok(SubmitPreview {
        commands,
        estimated_credits,
        unpriced,
        balance,
    })
}

/// 批量提交到即梦。
#[tauri::command]
#[specta::specta]
pub async fn submit_v2v_clips(
    state: State<'_, AppState>,
    app: AppHandle,
    ids: Vec<i64>,
) -> AppResult<SubmitSummary> {
    let s = load_settings(&state.db).await?;
    let sum = runner::submit_batch(&state.db, &s.bin, &ids, &s.defaults(), &state.v2v_log).await?;
    // 刚提交完人正盯着屏幕，而常规档位是 5/10 分钟 —— 请求一次 60 秒后的补扫。
    // 按批不按条：20 条一起提交也只多这一个进程。
    if sum.submitted > 0 {
        runner::request_sweep_soon(now_unix());
    }
    emit_changed(&state.db, &app, None).await;
    Ok(sum)
}

/// 立刻轮询一轮（用户点「刷新」；后台轮询器照常在跑）。
#[tauri::command]
#[specta::specta]
pub async fn poll_v2v_now(state: State<'_, AppState>, app: AppHandle) -> AppResult<i64> {
    let s = load_settings(&state.db).await?;
    state.v2v_log.info("poll", None, "手动查一次进度", None);
    // 手动点「查一次进度」要绕开退避（`force`）：人按下按钮就是要现在就问，
    // 而退避是给后台循环省成本的，不该反过来让人点了没反应。
    //
    // 用参数而不是把 `polled_at` 改成 0 去骗过退避判定：那样一旦这次查询失败，
    // 库里就留下一个 1970 年的时间戳 —— 卡片显示「55 年前查过」，而且此后每个 tick
    // 都判它到点，退避对这条彻底失效。
    let sum = runner::poll_once(
        &state.db,
        &state.dirs,
        &s.bin,
        s.timeout_secs(),
        true,
        &state.v2v_log,
    )
    .await?;
    emit_changed(&state.db, &app, None).await;
    Ok(sum.finished)
}

/// 一次可撤销动作的结果。
///
/// ## 为什么撤销令牌由 Rust 造、原样传回来
///
/// 「撤销」在验收流里不是锦上添花：看片流一秒判一条，手滑判错的概率接近 1，而错判
/// 「不通过」会把成片扔进废纸篓。但撤销**不能**让前端自己拼一条「把 stage 改回 rev」的
/// 命令 —— 那等于把状态机开给前端（铁律 1）。
///
/// 折中是：Rust 在动手**之前**取整份快照、封进令牌交给前端保管，撤销时原样传回，
/// 由 Rust 校验并写回。前端只当一个信封，令牌里每一个字段都是 Rust 自己写的。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct V2vAction {
    /// 真正改动了的条数（幂等跳过的不算）。
    pub changed: i64,
    /// 给人看的一句话：「已通过 3 条」。撤销 pill 上显示的就是它。
    pub label: String,
    /// 撤销令牌；为空表示这次没有可撤销的东西。
    pub undo: Vec<V2vUndoEntry>,
}

/// 一条 clip 的撤销原料 = 改动前的整份快照（+ 不通过时产生的废纸篓行）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct V2vUndoEntry {
    pub clip_id: i64,
    pub stage: String,
    pub video_prompt: Option<String>,
    pub submit_id: Option<String>,
    pub video_path: Option<String>,
    pub poster_path: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub fps: Option<f64>,
    pub duration_sec: Option<f64>,
    pub credit_count: Option<i64>,
    pub error_type: Option<String>,
    pub error_message: Option<String>,
    pub gen_status: Option<String>,
    pub queue_idx: Option<i64>,
    pub polled_at: Option<i64>,
    pub submitted_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub reviewed_at: Option<i64>,
    pub attempt: i64,
    /// 「不通过」时写进废纸篓的那一行。撤销即删掉它 —— 文件本来就没物理删，
    /// 所以这一步是干净的（清空废纸篓才会真删，而那时人已经确认过一次了）。
    pub trash_id: Option<i64>,
}

impl V2vUndoEntry {
    fn from_snapshot(s: repo::ClipSnapshot, trash_id: Option<i64>) -> Self {
        Self {
            clip_id: s.id,
            stage: s.stage,
            video_prompt: s.video_prompt,
            submit_id: s.submit_id,
            video_path: s.video_path,
            poster_path: s.poster_path,
            width: s.width,
            height: s.height,
            fps: s.fps,
            duration_sec: s.duration_sec,
            credit_count: s.credit_count,
            error_type: s.error_type,
            error_message: s.error_message,
            gen_status: s.gen_status,
            queue_idx: s.queue_idx,
            polled_at: s.polled_at,
            submitted_at: s.submitted_at,
            finished_at: s.finished_at,
            reviewed_at: s.reviewed_at,
            attempt: s.attempt,
            trash_id,
        }
    }

    fn to_snapshot(&self) -> repo::ClipSnapshot {
        repo::ClipSnapshot {
            id: self.clip_id,
            stage: self.stage.clone(),
            video_prompt: self.video_prompt.clone(),
            submit_id: self.submit_id.clone(),
            video_path: self.video_path.clone(),
            poster_path: self.poster_path.clone(),
            width: self.width,
            height: self.height,
            fps: self.fps,
            duration_sec: self.duration_sec,
            credit_count: self.credit_count,
            error_type: self.error_type.clone(),
            error_message: self.error_message.clone(),
            gen_status: self.gen_status.clone(),
            queue_idx: self.queue_idx,
            polled_at: self.polled_at,
            submitted_at: self.submitted_at,
            finished_at: self.finished_at,
            reviewed_at: self.reviewed_at,
            attempt: self.attempt,
        }
    }
}

/// 撤销上一次可撤销动作。
///
/// **只做写回，不做「反向操作」**：反向操作要为每种动作各写一遍逆变换，而逆变换写错了
/// 没人看得出来（撤销一次不通过 → 片子回来了但扣费记录没了）。整份写回只有一条路径。
///
/// 已被后续动作改过的条目不再撤销（`stage` 与令牌里记的「改动后应有的样子」对不上时
/// 强行写回会把新状态抹掉）—— 这里的判据是宽的：只要那一条现在不在快照里的旧态，
/// 就说明这次撤销仍然有意义；真正冲突的场景（撤销一条已经重新提交出去的）由
/// `submit_id` 变化挡住。
#[tauri::command]
#[specta::specta]
pub async fn undo_v2v(
    state: State<'_, AppState>,
    app: AppHandle,
    entries: Vec<V2vUndoEntry>,
) -> AppResult<i64> {
    let now = now_unix();
    let mut n = 0i64;
    // 一次 IN 查询取回整批当前状态：撤销是「看片流里连判了 20 条，⌘Z」，
    // 逐条 SELECT 加逐条事务只是把一个动作拆成几十轮往返。
    let ids: Vec<i64> = entries.iter().map(|e| e.clip_id).collect();
    let current = repo::get_many(&state.db, &ids).await?;
    let mut trash_ids: Vec<i64> = Vec::new();
    for e in &entries {
        // 已经重新提交出去的条目不能撤销回旧态：那会把新的 submit_id 抹掉，
        // 而那条任务在即梦那边正跑着、额度已经扣了 —— 抹掉它就再也认不出主人。
        let Some(cur) = current.iter().find(|c| c.id == e.clip_id) else {
            continue;
        };
        if cur.submit_id.is_some() && cur.submit_id != e.submit_id {
            continue;
        }
        // 撤销一次「通过」= 那条片子不再算交付，把交付拷贝收回来。
        // 不收的话 outputs/视频/ 下会留一个再也没有主人的文件——而人拿它去发布时，
        // 库里那条恰恰是没通过的（同 0025「包被删就该回落成待办」的道理）。
        if e.stage != "pass" {
            if let Some(p) = cur.export_path.clone() {
                let _ = tokio::task::spawn_blocking(move || std::fs::remove_file(&p)).await;
                let _ = repo::set_export_path(&state.db, e.clip_id, None).await;
            }
        }
        if repo::restore(&state.db, &e.to_snapshot(), now).await? {
            n += 1;
        }
        if let Some(tid) = e.trash_id {
            trash_ids.push(tid);
        }
    }
    if !trash_ids.is_empty() {
        let mut tx = state.db.begin().await?;
        trash_repo::delete_rows(&mut tx, &trash_ids).await?;
        tx.commit().await?;
    }
    refresh_handoff(&state.db, &app).await;
    Ok(n)
}

/// 视频验收：通过 / 不通过。不通过时成片进废纸篓（留封面 + 提示词记录）。
#[tauri::command]
#[specta::specta]
pub async fn review_v2v_clips(
    state: State<'_, AppState>,
    app: AppHandle,
    ids: Vec<i64>,
    pass: bool,
) -> AppResult<V2vAction> {
    let now = now_unix();
    // 交付目录读一次即可：一轮验收里它不会变，而每条读一次要多几十次 DB 往返。
    let settings = load_settings(&state.db).await?;
    let mut n = 0i64;
    let mut exported = 0i64;
    let mut undo: Vec<V2vUndoEntry> = Vec::new();
    let mut last_code = String::new();
    for id in ids {
        let Some(clip) = repo::get(&state.db, id).await? else {
            continue;
        };
        if clip.stage != "rev" {
            continue; // 幂等：连点/重复提交不得把已定态的再改一次
        }
        let snap = repo::snapshot(&state.db, id).await?;
        let mut trash_id: Option<i64> = None;
        if !pass {
            // 成片与封面进废纸篓待清理（同 E02：不立即物理删，误触不丢东西）。
            // 封面是 clip 自己的文件（首帧缩略图的副本），删它不会碰到作品缩略图。
            let mut files: Vec<String> = Vec::new();
            if let Some(v) = &clip.video_path {
                files.push(v.clone());
            }
            if let Some(p) = &clip.poster_path {
                files.push(p.clone());
            }
            let mut tx = state.db.begin().await?;
            let tid = trash_repo::insert(
                &mut tx,
                &trash_repo::NewTrashItem {
                    entity_type: "clip".into(),
                    ref_id: Some(clip.id),
                    thumb_path: clip.poster_path.clone(),
                    prompt_text: clip.video_prompt.clone(),
                    code: Some(clip.prompt_code.clone()),
                    title: Some(clip.group_name.clone()),
                    source_label: "视频验收未通过".into(),
                    file_paths: files,
                    payload_json: None,
                },
            )
            .await?;
            tx.commit().await?;
            trash_id = Some(tid);
        }
        if repo::set_reviewed(&state.db, id, if pass { "pass" } else { "rej" }, now).await? {
            n += 1;
            last_code = clip.prompt_code.clone();
            if pass {
                // 通过即交付：把成片从内部暂存区 clips/clip{id}.mp4 拷进
                // outputs/视频/{组}/{编号}_{日期}.mp4。图片验收通过时做的就是这件事，
                // 视频此前却只留在那个人在 Finder 里认不出谁是谁的内部目录里。
                match export_clip(&settings, &state, &clip).await {
                    Ok(Some(p)) => {
                        exported += 1;
                        let _ = repo::set_export_path(&state.db, id, Some(&p)).await;
                    }
                    Ok(None) => {}
                    // 拷贝失败不回滚验收（判定是人做的，文件是可以补的），但必须出声：
                    // 否则「通过了却没交付」会一直安静地不发生。
                    Err(e) => state.v2v_log.warn(
                        "review",
                        Some((clip.id, clip.prompt_code.as_str())),
                        format!("成片交付拷贝失败：{e}"),
                        None,
                    ),
                }
            }
            if let Some(s) = snap {
                undo.push(V2vUndoEntry::from_snapshot(s, trash_id));
            }
        }
    }
    emit_changed(&state.db, &app, None).await;
    let verb = if pass { "已通过" } else { "已不通过" };
    let mut label = action_label(verb, n, &last_code);
    if exported > 0 {
        label.push_str(&format!(" · 已交付 {exported} 条到输出目录"));
    }
    Ok(V2vAction {
        changed: n,
        label,
        undo,
    })
}

/// `{交付目录}/{组名}/{编号}_{日期}.mp4` —— 通过验收即交付的那份拷贝。
///
/// 交付目录默认 `{app_data}/outputs/视频`，用户可改（`V2vSettings::clips_output_dir`）。
/// 成片是 B-roll 素材，下游是剪辑而不是发布链 —— 它得落在人自己的工作目录里，
/// 而不是应用数据目录深处那个在 Finder 里找不到的位置。
///
/// **拷贝而不是移动**：clips/ 下那份是流水线自己的资产（封面、重跑、撤销都指着它），
/// 移走会让「撤销通过」变成一次搬回来的操作，而搬运是会失败的。
/// 磁盘代价是每条成片多占一份——几十 MB，换来的是撤销永远只改库不动文件。
///
/// 返回 `Ok(None)` = 这条根本没有成片文件（不该发生，但也不是错误）。
/// 交付一条成片：把 `clips/clip{id}.mp4` 拷进 `{交付目录}/{组}/{编号}_{日期}.mp4`。
///
/// **整个函数在 `spawn_blocking` 里跑**（见 [`export_clip`]）：一条成片几十 MB，
/// 而看片流里空格键判一条就走一次这里 —— 留在异步执行器上，判得越快卡得越狠。
fn export_clip_blocking(
    dir_base: std::path::PathBuf,
    clip: &repo::ClipRow,
) -> AppResult<Option<String>> {
    let Some(src) = clip.video_path.as_deref().filter(|p| !p.is_empty()) else {
        return Ok(None);
    };
    let group = if clip.group_name.trim().is_empty() {
        "未分组".to_string()
    } else {
        files::sanitize_filename(&clip.group_name)
    };
    let dir = dir_base.join(group);
    std::fs::create_dir_all(&dir).map_err(|e| AppError::Io(e.to_string()))?;

    // 编号去连字符，与图片输出命名同口径（files::output_filename）。
    let code = if clip.prompt_code.is_empty() {
        format!("clip{}", clip.id)
    } else {
        clip.prompt_code.replace('-', "")
    };
    let ext = std::path::Path::new(src)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("mp4")
        .to_lowercase();
    // 尝试次数进文件名：同一张图重跑几次都通过时，第二份不该悄悄盖掉第一份。
    let attempt = if clip.attempt > 1 {
        format!("_{}", clip.attempt)
    } else {
        String::new()
    };
    let name = format!(
        "{code}_{}{attempt}.{ext}",
        files::date_yymmdd(clip.reviewed_at.unwrap_or_else(now_unix))
    );
    let dest = dir.join(name);
    std::fs::copy(src, &dest).map_err(|e| AppError::Io(e.to_string()))?;
    Ok(Some(dest.to_string_lossy().to_string()))
}

/// [`export_clip_blocking`] 的异步包装：把那几十 MB 的拷贝挪出异步执行器。
async fn export_clip(
    settings: &V2vSettings,
    state: &State<'_, AppState>,
    clip: &repo::ClipRow,
) -> AppResult<Option<String>> {
    let base = settings.clips_dir(&state.dirs);
    let clip = clip.clone();
    tokio::task::spawn_blocking(move || export_clip_blocking(base, &clip))
        .await
        .map_err(|e| AppError::Io(format!("成片交付任务失败：{e}")))?
}

/// 重新交付：把成片再拷一次到当前交付目录。
///
/// 三种情况都会走到它，而它们在库里长得一模一样（`stage='pass'` 且 `export_path` 空
/// 或指向一个已经不存在的文件）：
/// 1. 验收那一刻拷贝失败了（磁盘满、目标目录不可写）—— 那时**不回滚验收**，
///    判定是人做的，文件是可以补的；
/// 2. 交付目录被改到了别处，旧成片还留在老位置；
/// 3. 人手动把交付出去的那份删了/移走了。
///
/// 之所以能补：`clips/clip{id}.mp4` 那份是流水线自己的资产，从来只拷不移。
#[tauri::command]
#[specta::specta]
pub async fn redeliver_v2v_clips(
    state: State<'_, AppState>,
    app: AppHandle,
    ids: Vec<i64>,
) -> AppResult<i64> {
    let settings = load_settings(&state.db).await?;
    let mut n = 0i64;
    let mut first_err: Option<String> = None;
    for id in ids {
        let Some(clip) = repo::get(&state.db, id).await? else {
            continue;
        };
        if clip.stage != "pass" {
            continue;
        }
        match export_clip(&settings, &state, &clip).await {
            Ok(Some(p)) => {
                repo::set_export_path(&state.db, id, Some(&p)).await?;
                n += 1;
            }
            // 没有成片文件就补不出来 —— 那不是交付失败，是这条根本没出片。
            Ok(None) => {}
            Err(e) => {
                let msg = e.to_string();
                state.v2v_log.warn(
                    "review",
                    Some((clip.id, clip.prompt_code.as_str())),
                    format!("重新交付失败：{msg}"),
                    None,
                );
                first_err.get_or_insert(msg);
            }
        }
    }
    // 一条都没成且确实有错 → 报出来。逐条静默会让「点了没反应」再次发生。
    if n == 0 {
        if let Some(e) = first_err {
            return Err(AppError::Io(e));
        }
    }
    emit_changed(&state.db, &app, None).await;
    Ok(n)
}

/// 在系统文件管理器打开成片交付目录。
#[tauri::command]
#[specta::specta]
pub async fn open_clips_output_dir(state: State<'_, AppState>, app: AppHandle) -> AppResult<()> {
    use tauri_plugin_opener::OpenerExt;
    let settings = load_settings(&state.db).await?;
    let dir = settings.clips_dir(&state.dirs);
    std::fs::create_dir_all(&dir).map_err(|e| AppError::Io(e.to_string()))?;
    app.opener()
        .open_path(dir.to_string_lossy().to_string(), None::<&str>)
        .map_err(|e| AppError::Io(e.to_string()))
}

/// 「已通过 BR31-0140」/「已通过 12 条」—— 一条时报编号，多条时报数量。
///
/// 一条时报编号是有用的：看片流一秒一条，撤销 pill 上写「已通过 1 条」等于什么都没说，
/// 而写出编号才能确认撤销的是不是刚才那一条。
fn action_label(verb: &str, n: i64, code: &str) -> String {
    if n == 1 && !code.is_empty() {
        format!("{verb} {code}")
    } else {
        format!("{verb} {n} 条")
    }
}

/// 重跑（同提示词）/ 退回改写 / 继续等待。
///
/// 默认是重跑：视频不通过多半是**没抽中**而不是提示词不对。
/// 但**判了超时的条目默认应当是「继续等待」**：超时只是我们这边不等了，即梦那边任务
/// 还在跑、额度已经扣了，而重跑会清掉 submit_id = 再花一份钱买同一条视频。
#[tauri::command]
#[specta::specta]
pub async fn requeue_v2v_clips(
    state: State<'_, AppState>,
    app: AppHandle,
    ids: Vec<i64>,
    mode: String,
) -> AppResult<V2vAction> {
    let verb = match mode.as_str() {
        "run" => "已重排待提交",
        "rewrite" => "已退回改写",
        "wait" => "已放回轮询",
        other => {
            return Err(AppError::InvalidInput(format!(
                "未知重排模式：{other}（只接受 run / rewrite / wait）"
            )))
        }
    };
    let now = now_unix();
    let mut n = 0i64;
    let mut undo: Vec<V2vUndoEntry> = Vec::new();
    let mut last_code = String::new();
    for id in ids {
        // 快照必须在改动之前取 —— 这三条路径都会清掉成片路径与扣费回执，
        // 事后再取就只剩清干净的空壳，撤销回去等于把片子弄丢。
        let snap = repo::snapshot(&state.db, id).await?;
        let code = repo::get(&state.db, id)
            .await?
            .map(|c| c.prompt_code)
            .unwrap_or_default();
        let ok = match mode.as_str() {
            "run" => repo::requeue_for_run(&state.db, id, now).await?,
            "rewrite" => repo::requeue_for_rewrite(&state.db, id, now).await?,
            _ => repo::resume_timed_out(&state.db, id, now).await?,
        };
        if ok {
            n += 1;
            last_code = code;
            // **收回这条 clip 的废纸篓行**。重跑是就地的：`v2v_clips` 只有一行，
            // 成片路径锚在 clip id 上（`clips/clip{id}.mp4`）。判过「不通过」的条目
            // 重跑之后，新片子会落到与旧片子完全相同的路径，而废纸篓里那行还指着它
            // —— 下一次清空废纸篓就会物理删掉一条还活着的成片。
            //
            // 代价是撤销这次重排后，那个文件不再有废纸篓行去回收它（最坏留下一个
            // 孤儿文件）。与「删掉正在用的成片」不对等，选这一边。
            let mut tx = state.db.begin().await?;
            let dropped = trash_repo::delete_by_clip(&mut tx, id).await?;
            tx.commit().await?;
            if dropped > 0 {
                state.v2v_log.info(
                    "review",
                    None,
                    format!("{last_code} 重排，同时收回它在废纸篓里的 {dropped} 条记录"),
                    None,
                );
            }
            if let Some(s) = snap {
                undo.push(V2vUndoEntry::from_snapshot(s, None));
            }
        }
    }
    if mode == "wait" && n > 0 {
        state.v2v_log.info(
            "poll",
            None,
            format!("{n} 条超时条目放回轮询（沿用原提交单，不重复扣额度）"),
            None,
        );
    }
    refresh_handoff(&state.db, &app).await;
    Ok(V2vAction {
        changed: n,
        label: action_label(verb, n, &last_code),
        undo,
    })
}

/// 从流水线移除（不想给这张图做视频了）。作品本体不受影响。
#[tauri::command]
#[specta::specta]
pub async fn remove_v2v_clips(
    state: State<'_, AppState>,
    app: AppHandle,
    ids: Vec<i64>,
) -> AppResult<i64> {
    let n = repo::remove(&state.db, &ids).await?;
    refresh_handoff(&state.db, &app).await;
    Ok(n)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即失败
mod tests {
    use super::*;

    // 撤销令牌走一趟前端再回来，必须一个字段都不掉。
    //
    // 它是 JSON 序列化过去、原样传回来的（前端只当信封），所以「加了列却忘了塞进
    // V2vUndoEntry」这类漏字段不会报错 —— 它会安静地把撤销变成一次数据丢失。
    #[test]
    fn undo_token_survives_the_round_trip_intact() {
        let snap = repo::ClipSnapshot {
            id: 7,
            stage: "rev".into(),
            video_prompt: Some("提示词".into()),
            submit_id: Some("sub-1".into()),
            video_path: Some("/clips/7.mp4".into()),
            poster_path: Some("/clips/7.jpg".into()),
            width: Some(720),
            height: Some(1280),
            fps: Some(24.0),
            duration_sec: Some(4.0),
            credit_count: Some(8),
            error_type: None,
            error_message: None,
            gen_status: Some("success".into()),
            queue_idx: Some(4485),
            polled_at: Some(1000),
            submitted_at: Some(900),
            finished_at: Some(990),
            reviewed_at: None,
            attempt: 2,
        };
        let entry = V2vUndoEntry::from_snapshot(snap.clone(), Some(42));
        let wire = serde_json::to_string(&entry).unwrap();
        let back: V2vUndoEntry = serde_json::from_str(&wire).unwrap();
        assert_eq!(
            back.trash_id,
            Some(42),
            "废纸篓行 id 必须留住，否则撤销后片子仍在废纸篓里"
        );

        let out = back.to_snapshot();
        assert_eq!(out.id, snap.id);
        assert_eq!(out.stage, snap.stage);
        assert_eq!(out.video_path, snap.video_path);
        assert_eq!(out.poster_path, snap.poster_path);
        assert_eq!(out.credit_count, snap.credit_count);
        assert_eq!(out.submit_id, snap.submit_id);
        assert_eq!(out.queue_idx, snap.queue_idx);
        assert_eq!(out.attempt, snap.attempt);
        assert_eq!(out.reviewed_at, snap.reviewed_at);
        // 载荷字段须 camelCase（specta 序列化配置统一保证，这里守住不被手改破坏）。
        assert!(wire.contains("\"clipId\""), "{wire}");
        assert!(wire.contains("\"videoPath\""), "{wire}");
    }

    // 一条时报编号、多条时报数量 —— 看片流一秒一条，「已通过 1 条」等于什么都没说。
    #[test]
    fn action_label_names_the_clip_when_there_is_only_one() {
        assert_eq!(action_label("已通过", 1, "BR31-0140"), "已通过 BR31-0140");
        assert_eq!(action_label("已通过", 12, "BR31-0140"), "已通过 12 条");
        // 编号取不到时不能拼出「已通过 」这种半截话。
        assert_eq!(action_label("已不通过", 1, ""), "已不通过 1 条");
    }

    // 前端的空输入框不该变成 `--model_version=` 这种必被拒的空 flag。
    #[test]
    fn blank_settings_fold_to_none_not_empty_flags() {
        let s = V2vSettings {
            model_version: "  ".into(),
            video_resolution: String::new(),
            ..Default::default()
        };
        let d = s.defaults();
        assert!(d.model_version.is_none());
        assert!(d.video_resolution.is_none());
        // 全空 → 走 CLI 默认路径，一个高级 flag 都不发。
        let n = dreamina::normalize_opts(&d).unwrap();
        assert!(n.model_version.is_none());
    }

    #[test]
    fn settings_trim_model_name() {
        let s = V2vSettings {
            model_version: " seedance2.0fast ".into(),
            ..Default::default()
        };
        assert_eq!(
            s.defaults().model_version.as_deref(),
            Some("seedance2.0fast")
        );
    }

    // 默认设置必须是合法组合：否则装完就用，第一次提交才发现设置存了个非法组合。
    #[test]
    fn default_settings_are_a_valid_combo() {
        let s = V2vSettings::default();
        assert!(dreamina::normalize_opts(&s.defaults()).is_ok());
        assert_eq!(s.bin, "dreamina");
        assert!(s.poll_enabled);
        assert!(s.root().ends_with("GenDesk交接"), "{:?}", s.root());
    }

    // 交接根为空串时回落到默认位置，不能拼出一个相对路径乱写文件。
    #[test]
    fn blank_handoff_root_falls_back_to_default() {
        let s = V2vSettings {
            handoff_root: "   ".into(),
            ..Default::default()
        };
        assert_eq!(s.root(), handoff::default_root());
    }

    // 损坏/缺失的设置 JSON 必须回落默认值，绝不让整页打不开。
    #[test]
    fn corrupt_settings_json_falls_back_to_default() {
        let parsed = serde_json::from_str::<V2vSettings>("{ 坏 json").ok();
        assert!(parsed.is_none());
        let s = parsed.unwrap_or_default();
        assert_eq!(s.bin, "dreamina");
        // 部分字段缺失也要能读（serde default 逐字段兜底）。
        let partial: V2vSettings = serde_json::from_str(r#"{"bin":"/opt/dreamina"}"#).unwrap();
        assert_eq!(partial.bin, "/opt/dreamina");
        assert!(partial.poll_enabled, "缺失的布尔项须回落到默认 true");
        assert!(!partial.handoff_root.is_empty(), "缺失的交接根须回落默认值");
    }
}
