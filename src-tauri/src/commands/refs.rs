//! refs 域：参考图导入（执行计划 2.1 / 1.5）。拷入 refs/、生成 512px 缩略图、入库。

use std::path::{Path, PathBuf};

use serde::Serialize;
use specta::Type;
use tauri::State;

use crate::db::repo::refs as repo;
use crate::error::{AppError, AppResult};
use crate::files;
use crate::state::AppState;

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RefImageView {
    pub id: i64,
    pub name: String,
    pub group_id: Option<i64>,
    pub file_path: String,
    pub thumb_path: String,
    pub width: i64,
    pub height: i64,
}

/// 在目录内生成不冲突的路径。
fn unique_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let first = if ext.is_empty() {
        dir.join(stem)
    } else {
        dir.join(format!("{stem}.{ext}"))
    };
    if !first.exists() {
        return first;
    }
    let mut n = 1;
    loop {
        let name = if ext.is_empty() {
            format!("{stem}_{n}")
        } else {
            format!("{stem}_{n}.{ext}")
        };
        let candidate = dir.join(name);
        if !candidate.exists() {
            return candidate;
        }
        n += 1;
    }
}

/// 导入参考图：拷入库、生成缩略图、写记录，返回视图列表。
#[tauri::command]
#[specta::specta]
pub async fn import_ref_images(
    state: State<'_, AppState>,
    paths: Vec<String>,
    group_id: Option<i64>,
) -> AppResult<Vec<RefImageView>> {
    state.dirs.init()?;
    let refs_dir = state.dirs.refs();
    let thumbs_dir = state.dirs.thumbs();
    let mut views = Vec::with_capacity(paths.len());

    for path in paths {
        let src = PathBuf::from(&path);
        let stem = src
            .file_stem()
            .and_then(|s| s.to_str())
            .map(files::sanitize_filename)
            .ok_or_else(|| AppError::InvalidInput(format!("无效文件路径：{path}")))?;
        let ext = src
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("png")
            .to_lowercase();

        // 拷入 refs/
        let dest = unique_path(&refs_dir, &stem, &ext);
        std::fs::copy(&src, &dest)?;

        // 生成缩略图
        let thumb = unique_path(&thumbs_dir, &stem, "jpg");
        let (w, h) = files::generate_thumbnail(&dest, &thumb)?;
        let file_size = std::fs::metadata(&dest)
            .map(|m| m.len() as i64)
            .unwrap_or(0);

        let new = repo::NewRefImage {
            name: stem.clone(),
            group_id,
            file_path: dest.to_string_lossy().to_string(),
            thumb_path: thumb.to_string_lossy().to_string(),
            width: w as i64,
            height: h as i64,
            file_size,
        };
        let id = repo::insert(&state.db, &new).await?;
        views.push(RefImageView {
            id,
            name: new.name,
            group_id,
            file_path: new.file_path,
            thumb_path: new.thumb_path,
            width: new.width,
            height: new.height,
        });
    }

    Ok(views)
}

/// 列出全部未删除参考图（供参考图库/生成页选择）。
#[tauri::command]
#[specta::specta]
pub async fn list_ref_images(state: State<'_, AppState>) -> AppResult<Vec<RefImageView>> {
    let rows = repo::list_active(&state.db).await?;
    Ok(rows
        .into_iter()
        .map(|r| RefImageView {
            id: r.id,
            name: r.name,
            group_id: r.group_id,
            file_path: r.file_path,
            thumb_path: r.thumb_path,
            width: r.width,
            height: r.height,
        })
        .collect())
}

/// 调整参考图分组。
#[tauri::command]
#[specta::specta]
pub async fn set_ref_image_group(
    state: State<'_, AppState>,
    id: i64,
    group_id: Option<i64>,
) -> AppResult<()> {
    repo::set_group(&state.db, id, group_id).await?;
    Ok(())
}
