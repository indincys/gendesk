//! works 域命令（作品库，执行计划 2.1 / 需求 14.4）。

use serde::{Deserialize, Serialize};
use specta::Type;
use sqlx::FromRow;
use tauri::State;

use crate::db::repo::{trash as trash_repo, works as work_repo};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

#[derive(Debug, Clone, Serialize, Type, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct WorkView {
    pub id: i64,
    pub prompt_code: String,
    pub group_name: String,
    pub ref_name: String,
    pub batch_id: Option<i64>,
    pub favorite: i64,
    pub accepted_at: i64,
    pub image_path: String,
    pub thumb_path: String,
    pub prompt_text: String,
}

#[derive(Debug, Clone, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct WorkFilter {
    pub group_id: Option<i64>,
    pub favorite_only: bool,
}

const WORK_SELECT: &str = "SELECT w.id, COALESCE(p.code,'') AS prompt_code,
        COALESCE(g.name,'') AS group_name, COALESCE(r.name,'') AS ref_name,
        w.batch_id, w.favorite, w.accepted_at, w.image_path, w.thumb_path, w.prompt_text
    FROM accepted_works w
    LEFT JOIN prompts p ON p.id = w.prompt_id
    LEFT JOIN prompt_groups g ON g.id = w.group_id
    LEFT JOIN ref_images r ON r.id = w.ref_image_id";

#[tauri::command]
#[specta::specta]
pub async fn list_works(
    state: State<'_, AppState>,
    filter: WorkFilter,
    page: Option<i64>,
) -> AppResult<Vec<WorkView>> {
    let mut sql = String::from(WORK_SELECT);
    let mut conds: Vec<String> = Vec::new();
    if filter.group_id.is_some() {
        conds.push("w.group_id = ?".into());
    }
    if filter.favorite_only {
        conds.push("w.favorite = 1".into());
    }
    if !conds.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&conds.join(" AND "));
    }
    sql.push_str(" ORDER BY w.accepted_at DESC, w.id DESC LIMIT ? OFFSET ?");

    let limit = 200i64;
    let offset = page.unwrap_or(0).max(0) * limit;
    let mut q = sqlx::query_as::<_, WorkView>(&sql);
    if let Some(gid) = filter.group_id {
        q = q.bind(gid);
    }
    Ok(q.bind(limit).bind(offset).fetch_all(&state.db).await?)
}

#[tauri::command]
#[specta::specta]
pub async fn get_work(state: State<'_, AppState>, id: i64) -> AppResult<WorkView> {
    let sql = format!("{WORK_SELECT} WHERE w.id = ?");
    sqlx::query_as::<_, WorkView>(&sql)
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::InvalidInput("作品不存在".into()))
}

#[tauri::command]
#[specta::specta]
pub async fn toggle_work_favorite(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    work_repo::toggle_favorite(&state.db, id).await?;
    Ok(())
}

/// 删除作品 → 进废纸篓（记录删除，文件待清理时物理删）。
#[tauri::command]
#[specta::specta]
pub async fn trash_work(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    let Some(row) = work_repo::delete(&state.db, id).await? else {
        return Ok(());
    };
    let mut tx = state.db.begin().await?;
    trash_repo::insert(
        &mut tx,
        &trash_repo::NewTrashItem {
            entity_type: "work".into(),
            ref_id: Some(row.id),
            thumb_path: Some(row.thumb_path.clone()),
            prompt_text: Some(row.prompt_text.clone()),
            code: None,
            title: None,
            source_label: "手动删除".into(),
            file_paths: vec![row.image_path, row.thumb_path],
        },
    )
    .await?;
    tx.commit().await?;
    Ok(())
}
