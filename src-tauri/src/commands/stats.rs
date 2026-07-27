//! stats 域命令：生产总览（E25 生成页顶部条）。
//!
//! 「分组/提示词效果统计」随提示词库页一并去掉（v0.21.0）：那套合格率是**长期资产**的
//! 口径——它要回答「这个分组历来好不好用」，而提示词现在跑完一次就没了，
//! 一个只有一次样本的合格率不构成任何判断依据。

use serde::Serialize;
use specta::Type;
use tauri::State;

use crate::error::AppResult;
use crate::state::AppState;

/// 生产总览（E25 生成页顶部条）：今日生成/通过/请求。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProductionOverview {
    /// 今日成功产出的图片数（task_attempts.outcome='success'）。
    pub generated_today: i64,
    /// 今日验收通过的作品数。
    pub accepted_today: i64,
    /// 今日请求次数（含重试，全部 task_attempts）。
    pub requests_today: i64,
}

#[tauri::command]
#[specta::specta]
pub async fn production_overview(state: State<'_, AppState>) -> AppResult<ProductionOverview> {
    let now = crate::db::now_unix();
    let day_start = now - now.rem_euclid(86_400); // UTC 当日 0 点（与命名器同口径）。

    let requests_today: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM task_attempts WHERE started_at >= ?1")
            .bind(day_start)
            .fetch_one(&state.db)
            .await?;
    let generated_today: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM task_attempts WHERE started_at >= ?1 AND outcome = 'success'",
    )
    .bind(day_start)
    .fetch_one(&state.db)
    .await?;
    let accepted_today: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM accepted_works WHERE accepted_at >= ?1")
            .bind(day_start)
            .fetch_one(&state.db)
            .await?;
    Ok(ProductionOverview {
        generated_today,
        accepted_today,
        requests_today,
    })
}
