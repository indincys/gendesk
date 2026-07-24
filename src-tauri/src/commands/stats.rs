//! stats 域命令（E25：提示词效果统计 + 生产总览）。
//!
//! 合格率口径（决策 D2）：以「组合(参考图 × 提示词)是否至少产出一张通过图」计，避免抽卡稀释。
//! - 分母 combos = 已产出图（任务达 rev/pass/rej）的去重组合数；
//! - 分子 passed = 通过作品覆盖的去重组合数。

use serde::Serialize;
use specta::Type;
use sqlx::SqlitePool;
use tauri::State;

use crate::error::AppResult;
use crate::state::AppState;

/// 单个分组的产出统计（E25 分组卡片合格率）。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct GroupStat {
    pub group_id: i64,
    /// 已产出图的去重组合数（分母）。
    pub combos: i64,
    /// 通过图覆盖的去重组合数（分子）。
    pub passed: i64,
    /// 累计通过作品数。
    pub works: i64,
}

/// 各分组产出统计（E25）。按当前 prompts.group_id 归属（提示词移组后随之变化）。
#[tauri::command]
#[specta::specta]
pub async fn list_group_stats(state: State<'_, AppState>) -> AppResult<Vec<GroupStat>> {
    Ok(group_stats(&state.db).await?)
}

async fn group_stats(pool: &SqlitePool) -> Result<Vec<GroupStat>, sqlx::Error> {
    // 分母：已产出图（rev/pass/rej）的去重 (ref, prompt) 组合，按当前分组归集。
    let denom: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT p.group_id, COUNT(DISTINCT t.ref_image_id || '-' || t.prompt_id)
         FROM tasks t JOIN prompts p ON p.id = t.prompt_id
         WHERE t.status IN ('rev','pass','rej')
         GROUP BY p.group_id",
    )
    .fetch_all(pool)
    .await?;
    // 分子 + 作品数：通过作品覆盖的去重组合，按当前分组归集。
    let numer: Vec<(i64, i64, i64)> = sqlx::query_as(
        "SELECT p.group_id, COUNT(DISTINCT w.ref_image_id || '-' || w.prompt_id), COUNT(*)
         FROM accepted_works w JOIN prompts p ON p.id = w.prompt_id
         GROUP BY p.group_id",
    )
    .fetch_all(pool)
    .await?;

    use std::collections::BTreeMap;
    let mut map: BTreeMap<i64, GroupStat> = BTreeMap::new();
    for (gid, combos) in denom {
        map.entry(gid)
            .or_insert(GroupStat {
                group_id: gid,
                combos: 0,
                passed: 0,
                works: 0,
            })
            .combos = combos;
    }
    for (gid, passed, works) in numer {
        let e = map.entry(gid).or_insert(GroupStat {
            group_id: gid,
            combos: 0,
            passed: 0,
            works: 0,
        });
        e.passed = passed;
        e.works = works;
    }
    Ok(map.into_values().collect())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即失败
mod tests {
    use super::*;
    use crate::db::repo::{prompts, refs, tasks, works};
    use crate::db::test_support::test_pool;

    // E25：合格率按 D2 组合口径聚合，数字与库内记录一致。
    #[tokio::test]
    async fn group_stats_counts_combos_and_passed() {
        let (pool, _d) = test_pool().await;
        let rid = refs::insert(
            &pool,
            &refs::NewRefImage {
                name: "r".into(),
                ref_group_id: None,
                ephemeral: false,
                file_path: "/r".into(),
                thumb_path: "/t".into(),
                width: 1,
                height: 1,
                file_size: 1,
                content_hash: None,
                upload_path: None,
            },
        )
        .await
        .unwrap();

        let mut tx = pool.begin().await.unwrap();
        let gid = prompts::create_group(&mut tx, "组", "GG", "", false)
            .await
            .unwrap();
        let p1 = prompts::insert_prompt(&mut tx, gid, "GG-0001", None, "a", "library")
            .await
            .unwrap();
        let p2 = prompts::insert_prompt(&mut tx, gid, "GG-0002", None, "b", "library")
            .await
            .unwrap();
        let bid = tasks::create_batch(&mut tx, "/out", "{}").await.unwrap();
        // 组合1 (r,p1) 产出并通过；组合2 (r,p2) 产出但未通过。
        let t1 = tasks::insert_task(&mut tx, bid, rid, p1, "a", 1)
            .await
            .unwrap();
        let t2 = tasks::insert_task(&mut tx, bid, rid, p2, "b", 1)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        sqlx::query("UPDATE tasks SET status='pass' WHERE id=?1")
            .bind(t1)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE tasks SET status='rej' WHERE id=?1")
            .bind(t2)
            .execute(&pool)
            .await
            .unwrap();
        let mut tx = pool.begin().await.unwrap();
        works::insert(
            &mut tx,
            &works::NewWork {
                task_id: t1,
                image_path: "/i.jpg".into(),
                thumb_path: "/it.jpg".into(),
                prompt_id: p1,
                prompt_text: "a".into(),
                group_id: Some(gid),
                ref_image_id: rid,
                batch_id: bid,
            },
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();

        let stats = group_stats(&pool).await.unwrap();
        let g = stats.iter().find(|s| s.group_id == gid).unwrap();
        assert_eq!(g.combos, 2, "两个组合都已产出图");
        assert_eq!(g.passed, 1, "仅一个组合通过");
        assert_eq!(g.works, 1);
    }
}

/// 单条提示词的产出统计（E25 提示词详情）。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PromptStat {
    pub works: i64,
    pub combos: i64,
    pub passed: i64,
}

#[tauri::command]
#[specta::specta]
pub async fn prompt_stats(state: State<'_, AppState>, prompt_id: i64) -> AppResult<PromptStat> {
    let works: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM accepted_works WHERE prompt_id = ?1")
        .bind(prompt_id)
        .fetch_one(&state.db)
        .await?;
    // 组合以参考图去重（同一提示词下每张参考图算一个组合）。
    let combos: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT ref_image_id) FROM tasks
         WHERE prompt_id = ?1 AND status IN ('rev','pass','rej')",
    )
    .bind(prompt_id)
    .fetch_one(&state.db)
    .await?;
    let passed: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT ref_image_id) FROM accepted_works WHERE prompt_id = ?1",
    )
    .bind(prompt_id)
    .fetch_one(&state.db)
    .await?;
    Ok(PromptStat {
        works,
        combos,
        passed,
    })
}

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
