//! tasks 域命令（列表/详情/重试）。

use serde::Serialize;
use specta::Type;
use tauri::State;

use crate::db::repo::tasks as repo;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// 任务视图（含参考图名/提示词编号/分组名/Key 别名，供任务表直接渲染）。
#[derive(Debug, Clone, Serialize, Type, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct TaskView {
    pub id: i64,
    pub batch_id: i64,
    pub status: String,
    pub ref_image_id: i64,
    pub ref_name: String,
    pub prompt_id: i64,
    pub prompt_code: String,
    pub group_name: String,
    pub api_key_id: Option<i64>,
    pub key_alias: Option<String>,
    pub error_type: Option<String>,
    pub error_message: Option<String>,
    pub retry_count: i64,
    pub result_thumb_path: Option<String>,
    pub prompt_text_snapshot: String,
}

const TASK_SELECT: &str = "SELECT t.id, t.batch_id, t.status, t.ref_image_id,
        COALESCE(r.name, '') AS ref_name, t.prompt_id,
        COALESCE(p.code, '') AS prompt_code, COALESCE(g.name, '') AS group_name,
        t.api_key_id, k.name AS key_alias, t.error_type, t.error_message,
        t.retry_count, t.result_thumb_path, t.prompt_text_snapshot
    FROM tasks t
    LEFT JOIN ref_images r ON r.id = t.ref_image_id
    LEFT JOIN prompts p ON p.id = t.prompt_id
    LEFT JOIN prompt_groups g ON g.id = p.group_id
    LEFT JOIN api_keys k ON k.id = t.api_key_id";

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
        "{TASK_SELECT} WHERE t.batch_id = ? AND t.status IN ({placeholders})
         ORDER BY t.id ASC LIMIT ? OFFSET ?"
    );
    let mut q = sqlx::query_as::<_, TaskView>(&sql).bind(batch_id);
    for s in statuses {
        q = q.bind(*s);
    }
    let rows = q.bind(limit).bind(offset).fetch_all(&state.db).await?;
    Ok(rows)
}

#[tauri::command]
#[specta::specta]
pub async fn get_task(state: State<'_, AppState>, id: i64) -> AppResult<TaskView> {
    let sql = format!("{TASK_SELECT} WHERE t.id = ?");
    sqlx::query_as::<_, TaskView>(&sql)
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::InvalidInput("任务不存在".into()))
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
