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
use crate::v2v::dreamina::{self, GenOpts, ModelInfo};
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

/// 受控模型清单（前端选择器渲染源，单点在 `v2v::dreamina`）。
#[tauri::command]
#[specta::specta]
pub async fn v2v_models() -> AppResult<Vec<ModelInfo>> {
    Ok(dreamina::models())
}

/// 查即梦余额（设置页显示 + 批量提交前预检）。
#[tauri::command]
#[specta::specta]
pub async fn v2v_credit(state: State<'_, AppState>) -> AppResult<i64> {
    let s = load_settings(&state.db).await?;
    dreamina::user_credit(&s.bin).await
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
    emit_changed(&state.db, &app, Some(id)).await;
    Ok(ok)
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
    let mut out = Vec::new();
    for clip in repo::take_ready(&state.db, &ids).await? {
        let opts = dreamina::normalize_opts(&runner::opts_for(&clip, &defaults))?;
        let argv = dreamina::command_line(
            &s.bin,
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
    let sum = runner::submit_batch(&state.db, &s.bin, &ids, &s.defaults()).await?;
    emit_changed(&state.db, &app, None).await;
    Ok(sum)
}

/// 立刻轮询一轮（用户点「刷新」；后台轮询器照常在跑）。
#[tauri::command]
#[specta::specta]
pub async fn poll_v2v_now(state: State<'_, AppState>, app: AppHandle) -> AppResult<i64> {
    let s = load_settings(&state.db).await?;
    let sum = runner::poll_once(&state.db, &state.dirs, &s.bin).await?;
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

/// 重跑（同提示词）/ 退回改写。
///
/// 默认是重跑：视频不通过多半是**没抽中**而不是提示词不对。
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
            other => {
                return Err(AppError::InvalidInput(format!(
                    "未知重排模式：{other}（只接受 run / rewrite）"
                )))
            }
        };
        if ok {
            n += 1;
        }
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
