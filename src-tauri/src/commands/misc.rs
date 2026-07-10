//! 杂项命令 —— 前端错误转发等（执行计划 0.7 / 2.1 updater/misc 域）。

use serde::Deserialize;
use specta::Type;

use crate::error::AppResult;

/// 前端未捕获错误载荷。经 ErrorBoundary / window.onerror 采集后转发到此。
#[derive(Debug, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct FrontendErrorPayload {
    /// 错误消息。
    pub message: String,
    /// 调用栈（可选）。
    pub stack: Option<String>,
    /// 来源标识（组件 / 路由）。
    pub source: Option<String>,
    /// 关联任务 ID（若发生在任务上下文），用于全链路贯穿。
    pub task_id: Option<String>,
}

/// 返回应用当前版本（取自 tauri.conf 的 package 版本，与 updater 比对同源，随发布 tag 同步）。
/// 前端页脚/设置页展示真实版本，避免硬编码漂移；「是否有新版」由 updater 域另行驱动。
#[tauri::command]
#[specta::specta]
pub fn app_version(app: tauri::AppHandle) -> AppResult<String> {
    Ok(app.package_info().version.to_string())
}

/// 将前端错误写入统一 tracing 日志流（AI 修 bug 的输入就是这份日志）。
#[tauri::command]
#[specta::specta]
pub fn log_frontend_error(payload: FrontendErrorPayload) -> AppResult<()> {
    tracing::error!(
        target: "frontend",
        source = payload.source.as_deref().unwrap_or("unknown"),
        task_id = payload.task_id.as_deref().unwrap_or(""),
        stack = payload.stack.as_deref().unwrap_or(""),
        "frontend error: {}",
        payload.message
    );
    Ok(())
}
