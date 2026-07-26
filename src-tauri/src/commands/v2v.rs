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
use crate::state::AppState;
use crate::v2v::activity::ActivityEntry;
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
    /// 默认模型。空 = 不发高级控制，走 CLI 自己的默认路径（最稳，不锁定模型名）。
    #[serde(default)]
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
}

fn d_root() -> String {
    handoff::default_root().to_string_lossy().to_string()
}
fn d_bin() -> String {
    dreamina::DEFAULT_BIN.to_string()
}
fn d_true() -> bool {
    true
}

impl Default for V2vSettings {
    fn default() -> Self {
        Self {
            handoff_root: d_root(),
            bin: d_bin(),
            model_version: String::new(),
            duration: None,
            video_resolution: String::new(),
            session: None,
            poll_enabled: true,
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
    pub fn root(&self) -> std::path::PathBuf {
        if self.handoff_root.trim().is_empty() {
            handoff::default_root()
        } else {
            std::path::PathBuf::from(self.handoff_root.trim())
        }
    }
}

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
    pub spent_7d: i64,
    pub spent_24h: i64,
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
    out.spent_24h = repo::credit_since(&state.db, now - 24 * 3600).await?;
    out.spent_7d = repo::credit_since(&state.db, now - 7 * 24 * 3600).await?;
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
    pub accepted_at: i64,
    pub updated_at: i64,
}

impl From<repo::ClipRow> for ClipView {
    fn from(r: repo::ClipRow) -> Self {
        Self {
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
    group_id: Option<i64>,
    group_name: Option<String>,
    batch_id: Option<i64>,
    prompt_text: String,
}

/// 入队若干作品，返回真正新增的条数。验收命令与手动入队共用。
pub async fn enqueue_works(pool: &SqlitePool, work_ids: &[i64]) -> AppResult<i64> {
    if work_ids.is_empty() {
        return Ok(0);
    }
    let now = now_unix();
    let mut added = 0i64;
    for wid in work_ids {
        let row: Option<QueueSeed> = sqlx::query_as(
            "SELECT w.group_id, g.name AS group_name, w.batch_id, w.prompt_text
               FROM accepted_works w LEFT JOIN prompt_groups g ON g.id = w.group_id
              WHERE w.id = ?1",
        )
        .bind(wid)
        .fetch_optional(pool)
        .await?;
        let Some(seed) = row else {
            continue;
        };
        let mut tx = pool.begin().await?;
        if repo::enqueue(
            &mut tx,
            *wid,
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
        tx.commit().await?;
    }
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

/// 提交前给人看的**真实命令行**（每条一行）。
///
/// 「我设了却没生效」这类怀疑只能靠把真实请求摆到确认之前来消除；与真正 exec 的 argv
/// 同源（`dreamina::command_line`），不是另写一份格式化字符串。
#[tauri::command]
#[specta::specta]
pub async fn preview_v2v_commands(
    state: State<'_, AppState>,
    ids: Vec<i64>,
) -> AppResult<Vec<String>> {
    let s = load_settings(&state.db).await?;
    let defaults = s.defaults();
    // 展示的就是即将 exec 的那一串，所以这里也要解析成绝对路径 —— 顺带让「CLI 找不到」
    // 在花钱之前就报出来，而不是点了提交才发现。
    let bin = dreamina::resolve_bin(&s.bin)?;
    let mut out = Vec::new();
    for clip in repo::take_ready(&state.db, &ids).await? {
        let opts = dreamina::normalize_opts(&runner::opts_for(&clip, &defaults))?;
        let argv = dreamina::command_line(
            &bin,
            &clip.image_path,
            clip.video_prompt.as_deref().unwrap_or(""),
            &opts,
        );
        out.push(dreamina::display_command(&argv));
    }
    Ok(out)
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
    emit_changed(&state.db, &app, None).await;
    Ok(sum)
}

/// 立刻轮询一轮（用户点「刷新」；后台轮询器照常在跑）。
#[tauri::command]
#[specta::specta]
pub async fn poll_v2v_now(state: State<'_, AppState>, app: AppHandle) -> AppResult<i64> {
    let s = load_settings(&state.db).await?;
    state.v2v_log.info("poll", None, "手动查一次进度", None);
    let sum = runner::poll_once(&state.db, &state.dirs, &s.bin, &state.v2v_log).await?;
    emit_changed(&state.db, &app, None).await;
    Ok(sum.finished)
}

/// 视频验收：通过 / 不通过。不通过时成片进废纸篓（留封面 + 提示词记录）。
#[tauri::command]
#[specta::specta]
pub async fn review_v2v_clips(
    state: State<'_, AppState>,
    app: AppHandle,
    ids: Vec<i64>,
    pass: bool,
) -> AppResult<i64> {
    let now = now_unix();
    let mut n = 0i64;
    for id in ids {
        let Some(clip) = repo::get(&state.db, id).await? else {
            continue;
        };
        if clip.stage != "rev" {
            continue; // 幂等：连点/重复提交不得把已定态的再改一次
        }
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
            trash_repo::insert(
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
                },
            )
            .await?;
            tx.commit().await?;
        }
        if repo::set_reviewed(&state.db, id, if pass { "pass" } else { "rej" }, now).await? {
            n += 1;
        }
    }
    emit_changed(&state.db, &app, None).await;
    Ok(n)
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
) -> AppResult<i64> {
    let now = now_unix();
    let mut n = 0i64;
    for id in ids {
        let ok = match mode.as_str() {
            "run" => repo::requeue_for_run(&state.db, id, now).await?,
            "rewrite" => repo::requeue_for_rewrite(&state.db, id, now).await?,
            "wait" => repo::resume_timed_out(&state.db, id, now).await?,
            other => {
                return Err(AppError::InvalidInput(format!(
                    "未知重排模式：{other}（只接受 run / rewrite / wait）"
                )))
            }
        };
        if ok {
            n += 1;
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
    Ok(n)
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
