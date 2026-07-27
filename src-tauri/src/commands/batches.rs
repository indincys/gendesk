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

/// 建批回执。**批次不再是一个可管理的对象**（v0.21.0）：它没有列表、没有切换器、
/// 没有重命名，也不能「按此配置再来一批」——跑完就退出历史（`retire_resolved_batches`）。
/// 剩下的只是「这一次点下去产生了什么」，故这个结构只回答那一句。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct BatchView {
    pub id: i64,
    pub created_at: i64,
    pub status: String,
    pub task_count: i64,
    /// 批次生效的生成参数快照（E16 / D1）。
    pub params_json: String,
}

/// 跑一遍「已了结的批次退出历史」。
///
/// 只记日志、不打断调用方：它是每次验收/删除/清废纸篓之后**顺手**做的收尾，
/// 失败了下一次还会再扫（条件是幂等的），不该让一次验收因此报错。
pub async fn retire_batches_quietly(pool: &sqlx::SqlitePool) {
    match repo::retire_resolved_batches(pool).await {
        Ok(r) if !r.is_empty() => tracing::info!(
            batches = r.batches,
            prompts = r.prompts,
            groups = r.groups,
            "批次已了结，退出历史（提示词随之消耗）"
        ),
        Ok(_) => {}
        Err(e) => tracing::warn!(error = %e, "批次退休扫描失败"),
    }
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
    // 花钱之前的本地预检：受控取值/尺寸边长/压缩区间。端点的拒绝发生在计费之后，
    // 一批 20 个任务会连报 20 次同一个错。严格解析同时挡住「键类型不对 → 整份参数
    // 静默退化成空 → 选了 9:16 却一个字段都没发出去」。
    crate::provider::GenParams::parse_checked(&input.params_json)
        .map_err(AppError::InvalidInput)?;
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

/// 在系统文件管理器打开输出根目录 `outputs/`（验收通过的图按 `{批次}/{分组}/` 落在里面）。
///
/// 取代了原来那个「打开本批输出目录」——批次已经不是可点的对象，而人还是要能拿到文件。
#[tauri::command]
#[specta::specta]
pub async fn open_outputs_dir(state: State<'_, AppState>, app: tauri::AppHandle) -> AppResult<()> {
    use tauri_plugin_opener::OpenerExt;
    let dir = state.dirs.outputs();
    std::fs::create_dir_all(&dir).map_err(|e| AppError::Io(e.to_string()))?;
    app.opener()
        .open_path(dir.to_string_lossy().to_string(), None::<&str>)
        .map_err(|e| AppError::Io(e.to_string()))
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
