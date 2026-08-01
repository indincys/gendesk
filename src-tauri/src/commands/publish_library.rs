//! 图片素材库与商品级文案/话题库命令。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;
use walkdir::WalkDir;

use crate::commands::publish_settings;
use crate::db::repo::{copy as copy_repo, images as image_repo, products, trash as trash_repo};
use crate::error::{AppError, AppResult};
use crate::publish::{copy_ingest, paths};
use crate::state::AppState;

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ImageAssetView {
    pub id: i64,
    pub sku_id: i64,
    pub sku_code: String,
    pub sku_name: String,
    pub product_id: Option<i64>,
    pub product_name: Option<String>,
    pub path: String,
    pub thumb: String,
    pub source: String,
    pub state: String,
    pub post_id: Option<i64>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ImageImportReport {
    pub imported: i64,
    pub unmatched: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CopyItemView {
    pub id: i64,
    pub product_id: i64,
    pub product_code: String,
    pub product_name: String,
    pub kind: String,
    pub text: String,
    pub source: String,
    pub enabled: bool,
    pub state: String,
    pub post_id: Option<i64>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TopicGroupView {
    pub id: i64,
    pub product_id: Option<i64>,
    pub product_name: Option<String>,
    pub scope: String,
    pub sku_ids: Vec<i64>,
    pub tags: Vec<String>,
    pub enabled: bool,
    pub created_at: i64,
}

#[derive(Debug, Clone, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TopicGroupInput {
    pub product_id: Option<i64>,
    pub scope: String,
    pub sku_ids: Vec<i64>,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CopyFilePreview {
    pub product_code: Option<String>,
    pub kind: String,
    pub titles: i64,
    pub bodies: i64,
    pub topics: Vec<String>,
}

#[tauri::command]
#[specta::specta]
pub async fn list_image_assets(
    state: State<'_, AppState>,
    product_id: Option<i64>,
    sku_id: Option<i64>,
    asset_state: Option<String>,
) -> AppResult<Vec<ImageAssetView>> {
    let root = publish_settings::root_local(&state.db).await?;
    Ok(
        image_repo::list(&state.db, product_id, sku_id, asset_state.as_deref())
            .await?
            .into_iter()
            .map(|row| ImageAssetView {
                id: row.id,
                sku_id: row.sku_id,
                sku_code: row.sku_code,
                sku_name: row.sku_name,
                product_id: row.product_id,
                product_name: row.product_name,
                path: paths::RelPath::new(&row.path_rel)
                    .to_local(&root)
                    .to_string_lossy()
                    .to_string(),
                thumb: paths::RelPath::new(&row.thumb_rel)
                    .to_local(&root)
                    .to_string_lossy()
                    .to_string(),
                source: row.source,
                state: row.state,
                post_id: row.post_id,
                created_at: row.created_at,
            })
            .collect(),
    )
}

#[tauri::command]
#[specta::specta]
pub async fn pick_image_folder(app: AppHandle) -> AppResult<Option<String>> {
    Ok(app
        .dialog()
        .file()
        .blocking_pick_folder()
        .and_then(|path| path.into_path().ok())
        .map(|path| path.to_string_lossy().to_string()))
}

#[tauri::command]
#[specta::specta]
pub async fn pick_copy_file(app: AppHandle) -> AppResult<Option<String>> {
    Ok(app
        .dialog()
        .file()
        .add_filter("文本文案", &["txt"])
        .blocking_pick_file()
        .and_then(|path| path.into_path().ok())
        .map(|path| path.to_string_lossy().to_string()))
}

fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "jpg" | "jpeg" | "png" | "webp"
            )
        })
}

#[tauri::command]
#[specta::specta]
pub async fn import_image_folder(
    state: State<'_, AppState>,
    folder: String,
    fallback_sku_id: Option<i64>,
) -> AppResult<ImageImportReport> {
    let source_root = PathBuf::from(&folder);
    if !source_root.is_dir() {
        return Err(AppError::InvalidInput("所选路径不是文件夹".into()));
    }
    let root = publish_settings::root_local(&state.db).await?;
    let skus = products::list_skus(&state.db, None).await?;
    let mut by_name = HashMap::new();
    let mut code_by_id = HashMap::new();
    for sku in &skus {
        by_name.insert(sku.code.to_ascii_lowercase(), sku.id);
        if !sku.folder_alias.trim().is_empty() {
            by_name.insert(sku.folder_alias.to_ascii_lowercase(), sku.id);
        }
        code_by_id.insert(sku.id, sku.code.clone());
    }
    let root_name = source_root
        .file_name()
        .and_then(|x| x.to_str())
        .unwrap_or("");
    let mut unmatched = Vec::new();
    let mut jobs: Vec<(PathBuf, i64, paths::RelPath)> = Vec::new();
    let mut taken: HashSet<String> = sqlx::query_scalar("SELECT path_rel FROM image_assets")
        .fetch_all(&state.db)
        .await?
        .into_iter()
        .collect();
    let mut sequence = 0usize;
    for entry in WalkDir::new(&source_root)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
    {
        let source = entry.path();
        if !entry.file_type().is_file() || !is_image(source) {
            continue;
        }
        let rel = source.strip_prefix(&source_root).unwrap_or(source);
        let top = rel
            .components()
            .next()
            .and_then(|c| c.as_os_str().to_str())
            .unwrap_or(root_name);
        let auto = by_name
            .get(&top.to_ascii_lowercase())
            .copied()
            .or_else(|| by_name.get(&root_name.to_ascii_lowercase()).copied());
        let Some(sku_id) = auto.or(fallback_sku_id) else {
            if !unmatched.iter().any(|x| x == top) {
                unmatched.push(top.to_string());
            }
            continue;
        };
        let Some(code) = code_by_id.get(&sku_id) else {
            return Err(AppError::InvalidInput("手动指认的 SKU 不存在".into()));
        };
        sequence += 1;
        let ext = paths::ascii_ext(&source.to_string_lossy());
        let stem = source
            .file_stem()
            .and_then(|x| x.to_str())
            .unwrap_or("image");
        let base = format!("{}_{sequence:04}.{ext}", paths::ascii_slug(stem));
        let filename = paths::dedupe_name(&base, &|candidate| {
            let candidate_rel = paths::RelPath::from_parts([paths::IMAGE_LIBRARY, code, candidate]);
            taken.contains(candidate_rel.as_str()) || candidate_rel.to_local(&root).exists()
        });
        let rel = paths::RelPath::from_parts([paths::IMAGE_LIBRARY, code, &filename]);
        taken.insert(rel.as_str().to_string());
        jobs.push((source.to_path_buf(), sku_id, rel));
    }
    let destinations: Vec<(PathBuf, i64, paths::RelPath)> = jobs;
    let copy_jobs: Vec<(PathBuf, PathBuf)> = destinations
        .iter()
        .map(|(source, _, rel)| (source.clone(), rel.to_local(&root)))
        .collect();
    let copied_paths = copy_jobs
        .iter()
        .map(|(_, destination)| destination.clone())
        .collect::<Vec<_>>();
    tokio::task::spawn_blocking(move || paths::copy_batch_new(&copy_jobs))
        .await
        .map_err(|err| AppError::Io(format!("图片导入任务失败：{err}")))??;
    let mut copied_guard = paths::CreatedFilesGuard::new(copied_paths);

    let mut tx = state.db.begin().await?;
    for (_, sku_id, rel) in &destinations {
        image_repo::insert(&mut tx, *sku_id, rel.as_str(), rel.as_str(), "import", None).await?;
    }
    tx.commit().await?;
    copied_guard.preserve_all();
    Ok(ImageImportReport {
        imported: destinations.len() as i64,
        unmatched,
    })
}

#[tauri::command]
#[specta::specta]
pub async fn set_image_assets_sku(
    state: State<'_, AppState>,
    ids: Vec<i64>,
    sku_id: i64,
) -> AppResult<i64> {
    Ok(image_repo::set_sku(&state.db, &ids, sku_id).await? as i64)
}

#[tauri::command]
#[specta::specta]
pub async fn delete_image_assets(state: State<'_, AppState>, ids: Vec<i64>) -> AppResult<i64> {
    let root = publish_settings::root_local(&state.db).await?;
    let mut deleted = 0;
    for id in ids {
        deleted += trash_image_asset(&state.db, &root, id).await? as i64;
    }
    Ok(deleted)
}

/// free 图片删除时只删记录并写入废纸篓，文件留到“彻底删除”才物理清理。
/// 图片行快照与废纸篓记录同事务写入，保证误删后可以恢复为 free。
pub(crate) async fn trash_image_asset(
    pool: &sqlx::SqlitePool,
    root: &Path,
    id: i64,
) -> AppResult<bool> {
    let Some(row) = image_repo::get(pool, id).await? else {
        return Ok(false);
    };
    if row.state != "free" {
        return Ok(false);
    }
    let path = paths::RelPath::new(&row.path_rel).to_local(root);
    let thumb = paths::RelPath::new(&row.thumb_rel).to_local(root);
    let sku_code: Option<String> = sqlx::query_scalar("SELECT code FROM skus WHERE id=?1")
        .bind(row.sku_id)
        .fetch_optional(pool)
        .await?;
    let mut tx = pool.begin().await?;
    // 失败任务释放后的图片仍可能出现在历史帖子快照里。废纸篓载荷已经保留图片行，
    // 因此这里只断开非占用的历史引用。
    sqlx::query("DELETE FROM post_images WHERE asset_id=?1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    trash_repo::insert(
        &mut tx,
        &trash_repo::NewTrashItem {
            entity_type: "image_asset".into(),
            ref_id: Some(row.id),
            thumb_path: Some(thumb.to_string_lossy().to_string()),
            prompt_text: None,
            code: sku_code,
            title: path
                .file_name()
                .map(|name| name.to_string_lossy().to_string()),
            source_label: "图片素材库删除".into(),
            file_paths: (path != thumb)
                .then(|| path.to_string_lossy().to_string())
                .into_iter()
                .collect(),
            payload_json: Some(serde_json::to_string(&row)?),
        },
    )
    .await?;
    let changed = sqlx::query("DELETE FROM image_assets WHERE id=?1 AND state='free'")
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if changed == 0 {
        tx.rollback().await?;
        return Ok(false);
    }
    tx.commit().await?;
    Ok(true)
}

/// 把存量作品补录进某个 SKU 图片库。新作品优先走验收时自动入库，此命令只服务存量与改挂靠。
#[tauri::command]
#[specta::specta]
pub async fn add_works_to_image_library(
    state: State<'_, AppState>,
    sku_id: i64,
    work_ids: Vec<i64>,
) -> AppResult<i64> {
    let root = publish_settings::root_local(&state.db).await?;
    let sku_code: Option<String> = sqlx::query_scalar("SELECT code FROM skus WHERE id=?1")
        .bind(sku_id)
        .fetch_optional(&state.db)
        .await?;
    let sku_code = sku_code.ok_or_else(|| AppError::InvalidInput("SKU 不存在".into()))?;
    let mut jobs = Vec::new();
    for work_id in work_ids {
        let existing: Option<String> =
            sqlx::query_scalar("SELECT state FROM image_assets WHERE work_id=?1")
                .bind(work_id)
                .fetch_optional(&state.db)
                .await?;
        if let Some(asset_state) = existing {
            return Err(AppError::InvalidInput(format!(
                "作品 {work_id} 已在图片素材库（{asset_state}），请在素材库中改归属"
            )));
        }
        let source: Option<String> =
            sqlx::query_scalar("SELECT image_path FROM accepted_works WHERE id=?1")
                .bind(work_id)
                .fetch_optional(&state.db)
                .await?;
        let Some(source) = source else { continue };
        let ext = paths::ascii_ext(&source);
        let filename = format!("work_{work_id}.{ext}");
        let rel = paths::RelPath::from_parts([
            paths::IMAGE_LIBRARY,
            sku_code.as_str(),
            filename.as_str(),
        ]);
        jobs.push((work_id, PathBuf::from(source), rel));
    }
    let copies = jobs
        .iter()
        .map(|(_, source, rel)| (source.clone(), rel.to_local(&root)))
        .collect::<Vec<_>>();
    let copied_paths = copies
        .iter()
        .map(|(_, destination)| destination.clone())
        .collect::<Vec<_>>();
    tokio::task::spawn_blocking(move || paths::copy_batch_new(&copies))
        .await
        .map_err(|err| AppError::Io(format!("作品补录任务失败：{err}")))??;
    let mut copied_guard = paths::CreatedFilesGuard::new(copied_paths);
    let mut tx = state.db.begin().await?;
    for (work_id, _, rel) in &jobs {
        image_repo::insert(
            &mut tx,
            sku_id,
            rel.as_str(),
            rel.as_str(),
            "works",
            Some(*work_id),
        )
        .await?;
        sqlx::query("UPDATE accepted_works SET sku_id=?2 WHERE id=?1")
            .bind(work_id)
            .bind(sku_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    copied_guard.preserve_all();
    Ok(jobs.len() as i64)
}

#[tauri::command]
#[specta::specta]
pub async fn list_copy_items(
    state: State<'_, AppState>,
    product_id: Option<i64>,
    kind: String,
) -> AppResult<Vec<CopyItemView>> {
    if !matches!(kind.as_str(), "title" | "body") {
        return Err(AppError::InvalidInput("文案类型非法".into()));
    }
    Ok(copy_repo::list_copy(&state.db, product_id, &kind)
        .await?
        .into_iter()
        .map(|row| CopyItemView {
            id: row.id,
            product_id: row.product_id,
            product_code: row.product_code,
            product_name: row.product_name,
            kind: row.kind,
            text: row.text,
            source: row.source,
            enabled: row.enabled != 0,
            state: row.state,
            post_id: row.post_id,
            created_at: row.created_at,
        })
        .collect())
}

#[tauri::command]
#[specta::specta]
pub async fn add_copy_item(
    state: State<'_, AppState>,
    product_id: i64,
    kind: String,
    text: String,
) -> AppResult<i64> {
    if !matches!(kind.as_str(), "title" | "body") || text.trim().is_empty() {
        return Err(AppError::InvalidInput("文案类型或内容非法".into()));
    }
    let mut tx = state.db.begin().await?;
    let id = copy_repo::insert_copy(&mut tx, product_id, &kind, text.trim(), "manual").await?;
    tx.commit().await?;
    Ok(id)
}

#[tauri::command]
#[specta::specta]
pub async fn update_copy_item(state: State<'_, AppState>, id: i64, text: String) -> AppResult<()> {
    if text.trim().is_empty() {
        return Err(AppError::InvalidInput("文案不能为空".into()));
    }
    let n = sqlx::query("UPDATE text_items SET text=?2 WHERE id=?1 AND state='free'")
        .bind(id)
        .bind(text.trim())
        .execute(&state.db)
        .await?
        .rows_affected();
    if n == 0 {
        return Err(AppError::InvalidInput("仅可编辑 free 状态文案".into()));
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn set_copy_items_enabled(
    state: State<'_, AppState>,
    ids: Vec<i64>,
    enabled: bool,
) -> AppResult<i64> {
    let mut changed = 0;
    for id in ids {
        changed += sqlx::query("UPDATE text_items SET enabled=?2 WHERE id=?1 AND state='free'")
            .bind(id)
            .bind(enabled as i64)
            .execute(&state.db)
            .await?
            .rows_affected();
    }
    Ok(changed as i64)
}

#[tauri::command]
#[specta::specta]
pub async fn delete_copy_items(state: State<'_, AppState>, ids: Vec<i64>) -> AppResult<i64> {
    let mut tx = state.db.begin().await?;
    let mut deleted = 0;
    for id in ids {
        sqlx::query(
            "UPDATE posts SET title_id=NULL WHERE title_id=?1 AND EXISTS(
               SELECT 1 FROM text_items WHERE id=?1 AND state='free'
             )",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE posts SET body_id=NULL WHERE body_id=?1 AND EXISTS(
               SELECT 1 FROM text_items WHERE id=?1 AND state='free'
             )",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?;
        deleted += sqlx::query("DELETE FROM text_items WHERE id=?1 AND state='free'")
            .bind(id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
    }
    tx.commit().await?;
    Ok(deleted as i64)
}

#[tauri::command]
#[specta::specta]
pub async fn list_topic_groups(
    state: State<'_, AppState>,
    product_id: Option<i64>,
) -> AppResult<Vec<TopicGroupView>> {
    Ok(copy_repo::list_topics(&state.db, product_id)
        .await?
        .into_iter()
        .map(|row| TopicGroupView {
            id: row.id,
            product_id: row.product_id,
            product_name: row.product_name,
            scope: row.scope,
            sku_ids: serde_json::from_str(&row.sku_ids_json).unwrap_or_default(),
            tags: serde_json::from_str(&row.tags_json).unwrap_or_default(),
            enabled: row.enabled != 0,
            created_at: row.created_at,
        })
        .collect())
}

fn validate_topic(input: &TopicGroupInput) -> AppResult<(String, String)> {
    if !matches!(input.scope.as_str(), "combo" | "product" | "general") {
        return Err(AppError::InvalidInput("话题组范围非法".into()));
    }
    if input.scope == "combo" && (input.product_id.is_none() || input.sku_ids.len() < 2) {
        return Err(AppError::InvalidInput(
            "组合专用话题必须指定商品和至少两个 SKU".into(),
        ));
    }
    if input.scope == "product" && input.product_id.is_none() {
        return Err(AppError::InvalidInput("商品专用话题必须指定商品".into()));
    }
    if input.scope == "general" && input.product_id.is_some() {
        return Err(AppError::InvalidInput("通用话题不能绑定商品".into()));
    }
    if input.scope != "combo" && !input.sku_ids.is_empty() {
        return Err(AppError::InvalidInput(
            "只有组合专用话题可以绑定 SKU".into(),
        ));
    }
    if input.sku_ids.iter().collect::<HashSet<_>>().len() != input.sku_ids.len() {
        return Err(AppError::InvalidInput("话题组的 SKU 不能重复".into()));
    }
    let mut tags: Vec<String> = input
        .tags
        .iter()
        .map(|tag| format!("#{}", tag.trim().trim_start_matches('#')))
        .filter(|tag| tag.len() > 1)
        .collect();
    tags.sort();
    tags.dedup();
    if tags.is_empty() {
        return Err(AppError::InvalidInput("话题组至少要有一个标签".into()));
    }
    Ok((
        serde_json::to_string(&input.sku_ids)?,
        serde_json::to_string(&tags)?,
    ))
}

#[tauri::command]
#[specta::specta]
pub async fn save_topic_group(
    state: State<'_, AppState>,
    id: Option<i64>,
    input: TopicGroupInput,
) -> AppResult<i64> {
    let (sku_ids, tags) = validate_topic(&input)?;
    if let Some(product_id) = input.product_id {
        let active: Option<i64> =
            sqlx::query_scalar("SELECT id FROM products WHERE id=?1 AND status='active'")
                .bind(product_id)
                .fetch_optional(&state.db)
                .await?;
        if active.is_none() {
            return Err(AppError::InvalidInput("商品不存在或已停用".into()));
        }
        if input.scope == "combo" {
            let valid_skus: HashSet<i64> = sqlx::query_scalar(
                "SELECT id FROM skus WHERE product_id=?1 AND is_general=0 AND status='active'",
            )
            .bind(product_id)
            .fetch_all(&state.db)
            .await?
            .into_iter()
            .collect();
            if input.sku_ids.iter().any(|id| !valid_skus.contains(id)) {
                return Err(AppError::InvalidInput(
                    "组合话题包含不属于当前商品或已停用的 SKU".into(),
                ));
            }
        }
    }
    let now = crate::db::now_unix();
    if let Some(id) = id {
        sqlx::query(
            "UPDATE topic_groups SET product_id=?2,scope=?3,sku_ids_json=?4,tags_json=?5,updated_at=?6 WHERE id=?1",
        )
        .bind(id)
        .bind(input.product_id)
        .bind(&input.scope)
        .bind(sku_ids)
        .bind(tags)
        .bind(now)
        .execute(&state.db)
        .await?;
        Ok(id)
    } else {
        Ok(sqlx::query_scalar(
            "INSERT INTO topic_groups(product_id,scope,sku_ids_json,tags_json,created_at,updated_at)
             VALUES(?1,?2,?3,?4,?5,?5) RETURNING id",
        )
        .bind(input.product_id)
        .bind(&input.scope)
        .bind(sku_ids)
        .bind(tags)
        .bind(now)
        .fetch_one(&state.db)
        .await?)
    }
}

#[tauri::command]
#[specta::specta]
pub async fn set_topic_groups_enabled(
    state: State<'_, AppState>,
    ids: Vec<i64>,
    enabled: bool,
) -> AppResult<i64> {
    let mut changed = 0;
    for id in ids {
        changed += sqlx::query("UPDATE topic_groups SET enabled=?2,updated_at=?3 WHERE id=?1")
            .bind(id)
            .bind(enabled as i64)
            .bind(crate::db::now_unix())
            .execute(&state.db)
            .await?
            .rows_affected();
    }
    Ok(changed as i64)
}

#[tauri::command]
#[specta::specta]
pub async fn delete_topic_groups(state: State<'_, AppState>, ids: Vec<i64>) -> AppResult<i64> {
    let mut deleted = 0;
    for id in ids {
        deleted += sqlx::query("DELETE FROM topic_groups WHERE id=?1")
            .bind(id)
            .execute(&state.db)
            .await?
            .rows_affected();
    }
    Ok(deleted as i64)
}

#[tauri::command]
#[specta::specta]
pub async fn preview_copy_file(path: String) -> AppResult<CopyFilePreview> {
    let bytes = std::fs::read(&path)?;
    let text = String::from_utf8(bytes)
        .map_err(|_| AppError::InvalidInput("文案文件必须是 UTF-8".into()))?;
    let filename = Path::new(&path)
        .file_name()
        .and_then(|x| x.to_str())
        .unwrap_or("");
    let parsed = copy_ingest::parse(&text, filename).map_err(AppError::InvalidInput)?;
    Ok(CopyFilePreview {
        product_code: parsed.product_code,
        kind: parsed.kind,
        titles: parsed.titles.len() as i64,
        bodies: parsed.bodies.len() as i64,
        topics: parsed.topics,
    })
}

#[tauri::command]
#[specta::specta]
pub async fn import_copy_file(
    state: State<'_, AppState>,
    product_id: i64,
    path: String,
) -> AppResult<i64> {
    let bytes = std::fs::read(&path)?;
    let text = String::from_utf8(bytes)
        .map_err(|_| AppError::InvalidInput("文案文件必须是 UTF-8".into()))?;
    let filename = Path::new(&path)
        .file_name()
        .and_then(|x| x.to_str())
        .unwrap_or("");
    let parsed = copy_ingest::parse(&text, filename).map_err(AppError::InvalidInput)?;
    let mut tx = state.db.begin().await?;
    let mut count = 0;
    for title in parsed.titles {
        copy_repo::insert_copy(&mut tx, product_id, "title", &title, "manual").await?;
        count += 1;
    }
    for body in parsed.bodies {
        copy_repo::insert_copy(&mut tx, product_id, "body", &body, "manual").await?;
        count += 1;
    }
    tx.commit().await?;
    Ok(count)
}
