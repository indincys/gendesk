//! 交付域命令：交付根设置 + 打开目录。
//!
//! 交付本身没有按钮——验收通过即交付（`commands::review::accept_tasks`）。
//! 这里只回答两件事：「交付到哪里」与「让我看看那个目录」。

use serde::{Deserialize, Serialize};
use specta::Type;
use sqlx::SqlitePool;
use tauri::{AppHandle, State};

use crate::db::repo::settings as settings_repo;
use crate::delivery;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

const KEY: &str = "delivery";

/// 交付设置（`settings` 表 key='delivery' 单行 JSON）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DeliverySettings {
    /// 交付根目录。`自动发布` 那一侧监听它。
    #[serde(default = "d_root")]
    pub root: String,
}

fn d_root() -> String {
    delivery::default_root().to_string_lossy().to_string()
}

impl Default for DeliverySettings {
    fn default() -> Self {
        Self { root: d_root() }
    }
}

impl DeliverySettings {
    pub fn root_path(&self) -> std::path::PathBuf {
        if self.root.trim().is_empty() {
            delivery::default_root()
        } else {
            std::path::PathBuf::from(self.root.trim())
        }
    }
}

/// 读设置（缺失/损坏都回默认值，绝不让整页打不开）。
pub async fn load_settings(pool: &SqlitePool) -> AppResult<DeliverySettings> {
    let raw = settings_repo::get_by_key(pool, KEY).await?;
    Ok(raw
        .and_then(|j| serde_json::from_str::<DeliverySettings>(&j).ok())
        .unwrap_or_default())
}

#[tauri::command]
#[specta::specta]
pub async fn get_delivery_settings(state: State<'_, AppState>) -> AppResult<DeliverySettings> {
    load_settings(&state.db).await
}

#[tauri::command]
#[specta::specta]
pub async fn update_delivery_settings(
    state: State<'_, AppState>,
    settings: DeliverySettings,
) -> AppResult<DeliverySettings> {
    let json = serde_json::to_string(&settings)
        .map_err(|e| AppError::Internal(format!("设置序列化失败：{e}")))?;
    settings_repo::set_by_key(&state.db, KEY, &json).await?;
    load_settings(&state.db).await
}

#[tauri::command]
#[specta::specta]
pub async fn pick_delivery_root(app: AppHandle) -> AppResult<Option<String>> {
    use tauri_plugin_dialog::DialogExt;
    let picked = app.dialog().file().blocking_pick_folder();
    Ok(picked.and_then(|p| p.into_path().ok().map(|p| p.to_string_lossy().to_string())))
}

/// 在系统文件管理器打开交付根。
#[tauri::command]
#[specta::specta]
pub async fn open_delivery_dir(state: State<'_, AppState>, app: AppHandle) -> AppResult<()> {
    use tauri_plugin_opener::OpenerExt;
    let dir = load_settings(&state.db).await?.root_path();
    std::fs::create_dir_all(&dir).map_err(|e| AppError::Io(e.to_string()))?;
    app.opener()
        .open_path(dir.to_string_lossy().to_string(), None::<&str>)
        .map_err(|e| AppError::Io(e.to_string()))
}
