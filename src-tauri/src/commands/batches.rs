//! batches / tasks 域命令（执行计划 2.1 引擎）。

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::State;

use crate::db::repo::tasks as repo;
use crate::engine::{self, RefMapping};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

#[derive(Debug, Clone, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RefMappingInput {
    pub ref_image_id: i64,
    pub prompt_group_id: i64,
}

#[derive(Debug, Clone, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CreateBatchInput {
    pub refs: Vec<RefMappingInput>,
    pub params_json: String,
    /// 抽卡次数 k（E17 / D2）：每个组合独立生成 k 次。默认 1，后端夹取 1..=5。
    pub draws: i64,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct BatchView {
    pub id: i64,
    pub created_at: i64,
    pub status: String,
    pub task_count: i64,
    /// 批次生效的生成参数快照（E16 / D1），任务页可回查。
    pub params_json: String,
}

/// 组合展开创建批次，调度器自动开跑。返回批次视图（含任务总数）。
#[tauri::command]
#[specta::specta]
pub async fn create_batch(
    state: State<'_, AppState>,
    input: CreateBatchInput,
) -> AppResult<BatchView> {
    if input.refs.is_empty() {
        return Err(AppError::InvalidInput("未选择任何参考图挂靠".into()));
    }
    let mappings: Vec<RefMapping> = input
        .refs
        .iter()
        .map(|r| RefMapping {
            ref_image_id: r.ref_image_id,
            prompt_group_id: r.prompt_group_id,
        })
        .collect();

    let output_dir = state.dirs.outputs().to_string_lossy().to_string();
    let (batch_id, count) =
        engine::create_batch(&state.db, &output_dir, &input.params_json, &mappings, input.draws)
            .await?;

    // 唤醒调度器开跑。
    state.engine.kick();

    Ok(BatchView {
        id: batch_id,
        created_at: crate::db::now_unix(),
        status: "running".into(),
        task_count: count,
        params_json: input.params_json.clone(),
    })
}

/// 历史单张生成均值秒数（E31 确认摘要 ETA 估算）；无成功历史返回 None。
#[tauri::command]
#[specta::specta]
pub async fn estimate_task_seconds(state: State<'_, AppState>) -> AppResult<Option<f64>> {
    // 近 50 次成功尝试的平均耗时。
    let avg: Option<f64> = sqlx::query_scalar(
        "SELECT AVG(duration_ms) FROM (
            SELECT duration_ms FROM task_attempts
            WHERE outcome = 'success' AND duration_ms IS NOT NULL
            ORDER BY id DESC LIMIT 50)",
    )
    .fetch_one(&state.db)
    .await?;
    Ok(avg.map(|ms| ms / 1000.0))
}

#[tauri::command]
#[specta::specta]
pub async fn list_batches(state: State<'_, AppState>) -> AppResult<Vec<BatchView>> {
    let rows = repo::list_batches(&state.db).await?;
    let mut out = Vec::with_capacity(rows.len());
    for b in rows {
        let counts = repo::counts_for_batch(&state.db, b.id).await?;
        out.push(BatchView {
            id: b.id,
            created_at: b.created_at,
            status: b.status,
            task_count: counts.total,
            params_json: b.params_json,
        });
    }
    Ok(out)
}

#[tauri::command]
#[specta::specta]
pub async fn pause_queue(state: State<'_, AppState>) -> AppResult<()> {
    state.engine.pause();
    persist_paused(&state.db, true).await
}

#[tauri::command]
#[specta::specta]
pub async fn resume_queue(state: State<'_, AppState>) -> AppResult<()> {
    state.engine.resume();
    persist_paused(&state.db, false).await
}

/// 把暂停态写回 settings（持久化，重启沿用）。
async fn persist_paused(pool: &sqlx::SqlitePool, paused: bool) -> AppResult<()> {
    let current = crate::db::repo::settings::get_raw(pool).await?;
    let mut v: serde_json::Value = current
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    v["paused"] = serde_json::Value::Bool(paused);
    crate::db::repo::settings::set_raw(pool, &v.to_string()).await?;
    Ok(())
}
