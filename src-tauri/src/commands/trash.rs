//! trash 域命令（废纸篓，执行计划 2.1 / 需求 13.4）。
//! 清理 = 物理删文件 + 级联删记录 + 编号回收，不可恢复。

use std::path::PathBuf;

use serde::Serialize;
use specta::Type;
use tauri::State;

use crate::db::repo::tasks as task_repo;
use crate::db::repo::trash as repo;
use crate::error::AppResult;
use crate::{files, ids};

/// 启动时到期自动清理（E22 批次 + E40 废纸篓，决策 D3）。
/// 各自 0 天 = 关闭。清理失败仅告警，不阻断启动。返回 (删除批次数, 清理废纸篓项数)。
pub async fn run_startup_cleanup(
    pool: &sqlx::SqlitePool,
    batch_retention_days: i64,
    trash_retention_days: i64,
) -> (u64, i64) {
    let now = crate::db::now_unix();
    let mut batches_deleted = 0u64;
    let mut trash_purged = 0i64;

    if batch_retention_days > 0 {
        let cutoff = now - batch_retention_days * 86_400;
        match task_repo::delete_batches_archived_before(pool, cutoff).await {
            Ok(n) => batches_deleted = n,
            Err(e) => tracing::warn!(error = %e, "归档批次自动清理失败"),
        }
    }
    if trash_retention_days > 0 {
        let cutoff = now - trash_retention_days * 86_400;
        match repo::expired_ids(pool, cutoff).await {
            Ok(ids) if !ids.is_empty() => match purge_ids(pool, &ids).await {
                Ok(n) => trash_purged = n,
                Err(e) => tracing::warn!(error = %e, "废纸篓到期项自动清理失败"),
            },
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "废纸篓到期项查询失败"),
        }
    }
    if batches_deleted > 0 || trash_purged > 0 {
        tracing::info!(batches_deleted, trash_purged, "启动自动清理完成（D3）");
    }
    (batches_deleted, trash_purged)
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TrashItemView {
    pub id: i64,
    pub entity_type: String,
    pub code: Option<String>,
    pub title: Option<String>,
    pub ref_name: Option<String>,
    pub thumb_path: Option<String>,
    /// 未通过任务的原图路径（E02：原图暂存至清理前可查看）。仅 task 类有值。
    pub image_path: Option<String>,
    pub prompt_text: Option<String>,
    pub source_label: String,
    pub deleted_at: i64,
}

#[tauri::command]
#[specta::specta]
pub async fn list_trash(state: State<'_, crate::state::AppState>) -> AppResult<Vec<TrashItemView>> {
    let rows = repo::list(&state.db).await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            // 未通过任务的原图存于 file_paths 首位（E02）；仅 task 类暴露供查看。
            let image_path = (r.entity_type == "task")
                .then(|| {
                    serde_json::from_str::<Vec<String>>(&r.file_paths_json)
                        .ok()
                        .and_then(|v| v.into_iter().next())
                })
                .flatten();
            TrashItemView {
                id: r.id,
                entity_type: r.entity_type,
                code: r.code,
                title: r.title,
                ref_name: None, // trash_items 不冗余参考图名；列表以编号 + 提示词为主
                thumb_path: r.thumb_path,
                image_path,
                prompt_text: r.prompt_text,
                source_label: r.source_label,
                deleted_at: r.deleted_at,
            }
        })
        .collect())
}

/// 拆分编号 `DZ-0001` → (前缀, 序号)。
fn parse_code(code: &str) -> Option<(String, i64)> {
    let (prefix, num) = code.rsplit_once('-')?;
    let n: i64 = num.trim().parse().ok()?;
    Some((prefix.to_string(), n))
}

async fn purge(state: &crate::state::AppState, ids_in: &[i64]) -> AppResult<i64> {
    purge_ids(&state.db, ids_in).await
}

/// 物理删 + 级联删记录 + 编号回收（同事务）。命令层与启动清理（E40）共用。
pub async fn purge_ids(pool: &sqlx::SqlitePool, ids_in: &[i64]) -> AppResult<i64> {
    let rows = repo::take(pool, ids_in).await?;
    if rows.is_empty() {
        return Ok(0);
    }

    // 1) 物理删文件（缩略图 + file_paths_json）。
    for r in &rows {
        if let Some(t) = &r.thumb_path {
            let _ = files::purge(&PathBuf::from(t));
        }
        if let Ok(paths) = serde_json::from_str::<Vec<String>>(&r.file_paths_json) {
            for p in paths {
                let _ = files::purge(&PathBuf::from(p));
            }
        }
    }

    // 2) 级联删记录 + 编号回收（同事务）。
    let mut tx = pool.begin().await?;
    for r in &rows {
        match r.entity_type.as_str() {
            "prompt" => {
                if let Some(code) = &r.code {
                    if let Some((prefix, n)) = parse_code(code) {
                        ids::recycle(&mut tx, &prefix, n).await?;
                    }
                }
                if let Some(pid) = r.ref_id {
                    sqlx::query("DELETE FROM prompts WHERE id = ?1")
                        .bind(pid)
                        .execute(&mut *tx)
                        .await?;
                }
            }
            "ref" => {
                if let Some(rid) = r.ref_id {
                    sqlx::query("DELETE FROM ref_images WHERE id = ?1")
                        .bind(rid)
                        .execute(&mut *tx)
                        .await?;
                }
            }
            _ => {}
        }
    }
    let ids_vec: Vec<i64> = rows.iter().map(|r| r.id).collect();
    repo::delete_rows(&mut tx, &ids_vec).await?;
    tx.commit().await?;

    Ok(rows.len() as i64)
}

#[tauri::command]
#[specta::specta]
pub async fn purge_trash_items(
    state: State<'_, crate::state::AppState>,
    ids: Vec<i64>,
) -> AppResult<i64> {
    purge(&state, &ids).await
}

#[tauri::command]
#[specta::specta]
pub async fn purge_all_trash(state: State<'_, crate::state::AppState>) -> AppResult<i64> {
    let all = repo::all(&state.db).await?;
    let ids: Vec<i64> = all.iter().map(|r| r.id).collect();
    purge(&state, &ids).await
}

#[tauri::command]
#[specta::specta]
pub async fn count_trash(state: State<'_, crate::state::AppState>) -> AppResult<i64> {
    Ok(repo::count(&state.db).await?)
}
