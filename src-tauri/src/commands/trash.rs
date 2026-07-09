//! trash 域命令（废纸篓，执行计划 2.1 / 需求 13.4）。
//! 清理 = 物理删文件 + 级联删记录 + 编号回收，不可恢复。

use std::path::PathBuf;

use serde::Serialize;
use specta::Type;
use tauri::State;

use crate::db::repo::trash as repo;
use crate::error::AppResult;
use crate::{files, ids};

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TrashItemView {
    pub id: i64,
    pub entity_type: String,
    pub code: Option<String>,
    pub ref_name: Option<String>,
    pub thumb_path: Option<String>,
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
        .map(|r| TrashItemView {
            id: r.id,
            entity_type: r.entity_type,
            code: r.code,
            ref_name: None, // trash_items 不冗余参考图名；列表以编号 + 提示词为主
            thumb_path: r.thumb_path,
            prompt_text: r.prompt_text,
            source_label: r.source_label,
            deleted_at: r.deleted_at,
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
    let rows = repo::take(&state.db, ids_in).await?;
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
    let mut tx = state.db.begin().await?;
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
