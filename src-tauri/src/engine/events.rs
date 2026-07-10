//! 引擎事件（执行计划 2.2 事件表）。
//!
//! 为让引擎脱离 Tauri 可测试，定义 [`EventSink`] 抽象：生产用 [`TauriSink`] 经
//! tauri-specta 事件推送前端；测试用收集型 sink 断言。

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri_specta::Event;

use super::classify::ErrorType;
use super::progress::Phase;
use super::status::TaskStatus;

/// `task://status-changed`
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct TaskStatusChanged {
    pub task_id: i64,
    pub batch_id: i64,
    pub status: TaskStatus,
    pub error_type: Option<ErrorType>,
    pub error_message: Option<String>,
    pub retry_count: i64,
    pub api_key_id: Option<i64>,
}

/// `task://progress`（250ms 节流）
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct TaskProgress {
    pub task_id: i64,
    pub pct: u8,
    pub phase: Phase,
}

/// 5 视觉组计数。
#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SummaryCounts {
    /// 待处理 q
    pub pending: i64,
    /// 生成中 run+retry
    pub running: i64,
    /// 异常 fail
    pub failed: i64,
    /// 待验收 rev
    pub review: i64,
    /// 已通过 pass
    pub passed: i64,
    /// 未通过 rej
    pub rejected: i64,
    pub total: i64,
}

/// `batch://summary`（250ms 节流）
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct BatchSummary {
    pub batch_id: i64,
    pub counts: SummaryCounts,
    pub active_concurrency: i64,
    pub paused: bool,
}

/// Key 健康状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum KeyState {
    Ok,
    Limited,
    AuthFailed,
    Disabled,
}

/// `keys://health`
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct KeyHealth {
    pub key_id: i64,
    pub state: KeyState,
    pub used_concurrency: i64,
    pub success_rate: f64,
}

/// 引擎事件汇。engine 只依赖此 trait，不依赖 Tauri。
pub trait EventSink: Send + Sync + 'static {
    fn status_changed(&self, e: TaskStatusChanged);
    fn progress(&self, e: TaskProgress);
    fn batch_summary(&self, e: BatchSummary);
    fn key_health(&self, e: KeyHealth);
    /// 系统通知（E18 熔断 / E04 批次完成等无人值守事件）。默认无操作。
    fn notify(&self, _title: String, _body: String) {}
}

/// 生产实现：经 tauri-specta 事件推送前端。
pub struct TauriSink {
    app: tauri::AppHandle,
}

impl TauriSink {
    pub fn new(app: tauri::AppHandle) -> Self {
        Self { app }
    }
}

impl EventSink for TauriSink {
    fn status_changed(&self, e: TaskStatusChanged) {
        let _ = e.emit(&self.app);
    }
    fn progress(&self, e: TaskProgress) {
        let _ = e.emit(&self.app);
    }
    fn batch_summary(&self, e: BatchSummary) {
        let _ = e.emit(&self.app);
    }
    fn key_health(&self, e: KeyHealth) {
        let _ = e.emit(&self.app);
    }
    fn notify(&self, title: String, body: String) {
        use tauri_plugin_notification::NotificationExt;
        let _ = self
            .app
            .notification()
            .builder()
            .title(title)
            .body(body)
            .show();
    }
}

/// 无操作 sink（命令行/无 UI 场景）。
pub struct NullSink;
impl EventSink for NullSink {
    fn status_changed(&self, _e: TaskStatusChanged) {}
    fn progress(&self, _e: TaskProgress) {}
    fn batch_summary(&self, _e: BatchSummary) {}
    fn key_health(&self, _e: KeyHealth) {}
}

/// 便捷别名。
pub type SharedSink = Arc<dyn EventSink>;

#[cfg(test)]
pub mod test_sink {
    use std::sync::{Arc, Mutex};

    use super::*;

    /// 收集型 sink，测试断言事件。
    #[derive(Default)]
    pub struct CollectingSink {
        pub statuses: Mutex<Vec<TaskStatusChanged>>,
        pub summaries: Mutex<Vec<BatchSummary>>,
        pub progresses: Mutex<Vec<TaskProgress>>,
    }

    impl CollectingSink {
        pub fn shared() -> Arc<Self> {
            Arc::new(Self::default())
        }
    }

    impl EventSink for CollectingSink {
        fn status_changed(&self, e: TaskStatusChanged) {
            if let Ok(mut v) = self.statuses.lock() {
                v.push(e);
            }
        }
        fn progress(&self, e: TaskProgress) {
            if let Ok(mut v) = self.progresses.lock() {
                v.push(e);
            }
        }
        fn batch_summary(&self, e: BatchSummary) {
            if let Ok(mut v) = self.summaries.lock() {
                v.push(e);
            }
        }
        fn key_health(&self, _e: KeyHealth) {}
    }
}
