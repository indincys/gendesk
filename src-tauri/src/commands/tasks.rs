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
    pub prompt_title: Option<String>,
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
        COALESCE(p.code, '') AS prompt_code, p.title AS prompt_title,
        COALESCE(g.name, '') AS group_name,
        t.api_key_id, k.name AS key_alias, t.error_type, t.error_message,
        t.retry_count, t.result_thumb_path, t.prompt_text_snapshot
    FROM tasks t
    LEFT JOIN ref_images r ON r.id = t.ref_image_id
    LEFT JOIN prompts p ON p.id = t.prompt_id
    LEFT JOIN prompt_groups g ON g.id = p.group_id
    LEFT JOIN api_keys k ON k.id = t.api_key_id";

/// 列出任务，可按 5 视觉组筛选：all/pending/running/failed/review/done。
///
/// `batch_id = None` = **全部批次**，这是现在的常态：批次不再是可切换的对象
/// （v0.21.0），任务队列答的是「现在还有哪些活」而不是「第 N 批做到哪了」。
/// 批次内保持生成序，批次之间新的在前——与验收页、作品库同一排序。
#[tauri::command]
#[specta::specta]
pub async fn list_tasks(
    state: State<'_, AppState>,
    batch_id: Option<i64>,
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
    let batch_cond = if batch_id.is_some() {
        "t.batch_id = ? AND "
    } else {
        ""
    };
    let sql = format!(
        "{TASK_SELECT} WHERE {batch_cond}t.status IN ({placeholders})
         ORDER BY t.batch_id DESC, t.id ASC LIMIT ? OFFSET ?"
    );
    let mut q = sqlx::query_as::<_, TaskView>(&sql);
    if let Some(b) = batch_id {
        q = q.bind(b);
    }
    for s in statuses {
        q = q.bind(*s);
    }
    let rows = q.bind(limit).bind(offset).fetch_all(&state.db).await?;
    Ok(rows)
}

/// 批量操作回执：做成了几个、跳过了几个。
///
/// **跳过必须报出来**，不能只报成功数：中止/删除会静默放过在途任务，
/// 而「我选了 30 个，怎么只没了 22 个」如果没人解释，下一步就是再点一次。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct BulkTaskResult {
    pub affected: i64,
    pub skipped: i64,
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

/// 重试全部失败任务（E06：默认排除违规类 ContentPolicy——原样重试必再违规，
/// 应走「改词重试」E34 单独处理）。跨全部批次，不再按批次划范围。
#[tauri::command]
#[specta::specta]
pub async fn retry_failed_tasks(state: State<'_, AppState>) -> AppResult<i64> {
    let ids: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM tasks WHERE status = 'fail' \
         AND (error_type IS NULL OR error_type != 'ContentPolicy')",
    )
    .fetch_all(&state.db)
    .await?;
    requeue_and_revive(&state, &ids).await?;
    Ok(ids.len() as i64)
}

/// 回队 + 把它们所属的批次从 archived 拉回 running。
///
/// 归档是「这批没活了」的标记，回队等于又有活了。不改回来的话，退休扫描
/// 会看见一个 archived 却还有 q 态任务的批次——虽然退休条件本身挡得住，
/// 但那个状态本身就是在说谎。
async fn requeue_and_revive(state: &State<'_, AppState>, ids: &[i64]) -> AppResult<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let done = repo::requeue_many(&state.db, ids).await?;
    if done.is_empty() {
        return Ok(());
    }
    let ph = done.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!(
        "UPDATE batches SET status = 'running', archived_at = NULL
         WHERE id IN (SELECT DISTINCT batch_id FROM tasks WHERE id IN ({ph}))"
    );
    let mut q = sqlx::query(&sql);
    for id in &done {
        q = q.bind(id);
    }
    q.execute(&state.db).await?;
    state.engine.kick();
    Ok(())
}

/// 批量重试所选（任务队列的「重试所选」）。生成中/排队中的任务不参与，计入 skipped。
#[tauri::command]
#[specta::specta]
pub async fn retry_tasks(state: State<'_, AppState>, ids: Vec<i64>) -> AppResult<BulkTaskResult> {
    let total = ids.len() as i64;
    let done = repo::requeue_many(&state.db, &ids).await?;
    requeue_and_revive(&state, &done).await?;
    Ok(BulkTaskResult {
        affected: done.len() as i64,
        skipped: total - done.len() as i64,
    })
}

/// 批量删除所选。生成中/重试中的任务拒绝删除（与在途 worker 抢同一行会让那份图
/// 谁也找不到），计入 skipped。
#[tauri::command]
#[specta::specta]
pub async fn delete_tasks(state: State<'_, AppState>, ids: Vec<i64>) -> AppResult<BulkTaskResult> {
    const DELETABLE: &[&str] = &["q", "rev", "pass", "rej", "fail"];
    let total = ids.len() as i64;
    let (n, batches) = repo::delete_tasks_where(&state.db, &ids, DELETABLE).await?;
    settle(&state, &batches).await;
    Ok(BulkTaskResult {
        affected: n,
        skipped: total - n,
    })
}

/// 批量中止所选：只掐掉**还没开跑**的排队任务。
///
/// 已经发出去的请求中止不了——钱在请求发出的那一刻就花了，硬把行删掉只会让结果
/// 回来时无处可写。故在途任务一律计入 skipped 并如实说明，而不是假装中止成功。
#[tauri::command]
#[specta::specta]
pub async fn cancel_tasks(state: State<'_, AppState>, ids: Vec<i64>) -> AppResult<BulkTaskResult> {
    let total = ids.len() as i64;
    let (n, batches) = repo::delete_tasks_where(&state.db, &ids, &["q"]).await?;
    settle(&state, &batches).await;
    Ok(BulkTaskResult {
        affected: n,
        skipped: total - n,
    })
}

/// 任务被删/被中止之后的收尾：重估归档 → 补发汇总 → 跑一遍退休扫描。
/// 顺序不能反，退休会把批次删掉，而 emit_summary 要对着还在的批次算数。
async fn settle(state: &State<'_, AppState>, batches: &[i64]) {
    for b in batches {
        let _ = repo::archive_if_all_terminal(&state.db, *b).await;
        state.engine.emit_summary(*b).await;
    }
    if !batches.is_empty() {
        crate::commands::batches::retire_batches_quietly(&state.db).await;
    }
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

/// 删除单个任务（「不需要了」）。生成中/重试中的任务不允许删除，避免与在途 worker 竞争；
/// 删除会级联清除 task_attempts（外键 ON DELETE CASCADE）。删除后重估批次归档并补发汇总。
#[tauri::command]
#[specta::specta]
pub async fn delete_task(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    let Some(batch_id) = repo::delete_task(&state.db, id).await? else {
        return Err(AppError::InvalidInput(
            "任务不存在或正在生成中，无法删除".into(),
        ));
    };
    settle(&state, &[batch_id]).await;
    Ok(())
}

/// 删除全部失败任务（批量「不需要了」）。跨全部批次。返回删除数。
#[tauri::command]
#[specta::specta]
pub async fn delete_failed_tasks(state: State<'_, AppState>) -> AppResult<i64> {
    let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM tasks WHERE status = 'fail'")
        .fetch_all(&state.db)
        .await?;
    let (n, batches) = repo::delete_tasks_where(&state.db, &ids, &["fail"]).await?;
    settle(&state, &batches).await;
    Ok(n)
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
