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
    /// 最近一次挂靠的提示词组（E32 挂靠记忆）；生成页据此预填挂靠。
    pub last_group_id: Option<i64>,
    /// 已归档（0016）：批次开跑后自动置位，生成页选择器默认折起，库页仍可见可恢复。
    pub archived: bool,
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

        // E30b：内容 hash（去重比对用）。E41：超限则生成上传用压缩副本。
        let content_hash = files::content_hash(&dest).ok();
        let upload_dest = unique_path(&refs_dir, &format!("{stem}_up"), "jpg");
        let upload_path = match files::make_upload_copy(&dest, &upload_dest) {
            Ok(Some(_)) => Some(upload_dest.to_string_lossy().to_string()),
            _ => None,
        };

        let new = repo::NewRefImage {
            name: stem.clone(),
            group_id,
            file_path: dest.to_string_lossy().to_string(),
            thumb_path: thumb.to_string_lossy().to_string(),
            width: w as i64,
            height: h as i64,
            file_size,
            content_hash,
            upload_path,
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
            last_group_id: None,
            archived: false,
        });
    }

    Ok(views)
}

/// 导入前重复扫描（E30b）：按内容 hash 比对已有库 + 本次列表内，标注重复项。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RefScanItem {
    pub path: String,
    pub name: String,
    pub duplicate: bool,
    /// 与之重复的已有图名（库内）或本次靠前的文件名。
    pub dup_of: Option<String>,
}

#[tauri::command]
#[specta::specta]
pub async fn scan_ref_imports(
    state: State<'_, AppState>,
    paths: Vec<String>,
) -> AppResult<Vec<RefScanItem>> {
    // 库内 hash → 名称（best-effort 展示重复源）。
    let existing = repo::active_hash_names(&state.db).await?;
    let mut seen: std::collections::HashMap<String, String> = existing.into_iter().collect();
    let mut out = Vec::with_capacity(paths.len());
    for path in paths {
        let src = PathBuf::from(&path);
        let name = src
            .file_stem()
            .and_then(|s| s.to_str())
            .map(files::sanitize_filename)
            .unwrap_or_else(|| "未命名".into());
        let hash = files::content_hash(&src).ok();
        let dup_of = hash.as_ref().and_then(|h| seen.get(h).cloned());
        let duplicate = dup_of.is_some();
        if let Some(h) = hash {
            seen.entry(h).or_insert_with(|| name.clone());
        }
        out.push(RefScanItem {
            path,
            name,
            duplicate,
            dup_of,
        });
    }
    Ok(out)
}

/// 批量改分组（E30b）。gid=None 为未分组。
#[tauri::command]
#[specta::specta]
pub async fn set_ref_images_group(
    state: State<'_, AppState>,
    ids: Vec<i64>,
    group_id: Option<i64>,
) -> AppResult<()> {
    repo::set_group_many(&state.db, &ids, group_id).await?;
    Ok(())
}

/// 批量删除参考图 → 进废纸篓（E30b）。
#[tauri::command]
#[specta::specta]
pub async fn trash_ref_images(state: State<'_, AppState>, ids: Vec<i64>) -> AppResult<()> {
    for id in ids {
        trash_one_ref(&state, id).await?;
    }
    Ok(())
}

/// 单张参考图入废纸篓（原图 + 缩略图 + 上传副本一并暂存至清理）。
async fn trash_one_ref(state: &AppState, id: i64) -> AppResult<()> {
    let Some(row) = repo::soft_delete(&state.db, id).await? else {
        return Ok(());
    };
    let mut file_paths = vec![row.file_path.clone(), row.thumb_path.clone()];
    if let Some(up) = &row.upload_path {
        file_paths.push(up.clone());
    }
    let mut tx = state.db.begin().await?;
    crate::db::repo::trash::insert(
        &mut tx,
        &crate::db::repo::trash::NewTrashItem {
            entity_type: "ref".into(),
            ref_id: Some(row.id),
            thumb_path: Some(row.thumb_path.clone()),
            prompt_text: None,
            code: None,
            title: None,
            source_label: "手动删除".into(),
            file_paths,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(())
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
            last_group_id: r.last_group_id,
            archived: r.archived_at.is_some(),
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

/// 归档 / 取消归档参考图（0016）。批次开跑后由 `engine::create_batch` 自动归档；
/// 此命令供参考图库手动恢复（或手动归档一张用不上的旧图）。
#[tauri::command]
#[specta::specta]
pub async fn set_ref_image_archived(
    state: State<'_, AppState>,
    id: i64,
    archived: bool,
) -> AppResult<()> {
    if !repo::set_archived(&state.db, id, archived).await? {
        return Err(AppError::InvalidInput("参考图不存在".into()));
    }
    Ok(())
}

/// 参考图详情（含使用统计）。
#[derive(Debug, Clone, serde::Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RefImageDetail {
    pub id: i64,
    pub name: String,
    pub group_id: Option<i64>,
    pub file_path: String,
    pub thumb_path: String,
    pub width: i64,
    pub height: i64,
    pub used_count: i64,
    pub works_count: i64,
}

#[tauri::command]
#[specta::specta]
pub async fn get_ref_image(state: State<'_, AppState>, id: i64) -> AppResult<RefImageDetail> {
    let r = repo::get(&state.db, id)
        .await?
        .ok_or_else(|| AppError::InvalidInput("参考图不存在".into()))?;
    let (used, works) = repo::usage_stats(&state.db, id).await?;
    Ok(RefImageDetail {
        id: r.id,
        name: r.name,
        group_id: r.group_id,
        file_path: r.file_path,
        thumb_path: r.thumb_path,
        width: r.width,
        height: r.height,
        used_count: used,
        works_count: works,
    })
}

/// 更换参考图文件（保留 id 与关联）。
#[tauri::command]
#[specta::specta]
pub async fn replace_ref_image_file(
    state: State<'_, AppState>,
    id: i64,
    path: String,
) -> AppResult<()> {
    state.dirs.init()?;
    let src = PathBuf::from(&path);
    let stem = src
        .file_stem()
        .and_then(|s| s.to_str())
        .map(files::sanitize_filename)
        .ok_or_else(|| AppError::InvalidInput("无效文件路径".into()))?;
    let ext = src
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("png")
        .to_lowercase();
    let dest = unique_path(&state.dirs.refs(), &stem, &ext);
    std::fs::copy(&src, &dest)?;
    let thumb = unique_path(&state.dirs.thumbs(), &stem, "jpg");
    let (w, h) = files::generate_thumbnail(&dest, &thumb)?;
    let size = std::fs::metadata(&dest)
        .map(|m| m.len() as i64)
        .unwrap_or(0);
    // 文件已更换：刷新内容 hash（E30b）与上传压缩副本（E41）。
    let content_hash = files::content_hash(&dest).ok();
    let upload_dest = unique_path(&state.dirs.refs(), &format!("{stem}_up"), "jpg");
    let upload_path = match files::make_upload_copy(&dest, &upload_dest) {
        Ok(Some(_)) => Some(upload_dest.to_string_lossy().to_string()),
        _ => None,
    };
    repo::update_file(
        &state.db,
        id,
        &dest.to_string_lossy(),
        &thumb.to_string_lossy(),
        w as i64,
        h as i64,
        size,
        content_hash.as_deref(),
        upload_path.as_deref(),
    )
    .await?;
    Ok(())
}

/// 删除参考图 → 进废纸篓。
#[tauri::command]
#[specta::specta]
pub async fn trash_ref_image(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    trash_one_ref(&state, id).await
}
