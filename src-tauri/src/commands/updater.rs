//! 应用内更新（执行计划 4.1 / 需求 4.2）。
//!
//! 启动后与每隔 [`CHECK_EVERY`] 由 Rust 自查一次 → 后台下载 → 前台「已就绪 · 重启安装」→
//! 用户确认后安装并重启。应用常被 Agent CLI 后台拉起、一开就是几天，只在启动时查一次
//! 等于永远收不到新版，所以检查放在 Rust 里定时跑，而不是挂在某个页面上。
//! 更新过程经 `update://state` 事件驱动前端 UI；页面晚于事件挂载时用 [`get_update_state`] 补读。

use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};
use tauri_specta::Event;

use crate::error::{AppError, AppResult};

/// 启动后第一次检查的延迟：让界面、调度器与控制通道先起来，不和启动抢网络与磁盘。
const FIRST_CHECK: Duration = Duration::from_secs(20);
/// 之后的检查间隔。
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

/// 更新状态：已下载待安装的包 + 最近一次状态（供晚挂载的页面补读）。
#[derive(Default)]
pub struct PendingUpdate {
    ready: Mutex<Option<(Update, Vec<u8>)>>,
    last: Mutex<Option<UpdateStateChanged>>,
    /// 同一时刻只跑一次检查（定时与手动可能撞在一起，各下一遍是白下）。
    busy: tokio::sync::Mutex<()>,
}

/// `update://state`
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStateChanged {
    /// idle / checking / downloading / ready / uptodate / error
    pub state: String,
    pub version: Option<String>,
}

fn emit(app: &AppHandle, state: &str, version: Option<String>) {
    let event = UpdateStateChanged {
        state: state.to_string(),
        version,
    };
    if let Ok(mut last) = app.state::<PendingUpdate>().last.lock() {
        *last = Some(event.clone());
    }
    let _ = event.emit(app);
}

/// 定时自查。失败只记日志，下一轮再试。
pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK).await;
        loop {
            if let Err(error) = check(&app).await {
                tracing::warn!(%error, "定时检查更新失败");
            }
            tokio::time::sleep(CHECK_EVERY).await;
        }
    });
}

/// 检查并后台下载；已下载过同一版本就直接报就绪，不重复下载。
async fn check(app: &AppHandle) -> AppResult<Option<String>> {
    let pending = app.state::<PendingUpdate>();
    let _guard = pending.busy.lock().await;
    let ready_version = pending
        .ready
        .lock()
        .ok()
        .and_then(|slot| slot.as_ref().map(|(u, _)| u.version.clone()));

    emit(app, "checking", None);
    let updater = app
        .updater()
        .map_err(|e| AppError::Internal(format!("更新器初始化失败：{e}")))?;
    let update = match updater.check().await {
        Ok(Some(u)) => u,
        Ok(None) => {
            emit(app, "uptodate", None);
            return Ok(None);
        }
        Err(e) => {
            // 已有下载好的包时，一次网络失败不该把「重启安装」按钮收掉。
            match ready_version {
                Some(v) => emit(app, "ready", Some(v)),
                None => emit(app, "error", None),
            }
            return Err(AppError::Internal(format!("检查更新失败：{e}")));
        }
    };

    let version = update.version.clone();
    if ready_version.as_deref() == Some(version.as_str()) {
        emit(app, "ready", Some(version.clone()));
        return Ok(Some(version));
    }
    emit(app, "downloading", Some(version.clone()));
    let bytes = match update.download(|_chunk, _total| {}, || {}).await {
        Ok(bytes) => bytes,
        Err(e) => {
            emit(app, "error", Some(version.clone()));
            return Err(AppError::Internal(format!("下载更新失败：{e}")));
        }
    };
    if let Ok(mut slot) = pending.ready.lock() {
        *slot = Some((update, bytes));
    }
    emit(app, "ready", Some(version.clone()));
    Ok(Some(version))
}

/// 手动检查更新：有新版则后台下载并暂存，返回版本号；已是最新返回 None。
#[tauri::command]
#[specta::specta]
pub async fn check_update_now(app: AppHandle) -> AppResult<Option<String>> {
    check(&app).await
}

/// 当前更新状态（页面晚于事件挂载时补读）。
#[tauri::command]
#[specta::specta]
pub async fn get_update_state(
    pending: tauri::State<'_, PendingUpdate>,
) -> AppResult<UpdateStateChanged> {
    Ok(pending
        .last
        .lock()
        .ok()
        .and_then(|last| last.clone())
        .unwrap_or(UpdateStateChanged {
            state: "idle".into(),
            version: None,
        }))
}

/// 安装已下载的更新并重启（用户确认「重启安装」时调用）。
#[tauri::command]
#[specta::specta]
pub async fn install_update(
    app: AppHandle,
    pending: tauri::State<'_, PendingUpdate>,
) -> AppResult<()> {
    let taken = pending.ready.lock().ok().and_then(|mut s| s.take());
    let (update, bytes) = taken.ok_or_else(|| AppError::InvalidInput("没有待安装的更新".into()))?;

    update
        .install(bytes)
        .map_err(|e| AppError::Internal(format!("安装更新失败：{e}")))?;
    app.restart();
}

/// 启动失败时的自救检查：此时没有窗口和前端，不走 [`PendingUpdate`] 与事件，
/// 直接问更新源有没有新版本。
pub async fn rescue_check(app: &AppHandle) -> AppResult<Option<Update>> {
    app.updater()
        .map_err(|e| AppError::Internal(format!("更新器初始化失败：{e}")))?
        .check()
        .await
        .map_err(|e| AppError::Internal(format!("检查更新失败：{e}")))
}

/// 下载并安装自救检查找到的版本；成功后由调用方重启。
pub async fn rescue_install(update: Update) -> AppResult<()> {
    update
        .download_and_install(|_chunk, _total| {}, || {})
        .await
        .map_err(|e| AppError::Internal(format!("下载或安装更新失败：{e}")))
}
