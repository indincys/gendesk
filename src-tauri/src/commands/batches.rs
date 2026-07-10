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
    /// 批次备注名（E10）；None = 未命名。
    pub note: Option<String>,
    /// 首张产出缩略图（E10 批次切换器预览）。
    pub first_thumb_path: Option<String>,
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
    let (batch_id, count) = engine::create_batch(
        &state.db,
        &output_dir,
        &input.params_json,
        &mappings,
        input.draws,
    )
    .await?;

    // 唤醒调度器开跑。
    state.engine.kick();

    Ok(BatchView {
        id: batch_id,
        created_at: crate::db::now_unix(),
        status: "running".into(),
        task_count: count,
        params_json: input.params_json.clone(),
        note: None,
        first_thumb_path: None,
    })
}

/// 批次备注命名（E10）。空串清除备注。
#[tauri::command]
#[specta::specta]
pub async fn rename_batch(
    state: State<'_, AppState>,
    batch_id: i64,
    note: String,
) -> AppResult<()> {
    repo::rename_batch(&state.db, batch_id, &note).await?;
    Ok(())
}

/// 批次配置快照（E07「按此配置再来一批」）：还原生成页挂靠与参数。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct BatchConfig {
    /// 参考图 → 提示词组挂靠（仅保留当前仍存在的参考图与分组）。
    pub refs: Vec<RefMappingInput2>,
    pub params_json: String,
}

/// 挂靠输出项（与 RefMappingInput 同形，但用于序列化返回）。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RefMappingInput2 {
    pub ref_image_id: i64,
    pub prompt_group_id: i64,
}

/// 读取某批次的挂靠与参数快照（E07 再来一批）。只返回未删除的参考图与仍存在的分组，
/// 保证还原到生成页后可直接创建新批次。
#[tauri::command]
#[specta::specta]
pub async fn get_batch_config(state: State<'_, AppState>, batch_id: i64) -> AppResult<BatchConfig> {
    let params_json: Option<String> =
        sqlx::query_scalar("SELECT params_json FROM batches WHERE id = ?1")
            .bind(batch_id)
            .fetch_optional(&state.db)
            .await?;
    let Some(params_json) = params_json else {
        return Err(AppError::InvalidInput("批次不存在".into()));
    };
    let refs: Vec<RefMappingInput2> = sqlx::query_as::<_, (i64, i64)>(
        "SELECT br.ref_image_id, br.prompt_group_id FROM batch_refs br
         JOIN ref_images ri ON ri.id = br.ref_image_id AND ri.deleted_at IS NULL
         JOIN prompt_groups pg ON pg.id = br.prompt_group_id
         WHERE br.batch_id = ?1",
    )
    .bind(batch_id)
    .fetch_all(&state.db)
    .await?
    .into_iter()
    .map(|(ref_image_id, prompt_group_id)| RefMappingInput2 {
        ref_image_id,
        prompt_group_id,
    })
    .collect();
    Ok(BatchConfig { refs, params_json })
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
        let first_thumb_path = repo::batch_first_thumb(&state.db, b.id).await?;
        out.push(BatchView {
            id: b.id,
            created_at: b.created_at,
            status: b.status,
            task_count: counts.total,
            params_json: b.params_json,
            note: b.note,
            first_thumb_path,
        });
    }
    Ok(out)
}

/// 取消批次剩余排队任务（E03）：删除该批次全部 'q' 态任务，重估归档并补发汇总。
/// 在途（run/retry）任务不受影响，会自行跑完。返回取消数。
#[tauri::command]
#[specta::specta]
pub async fn cancel_batch_pending(state: State<'_, AppState>, batch_id: i64) -> AppResult<i64> {
    let n = repo::cancel_pending(&state.db, batch_id).await?;
    if n > 0 {
        // 剩余若全为终态则归档；补发汇总驱动前端进度/徽章即时更新。
        let _ = repo::archive_if_all_terminal(&state.db, batch_id).await;
        state.engine.emit_summary(batch_id).await;
    }
    Ok(n)
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
