//! 回执对账域命令（发布模块执行计划 4.1 reconcile 域）。

use serde::Serialize;
use specta::Type;
use tauri::{AppHandle, State};
use tauri_specta::Event;

use crate::commands::publish_settings;
use crate::db::repo::{accounts, planning};
use crate::error::{AppError, AppResult};
use crate::publish::events::SheetChangedEvent;
use crate::publish::paths::{self, RelPath};
use crate::publish::platform::Platform;
use crate::publish::reconcile::{self, ReconcileResult, ReportView, SuspectOutcome};
use crate::publish::xlsx::reader;
use crate::state::AppState;

/// 发 SheetChangedEvent + 徽章。
async fn emit_sheet(app: &AppHandle, pool: &sqlx::SqlitePool, sheet_id: i64) {
    if let Ok(Some(s)) = planning::get_sheet(pool, sheet_id).await {
        if let Ok(rows) = planning::list_tasks_by_sheet(pool, sheet_id).await {
            let c = |st: &str| rows.iter().filter(|r| r.status == st).count() as i64;
            let _ = SheetChangedEvent {
                sheet_id,
                date: s.date,
                status: s.status,
                pending: c("pending"),
                published: c("published"),
                failed: c("failed"),
                suspect: c("suspect"),
                canceled: c("canceled"),
            }
            .emit(app);
        }
    }
    crate::publish::inbox::watcher::emit_badges(pool, app).await;
}

/// 读取某任务单对应任务包内的 `任务单.xlsx` 回执。
async fn read_sheet_receipts(
    pool: &sqlx::SqlitePool,
    root: &std::path::Path,
    sheet_id: i64,
) -> AppResult<Vec<reader::ReceiptRow>> {
    let sheet = planning::get_sheet(pool, sheet_id)
        .await?
        .ok_or_else(|| AppError::InvalidInput("任务单不存在".into()))?;
    let yyyymmdd: String = sheet.date.chars().filter(|c| c.is_ascii_digit()).collect();
    let xlsx =
        RelPath::from_parts([paths::TASK_PACKAGES, &yyyymmdd, paths::TASK_XLSX]).to_local(root);
    if !xlsx.exists() {
        return Ok(Vec::new());
    }
    reader::read_receipts(&xlsx)
}

/// 手动导入回执（兜底，与 watcher 走同一对账管线）。
#[tauri::command]
#[specta::specta]
pub async fn import_receipts(
    state: State<'_, AppState>,
    app: AppHandle,
    sheet_id: i64,
) -> AppResult<ReconcileResult> {
    let root = publish_settings::root_local(&state.db).await?;
    let receipts = read_sheet_receipts(&state.db, &root, sheet_id).await?;
    let res = reconcile::apply_receipts(&state.db, sheet_id, &receipts).await?;
    reconcile::maybe_close(&state.db, sheet_id).await?;
    emit_sheet(&app, &state.db, sheet_id).await;
    Ok(res)
}

/// 人工定态疑似已发。
#[tauri::command]
#[specta::specta]
pub async fn resolve_suspect(
    state: State<'_, AppState>,
    app: AppHandle,
    task_id: i64,
    outcome: SuspectOutcome,
) -> AppResult<()> {
    let task = planning::get_task(&state.db, task_id)
        .await?
        .ok_or_else(|| AppError::InvalidInput("任务不存在".into()))?;
    reconcile::resolve_suspect(&state.db, task_id, outcome).await?;
    reconcile::maybe_close(&state.db, task.sheet_id).await?;
    emit_sheet(&app, &state.db, task.sheet_id).await;
    Ok(())
}

/// 全量对账（watcher / ticker 用）：遍历已导出单，读回执入账 + 关单。
pub async fn reconcile_run(pool: &sqlx::SqlitePool, app: &AppHandle) {
    let root = match publish_settings::root_local(pool).await {
        Ok(r) => r,
        Err(_) => return,
    };
    let sheets = match planning::exported_with_pending(pool).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "对账列举任务单失败");
            return;
        }
    };
    // exported_with_pending 只含有 pending 的；已 reconciling 但仍有 pending 也在内。
    for sheet in sheets {
        match read_sheet_receipts(pool, &root, sheet.id).await {
            Ok(receipts) if !receipts.is_empty() => {
                if let Err(e) = reconcile::apply_receipts(pool, sheet.id, &receipts).await {
                    tracing::warn!(error = %e, sheet = sheet.id, "对账失败");
                }
                let _ = reconcile::maybe_close(pool, sheet.id).await;
                emit_sheet(app, pool, sheet.id).await;
            }
            _ => {}
        }
    }
}

// ─────────────────────────────────────────────── 看板

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PlatformStat {
    pub platform: String,
    pub platform_zh: String,
    pub done: i64,
    pub total: i64,
    pub pct: i64,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AccountStat {
    pub id: i64,
    pub platform_zh: String,
    pub name: String,
    pub used: i64,
    pub daily_limit: i64,
    /// normal | disabled | circuit（当日熔断）
    pub health: String,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DashboardView {
    pub date: String,
    pub sheet_id: Option<i64>,
    pub plan: i64,
    pub published: i64,
    pub failed: i64,
    pub suspect: i64,
    pub pending: i64,
    pub platforms: Vec<PlatformStat>,
    pub accounts: Vec<AccountStat>,
    pub has_report: bool,
}

/// 今日看板：计划/已发布/失败/待核对 + 平台完成率 + 账号健康。
#[tauri::command]
#[specta::specta]
pub async fn get_dashboard(state: State<'_, AppState>, date: String) -> AppResult<DashboardView> {
    let sheet = planning::get_sheet_by_date(&state.db, &date).await?;
    let rows = match &sheet {
        Some(s) => planning::sheet_rows(&state.db, s.id).await?,
        None => Vec::new(),
    };
    let count = |st: &str| rows.iter().filter(|r| r.status == st).count() as i64;

    // 平台完成率。
    let mut platforms: Vec<PlatformStat> = Vec::new();
    for p in Platform::ALL {
        let code = p.code();
        let total = rows.iter().filter(|r| r.platform == code).count() as i64;
        if total == 0 {
            continue;
        }
        let done = rows
            .iter()
            .filter(|r| r.platform == code && r.status == "published")
            .count() as i64;
        platforms.push(PlatformStat {
            platform: code.to_string(),
            platform_zh: p.zh().to_string(),
            done,
            total,
            pct: if total > 0 { done * 100 / total } else { 0 },
        });
    }

    // 账号健康。
    let accts = accounts::list(&state.db).await?;
    let account_stats = accts
        .into_iter()
        .map(|a| {
            let used = rows
                .iter()
                .filter(|r| r.account_id == a.id && r.status == "published")
                .count() as i64;
            let has_risk_cancel = rows
                .iter()
                .any(|r| r.account_id == a.id && r.status == "canceled");
            let health = if a.status == "disabled" {
                "disabled"
            } else if has_risk_cancel {
                "circuit"
            } else {
                "normal"
            };
            AccountStat {
                id: a.id,
                platform_zh: Platform::from_code(&a.platform)
                    .map(|p| p.zh().to_string())
                    .unwrap_or(a.platform.clone()),
                name: a.name,
                used,
                daily_limit: a.daily_limit,
                health: health.to_string(),
            }
        })
        .collect();

    Ok(DashboardView {
        date,
        sheet_id: sheet.as_ref().map(|s| s.id),
        plan: rows.len() as i64,
        published: count("published"),
        failed: count("failed"),
        suspect: count("suspect"),
        pending: count("pending"),
        platforms,
        accounts: account_stats,
        has_report: sheet.and_then(|s| s.report_json).is_some(),
    })
}

/// 读日报（关单时写入 report_json）。
#[tauri::command]
#[specta::specta]
pub async fn get_report(
    state: State<'_, AppState>,
    sheet_id: i64,
) -> AppResult<Option<ReportView>> {
    let sheet = planning::get_sheet(&state.db, sheet_id)
        .await?
        .ok_or_else(|| AppError::InvalidInput("任务单不存在".into()))?;
    Ok(sheet
        .report_json
        .and_then(|j| serde_json::from_str::<ReportView>(&j).ok()))
}
