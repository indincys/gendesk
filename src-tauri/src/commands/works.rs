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
    /// 复刻/再生成所需的原始关联（E33）；批次删除后 task_id 可能为空。
    pub ref_image_id: Option<i64>,
    pub group_id: Option<i64>,
    pub task_id: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct WorkFilter {
    pub group_id: Option<i64>,
    pub favorite_only: bool,
}

const WORK_SELECT: &str = "SELECT w.id, COALESCE(p.code,'') AS prompt_code,
        COALESCE(g.name,'') AS group_name, COALESCE(r.name,'') AS ref_name,
        w.batch_id, w.favorite, w.accepted_at, w.image_path, w.thumb_path, w.prompt_text,
        w.ref_image_id, w.group_id, w.task_id
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
            // E21 决策：默认**不**物理删除外部输出文件（用户可能已发布/引用）；仅清缩略图。
            file_paths: vec![row.thumb_path],
        },
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

/// 批量收藏（E15）。favorite=true 收藏，false 取消。
#[tauri::command]
#[specta::specta]
pub async fn set_works_favorite(
    state: State<'_, AppState>,
    ids: Vec<i64>,
    favorite: bool,
) -> AppResult<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let ph = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!("UPDATE accepted_works SET favorite = ? WHERE id IN ({ph})");
    let mut q = sqlx::query(&sql).bind(favorite as i64);
    for id in &ids {
        q = q.bind(id);
    }
    q.execute(&state.db).await?;
    Ok(())
}

/// 批量删除作品 → 进废纸篓（E15）。默认不物理删除外部输出文件（同 trash_work 决策）。
#[tauri::command]
#[specta::specta]
pub async fn trash_works(state: State<'_, AppState>, ids: Vec<i64>) -> AppResult<()> {
    for id in ids {
        let Some(row) = work_repo::delete(&state.db, id).await? else {
            continue;
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
                source_label: "批量删除".into(),
                file_paths: vec![row.thumb_path],
            },
        )
        .await?;
        tx.commit().await?;
    }
    Ok(())
}

/// 批量导出作品到指定文件夹（E15）：复制各作品输出文件（image_path）到目标目录。
/// 返回成功导出数；源文件缺失的项跳过（不计入）。
#[tauri::command]
#[specta::specta]
pub async fn export_works(
    state: State<'_, AppState>,
    ids: Vec<i64>,
    dest_dir: String,
) -> AppResult<i64> {
    if ids.is_empty() {
        return Ok(0);
    }
    let dest = std::path::PathBuf::from(&dest_dir);
    std::fs::create_dir_all(&dest).map_err(|e| AppError::Io(e.to_string()))?;

    let ph = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!("SELECT image_path FROM accepted_works WHERE id IN ({ph})");
    let mut q = sqlx::query_scalar::<_, String>(&sql);
    for id in &ids {
        q = q.bind(id);
    }
    let paths = q.fetch_all(&state.db).await?;

    let mut exported = 0i64;
    for p in paths {
        let src = std::path::PathBuf::from(&p);
        if !src.is_file() {
            continue;
        }
        let Some(name) = src.file_name() else {
            continue;
        };
        // 目标同名冲突时追加序号，避免覆盖。
        let mut out = dest.join(name);
        let mut n = 1;
        while out.exists() {
            let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("work");
            let ext = src.extension().and_then(|s| s.to_str()).unwrap_or("jpg");
            out = dest.join(format!("{stem}_{n}.{ext}"));
            n += 1;
        }
        if std::fs::copy(&src, &out).is_ok() {
            exported += 1;
        }
    }
    Ok(exported)
}

/// 文件是否存在（E21 作品源文件缺失懒检测）。
#[tauri::command]
#[specta::specta]
pub async fn file_exists(path: String) -> AppResult<bool> {
    Ok(std::path::Path::new(&path).is_file())
}

/// 从资产区快照重新导出作品输出文件（E21）：源为 `results/{task_id}.{ext}`。
/// 批次已删除（task_id 为空）或快照已随清理消失时报可读错误。
#[tauri::command]
#[specta::specta]
pub async fn reexport_work(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    let row: Option<(Option<i64>, String)> =
        sqlx::query_as("SELECT task_id, image_path FROM accepted_works WHERE id = ?1")
            .bind(id)
            .fetch_optional(&state.db)
            .await?;
    let Some((task_id, image_path)) = row else {
        return Err(AppError::InvalidInput("作品不存在".into()));
    };
    let Some(task_id) = task_id else {
        return Err(AppError::InvalidInput(
            "该作品所属批次已清理，源快照不存在，无法重新导出".into(),
        ));
    };
    // 任务1：结果快照扩展名跟随输出文件（默认 jpg；保留原格式时可能 png）。
    let ext = std::path::Path::new(&image_path)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("jpg")
        .to_lowercase();
    let src = state.dirs.results().join(format!("{task_id}.{ext}"));
    if !src.is_file() {
        return Err(AppError::InvalidInput(
            "资产区源快照已不存在（可能随批次清理删除），无法重新导出".into(),
        ));
    }
    let dst = std::path::PathBuf::from(&image_path);
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).map_err(|e| AppError::Io(e.to_string()))?;
    }
    std::fs::copy(&src, &dst).map_err(|e| AppError::Io(e.to_string()))?;
    Ok(())
}
