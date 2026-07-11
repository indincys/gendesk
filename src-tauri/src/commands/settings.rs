//! settings 域命令（执行计划 2.1 / 1.3）。

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

use crate::db::repo::settings as repo;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// 应用设置（单行 JSON 持久化）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// 调度策略："round_robin" | "success_rate"
    pub schedule_strategy: String,
    /// 失败重试次数（0–3）
    pub retry_count: i64,
    /// 输出根目录
    pub output_dir: String,
    /// 动效偏好："standard" | "reduced"
    pub motion: String,
    /// 队列暂停态
    pub paused: bool,
    /// 全局熔断阈值（E05）：跨 Key 连续失败达此数自动暂停队列；0 = 关闭。
    #[serde(default = "default_global_fail_threshold")]
    pub global_fail_threshold: i64,
    /// 废纸篓保留天数（E40 / D3）：删除项保留满此天数后启动时自动物理清理；0 = 不自动清理。
    #[serde(default = "default_retention_days")]
    pub trash_retention_days: i64,
    /// 归档批次保留天数（E22 / D3）：批次归档满此天数后启动时自动删除（作品不受影响）；0 = 不自动删除。
    #[serde(default = "default_retention_days")]
    pub batch_retention_days: i64,
    /// 首次使用引导是否已完成（E13）：四步齐备后置 true，引导永久消失。
    #[serde(default)]
    pub onboarded: bool,
}

/// 全局熔断默认阈值（连续失败 10 次）。
fn default_global_fail_threshold() -> i64 {
    10
}

/// 保留期默认 30 天（D3）。
fn default_retention_days() -> i64 {
    30
}

impl Settings {
    fn defaults(output_dir: String) -> Self {
        Self {
            schedule_strategy: "round_robin".to_string(),
            retry_count: 1,
            output_dir,
            motion: "standard".to_string(),
            paused: false,
            global_fail_threshold: default_global_fail_threshold(),
            trash_retention_days: default_retention_days(),
            batch_retention_days: default_retention_days(),
            onboarded: false,
        }
    }

    fn sanitize(&mut self) {
        if self.schedule_strategy != "round_robin" && self.schedule_strategy != "success_rate" {
            self.schedule_strategy = "round_robin".to_string();
        }
        if self.motion != "standard" && self.motion != "reduced" {
            self.motion = "standard".to_string();
        }
        self.retry_count = self.retry_count.clamp(0, 3);
        // 0 = 关闭；否则至少 3 起（太低会误伤偶发失败），上限 100。
        if self.global_fail_threshold != 0 {
            self.global_fail_threshold = self.global_fail_threshold.clamp(3, 100);
        }
        // 保留天数：0 = 关闭；否则至少 1 天，上限 365。
        if self.trash_retention_days != 0 {
            self.trash_retention_days = self.trash_retention_days.clamp(1, 365);
        }
        if self.batch_retention_days != 0 {
            self.batch_retention_days = self.batch_retention_days.clamp(1, 365);
        }
    }
}

/// 设置补丁（部分更新）。
#[derive(Debug, Clone, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SettingsPatch {
    pub schedule_strategy: Option<String>,
    pub retry_count: Option<i64>,
    pub output_dir: Option<String>,
    pub motion: Option<String>,
    pub paused: Option<bool>,
    pub global_fail_threshold: Option<i64>,
    pub trash_retention_days: Option<i64>,
    pub batch_retention_days: Option<i64>,
    pub onboarded: Option<bool>,
}

/// 从连接池加载设置（供引擎启动读取策略/重试/暂停态）。
pub async fn load_settings(
    pool: &sqlx::SqlitePool,
    default_output_dir: &str,
) -> AppResult<Settings> {
    match repo::get_raw(pool).await? {
        Some(json) => {
            let mut s: Settings = serde_json::from_str(&json)
                .unwrap_or_else(|_| Settings::defaults(default_output_dir.to_string()));
            s.sanitize();
            Ok(s)
        }
        None => Ok(Settings::defaults(default_output_dir.to_string())),
    }
}

async fn load(state: &AppState) -> AppResult<Settings> {
    let default_dir = state.dirs.outputs().to_string_lossy().to_string();
    load_settings(&state.db, &default_dir).await
}

async fn save(state: &AppState, s: &Settings) -> AppResult<()> {
    let json = serde_json::to_string(s)?;
    repo::set_raw(&state.db, &json).await?;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn get_settings(state: State<'_, AppState>) -> AppResult<Settings> {
    load(&state).await
}

#[tauri::command]
#[specta::specta]
pub async fn update_settings(
    state: State<'_, AppState>,
    patch: SettingsPatch,
) -> AppResult<Settings> {
    let mut s = load(&state).await?;
    if let Some(v) = patch.schedule_strategy {
        s.schedule_strategy = v;
    }
    if let Some(v) = patch.retry_count {
        s.retry_count = v;
    }
    if let Some(v) = patch.output_dir {
        s.output_dir = v;
    }
    if let Some(v) = patch.motion {
        s.motion = v;
    }
    if let Some(v) = patch.paused {
        s.paused = v;
    }
    if let Some(v) = patch.global_fail_threshold {
        s.global_fail_threshold = v;
    }
    if let Some(v) = patch.trash_retention_days {
        s.trash_retention_days = v;
    }
    if let Some(v) = patch.batch_retention_days {
        s.batch_retention_days = v;
    }
    if let Some(v) = patch.onboarded {
        s.onboarded = v;
    }
    s.sanitize();
    save(&state, &s).await?;

    // 实时应用到引擎。
    state
        .engine
        .set_strategy(crate::engine::strategy::Strategy::from_str_or_default(
            &s.schedule_strategy,
        ));
    state.engine.set_user_retry(s.retry_count.max(0) as u32);
    state
        .engine
        .set_global_fail_threshold(s.global_fail_threshold.max(0) as u32);
    if s.paused {
        state.engine.pause();
    } else {
        state.engine.resume();
    }
    Ok(s)
}

/// 选择输出根目录（返回所选路径；取消返回 None）。
#[tauri::command]
#[specta::specta]
pub async fn pick_output_dir(app: AppHandle) -> AppResult<Option<String>> {
    let picked = app.dialog().file().blocking_pick_folder();
    Ok(picked
        .and_then(|p| p.into_path().ok())
        .map(|p| p.to_string_lossy().to_string()))
}

/// 选择单个 .txt 文件（提示词导入）。
#[tauri::command]
#[specta::specta]
pub async fn pick_txt_file(app: AppHandle) -> AppResult<Option<String>> {
    let picked = app
        .dialog()
        .file()
        .add_filter("文本文件", &["txt"])
        .blocking_pick_file();
    Ok(picked
        .and_then(|p| p.into_path().ok())
        .map(|p| p.to_string_lossy().to_string()))
}

/// 选择多张图片（参考图上传）。
#[tauri::command]
#[specta::specta]
pub async fn pick_image_files(app: AppHandle) -> AppResult<Vec<String>> {
    let picked = app
        .dialog()
        .file()
        .add_filter("图片", &["png", "jpg", "jpeg", "webp", "bmp"])
        .blocking_pick_files();
    Ok(picked
        .map(|list| {
            list.into_iter()
                .filter_map(|p| p.into_path().ok())
                .map(|p| p.to_string_lossy().to_string())
                .collect()
        })
        .unwrap_or_default())
}

/// 复制诊断信息（E27）：版本 / OS / Key 数量与状态 / 最近 5 条错误摘要。
/// 明确不含 Key 明文（仅计数与启用/熔断状态），可安全粘贴给支持者。
#[tauri::command]
#[specta::specta]
pub async fn diagnostics_info(state: State<'_, AppState>) -> AppResult<String> {
    let version = env!("CARGO_PKG_VERSION");
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;

    let (total, enabled, broken): (i64, i64, i64) = sqlx::query_as(
        "SELECT COUNT(*),
                COALESCE(SUM(enabled), 0),
                COALESCE(SUM(circuit_broken), 0)
         FROM api_keys",
    )
    .fetch_one(&state.db)
    .await?;

    // 最近 5 条错误摘要（类型 + 截断消息，不含敏感内容）。
    let errs: Vec<(Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT error_type, error_message FROM task_attempts
         WHERE outcome = 'error' ORDER BY id DESC LIMIT 5",
    )
    .fetch_all(&state.db)
    .await?;

    let mut out = format!(
        "GenDesk 诊断信息\n版本: v{version}\n系统: {os}/{arch}\nAPI Key: 共 {total} · 启用 {enabled} · 已熔断 {broken}\n最近错误:\n"
    );
    if errs.is_empty() {
        out.push_str("  （无）\n");
    } else {
        for (et, msg) in errs {
            let et = et.unwrap_or_else(|| "Other".into());
            let mut m = msg.unwrap_or_default();
            if m.chars().count() > 80 {
                m = m.chars().take(80).collect::<String>() + "…";
            }
            out.push_str(&format!("  [{et}] {m}\n"));
        }
    }
    Ok(out)
}

/// 在系统文件管理器中打开日志目录。
#[tauri::command]
#[specta::specta]
pub async fn open_logs_dir(state: State<'_, AppState>, app: AppHandle) -> AppResult<()> {
    let path = state.dirs.logs().to_string_lossy().to_string();
    app.opener()
        .open_path(path, None::<&str>)
        .map_err(|e| AppError::Io(e.to_string()))
}

/// 在系统文件管理器中显示指定路径（打开所在文件夹）。
#[tauri::command]
#[specta::specta]
pub async fn open_path_in_folder(app: AppHandle, path: String) -> AppResult<()> {
    app.opener()
        .reveal_item_in_dir(&path)
        .map_err(|e| AppError::Io(e.to_string()))
}
