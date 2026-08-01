//! 发布模块事件（发布模块执行计划 4.2）。沿用 engine/events.rs 的 Event derive 模式。

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri_specta::Event;

/// `publish://badges`：资产库待认领、发布计划待确认与待核对。
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct PublishBadgesEvent {
    pub unclaimed: i64,
    pub pending_sheets: i64,
    pub pending_reconcile: i64,
}

/// `publish://inbox-ingest`：单文件收录结果（前端 toast + 列表刷新）。
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct InboxIngestEvent {
    pub file_name: String,
    pub state: String,
    pub product_code: Option<String>,
    pub titles: i64,
    pub bodies: i64,
    pub message: String,
}

/// `publish://export-progress`：任务包导出进度（复制视频可达数百 MB，UI 需要交代）。
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct ExportProgressEvent {
    pub sheet_id: i64,
    pub done: i64,
    pub total: i64,
}

/// `publish://sheet-changed`：任务单状态/行状态变化（P2/P3 工作台与看板刷新）。
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct SheetChangedEvent {
    pub sheet_id: i64,
    pub date: String,
    pub status: String,
    /// 汇总计数（待执行/已发布/失败/疑似/已取消）。
    pub pending: i64,
    pub published: i64,
    pub failed: i64,
    pub suspect: i64,
    pub canceled: i64,
}
