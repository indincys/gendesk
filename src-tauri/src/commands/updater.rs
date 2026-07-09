//! 应用内更新（执行计划 4.1 / 需求 4.2）。
//!
//! 启动检查 → 后台下载 → 前台「已就绪 · 重启安装」→ 确认 relaunch。
//! 更新过程经 `update://state` 事件驱动前端 UI。

use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::AppHandle;
use tauri_plugin_updater::{Update, UpdaterExt};
use tauri_specta::Event;

use crate::error::{AppError, AppResult};

/// 已下载、待安装的更新（check 后暂存，install 时取用）。
#[derive(Default)]
pub struct PendingUpdate(pub Mutex<Option<(Update, Vec<u8>)>>);

/// `update://state`
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStateChanged {
    /// checking / downloading / ready / uptodate / error
    pub state: String,
    pub version: Option<String>,
}

fn emit(app: &AppHandle, state: &str, version: Option<String>) {
    let _ = UpdateStateChanged {
        state: state.to_string(),
        version,
    }
    .emit(app);
}

/// 手动/启动检查更新：有新版则后台下载并暂存，emit ready。
#[tauri::command]
#[specta::specta]
pub async fn check_update_now(
    app: AppHandle,
    pending: tauri::State<'_, PendingUpdate>,
) -> AppResult<Option<String>> {
    emit(&app, "checking", None);
    let updater = app
        .updater()
        .map_err(|e| AppError::Internal(format!("更新器初始化失败：{e}")))?;

    let update = match updater.check().await {
        Ok(Some(u)) => u,
        Ok(None) => {
            emit(&app, "uptodate", None);
            return Ok(None);
        }
        Err(e) => {
            emit(&app, "error", None);
            return Err(AppError::Internal(format!("检查更新失败：{e}")));
        }
    };

    let version = update.version.clone();
    emit(&app, "downloading", Some(version.clone()));
    let bytes = update
        .download(|_chunk, _total| {}, || {})
        .await
        .map_err(|e| {
            emit(&app, "error", Some(version.clone()));
            AppError::Internal(format!("下载更新失败：{e}"))
        })?;

    if let Ok(mut slot) = pending.0.lock() {
        *slot = Some((update, bytes));
    }
    emit(&app, "ready", Some(version.clone()));
    Ok(Some(version))
}

/// 安装已下载的更新并重启（用户确认「重启安装」时调用）。
#[tauri::command]
#[specta::specta]
pub async fn install_update(
    app: AppHandle,
    pending: tauri::State<'_, PendingUpdate>,
) -> AppResult<()> {
    let taken = pending.0.lock().ok().and_then(|mut s| s.take());
    let (update, bytes) = taken.ok_or_else(|| AppError::InvalidInput("没有待安装的更新".into()))?;

    update
        .install(bytes)
        .map_err(|e| AppError::Internal(format!("安装更新失败：{e}")))?;
    app.restart();
}
