//! tasks 域命令（列表/详情/重试）。

use serde::Serialize;
use specta::Type;
use tauri::State;

use crate::db::repo::tasks as repo;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TaskView {
    pub id: i64,
    pub batch_id: i64,
    pub status: String,
    pub ref_image_id: i64,
    pub prompt_id: i64,
    pub api_key_id: Option<i64>,
    pub error_type: Option<String>,
    pub error_message: Option<String>,
    pub retry_count: i64,
    pub result_thumb_path: Option<String>,
    pub prompt_text_snapshot: String,
}

impl From<repo::TaskRow> for TaskView {
    fn from(r: repo::TaskRow) -> Self {
        Self {
            id: r.id,
            batch_id: r.batch_id,
            status: r.status,
            ref_image_id: r.ref_image_id,
            prompt_id: r.prompt_id,
            api_key_id: r.api_key_id,
            error_type: r.error_type,
            error_message: r.error_message,
            retry_count: r.retry_count,
            result_thumb_path: r.result_thumb_path,
            prompt_text_snapshot: r.prompt_text_snapshot,
        }
    }
}

/// 列出某批次任务，可按 5 视觉组筛选：all/pending/running/failed/review/done。
#[tauri::command]
#[specta::specta]
pub async fn list_tasks(
    state: State<'_, AppState>,
    batch_id: i64,
    status_group: Option<String>,
    page: Option<i64>,
) -> AppResult<Vec<TaskView>> {
    let statuses: &[&str] = match status_group.as_deref() {
        Some("pending") => &["q"],
        Some("running") => &["run", "retry"],
        Some("failed") => &["fail"],
        Some("review") => &["rev"],
        Some("done") => &["pass", "rej"],
        _ => &["q", "run", "retry", "rev", "pass", "rej", "fail"],
    };
    let placeholders = statuses.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let limit = 500i64;
    let offset = page.unwrap_or(0).max(0) * limit;
    let sql = format!(
        "SELECT * FROM tasks WHERE batch_id = ? AND status IN ({placeholders})
         ORDER BY id ASC LIMIT ? OFFSET ?"
    );
    let mut q = sqlx::query_as::<_, repo::TaskRow>(&sql).bind(batch_id);
    for s in statuses {
        q = q.bind(*s);
    }
    let rows = q.bind(limit).bind(offset).fetch_all(&state.db).await?;
    Ok(rows.into_iter().map(TaskView::from).collect())
}

#[tauri::command]
#[specta::specta]
pub async fn get_task(state: State<'_, AppState>, id: i64) -> AppResult<TaskView> {
    let row = repo::get_task(&state.db, id)
        .await?
        .ok_or_else(|| AppError::InvalidInput("任务不存在".into()))?;
    Ok(TaskView::from(row))
}

/// 手动重试单个失败任务（可携带微调提示词写入快照，R8）。
#[tauri::command]
#[specta::specta]
pub async fn retry_task(
    state: State<'_, AppState>,
    id: i64,
    edited_prompt: Option<String>,
) -> AppResult<()> {
    if let Some(text) = edited_prompt {
        sqlx::query("UPDATE tasks SET prompt_text_snapshot = ?2 WHERE id = ?1")
            .bind(id)
            .bind(text)
            .execute(&state.db)
            .await?;
    }
    repo::requeue(&state.db, id).await?;
    state.engine.kick();
    Ok(())
}

/// 重试某批次全部失败任务。
#[tauri::command]
#[specta::specta]
pub async fn retry_failed_tasks(state: State<'_, AppState>, batch_id: i64) -> AppResult<i64> {
    let ids: Vec<i64> =
        sqlx::query_scalar("SELECT id FROM tasks WHERE batch_id = ?1 AND status = 'fail'")
            .bind(batch_id)
            .fetch_all(&state.db)
            .await?;
    for id in &ids {
        repo::requeue(&state.db, *id).await?;
    }
    // 批次可能已归档 → 恢复 running。
    if !ids.is_empty() {
        repo::set_batch_status(&state.db, batch_id, "running").await?;
    }
    state.engine.kick();
    Ok(ids.len() as i64)
}

/// 重试全部因中断而失败的任务（error_type=Interrupted）。
#[tauri::command]
#[specta::specta]
pub async fn retry_interrupted_tasks(state: State<'_, AppState>) -> AppResult<i64> {
    let ids: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM tasks WHERE status = 'fail' AND error_type = 'Interrupted'",
    )
    .fetch_all(&state.db)
    .await?;
    for id in &ids {
        repo::requeue(&state.db, *id).await?;
    }
    state.engine.kick();
    Ok(ids.len() as i64)
}

/// 统计当前中断任务数（前端 banner 用）。
#[tauri::command]
#[specta::specta]
pub async fn count_interrupted(state: State<'_, AppState>) -> AppResult<i64> {
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM tasks WHERE status = 'fail' AND error_type = 'Interrupted'",
    )
    .fetch_one(&state.db)
    .await?;
    Ok(n)
}
