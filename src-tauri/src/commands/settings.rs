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
}

impl Settings {
    fn defaults(output_dir: String) -> Self {
        Self {
            schedule_strategy: "round_robin".to_string(),
            retry_count: 1,
            output_dir,
            motion: "standard".to_string(),
            paused: false,
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
}

async fn load(state: &AppState) -> AppResult<Settings> {
    let default_dir = state.dirs.outputs().to_string_lossy().to_string();
    match repo::get_raw(&state.db).await? {
        Some(json) => {
            let mut s: Settings = serde_json::from_str(&json)
                .unwrap_or_else(|_| Settings::defaults(default_dir.clone()));
            s.sanitize();
            Ok(s)
        }
        None => Ok(Settings::defaults(default_dir)),
    }
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
    s.sanitize();
    save(&state, &s).await?;
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
