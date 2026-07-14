//! 收件箱域命令（发布模块执行计划 4.1 inbox 域）。

use serde::Serialize;
use specta::Type;
use tauri::State;

use crate::commands::publish_settings;
use crate::db::repo::inbox as repo;
use crate::error::{AppError, AppResult};
use crate::publish::inbox::ingest::{self, IngestOutcome};
use crate::publish::paths::RelPath;
use crate::state::AppState;

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct InboxItemView {
    pub id: i64,
    pub file_rel: String,
    /// 文件名（末段，UI 展示）。
    pub file_name: String,
    pub kind: Option<String>,
    pub sku_code: Option<String>,
    pub state: String,
    /// 收录报告 / 失败原因摘要。
    pub detail: Option<String>,
    pub created_at: i64,
}

fn detail_summary(detail_json: Option<&str>) -> Option<String> {
    let json = detail_json?;
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    match v.get("state").and_then(|s| s.as_str()) {
        Some("failed") => v.get("reason").and_then(|r| r.as_str()).map(str::to_string),
        Some("ingested") => {
            let titles = v.get("titles").and_then(|x| x.as_i64()).unwrap_or(0);
            let bodies = v.get("bodies").and_then(|x| x.as_i64()).unwrap_or(0);
            let mut s = format!("入库 标题×{titles} 正文×{bodies}");
            if let Some(diff) = v.get("topicDiff").and_then(|d| d.as_str()) {
                s.push_str(&format!("；{diff}"));
            }
            Some(s)
        }
        Some("unclaimed") => Some("未识别到已知 SKU".to_string()),
        _ => None,
    }
}

fn to_view(r: repo::InboxItemRow) -> InboxItemView {
    let file_name = r
        .file_rel
        .rsplit('/')
        .next()
        .unwrap_or(&r.file_rel)
        .to_string();
    let detail = detail_summary(r.detail_json.as_deref());
    InboxItemView {
        id: r.id,
        file_rel: r.file_rel,
        file_name,
        kind: r.kind,
        sku_code: r.sku_code,
        state: r.state,
        detail,
        created_at: r.created_at,
    }
}

/// 列出收件箱记录；state 为空则全部。
#[tauri::command]
#[specta::specta]
pub async fn list_inbox_items(
    state: State<'_, AppState>,
    filter_state: Option<String>,
) -> AppResult<Vec<InboxItemView>> {
    let rows = repo::list(&state.db, filter_state.as_deref()).await?;
    Ok(rows.into_iter().map(to_view).collect())
}

/// 认领：人工指认 SKU 后走正常收录管线。
#[tauri::command]
#[specta::specta]
pub async fn claim_inbox_item(
    state: State<'_, AppState>,
    id: i64,
    sku_code: String,
) -> AppResult<IngestOutcome> {
    let root = publish_settings::root_local(&state.db).await?;
    let item = repo::get(&state.db, id)
        .await?
        .ok_or_else(|| AppError::InvalidInput("收件箱记录不存在".into()))?;
    let rel = RelPath::new(&item.file_rel);
    let outcome = ingest::ingest_txt(&state.db, &root, &rel, Some(&sku_code)).await?;
    Ok(outcome)
}

/// 丢弃待认领/失败记录（删记录，文件留原位不动）。
#[tauri::command]
#[specta::specta]
pub async fn discard_inbox_item(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    repo::delete(&state.db, id).await?;
    Ok(())
}

/// 解析失败重试：对原文件重跑收录管线。
#[tauri::command]
#[specta::specta]
pub async fn retry_inbox_item(state: State<'_, AppState>, id: i64) -> AppResult<IngestOutcome> {
    let root = publish_settings::root_local(&state.db).await?;
    let item = repo::get(&state.db, id)
        .await?
        .ok_or_else(|| AppError::InvalidInput("收件箱记录不存在".into()))?;
    let rel = RelPath::new(&item.file_rel);
    let outcome = ingest::ingest_txt(&state.db, &root, &rel, None).await?;
    Ok(outcome)
}

/// 手动全量扫描收件箱。返回本次收录/待认领/失败计数。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RescanResult {
    pub ingested: i64,
    pub unclaimed: i64,
    pub failed: i64,
}

#[tauri::command]
#[specta::specta]
pub async fn rescan_inbox(state: State<'_, AppState>) -> AppResult<RescanResult> {
    let root = publish_settings::root_local(&state.db).await?;
    let outcomes = ingest::rescan(&state.db, &root).await?;
    let mut r = RescanResult {
        ingested: 0,
        unclaimed: 0,
        failed: 0,
    };
    for o in &outcomes {
        match o.state_code() {
            "ingested" => r.ingested += 1,
            "unclaimed" => r.unclaimed += 1,
            "failed" => r.failed += 1,
            _ => {}
        }
    }
    Ok(r)
}
