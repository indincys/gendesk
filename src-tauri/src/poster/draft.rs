//! 已通过作品的图文交付草稿。排除与排序只修改 Poster 批次成员。

use std::{collections::HashSet, path::Path};

use serde::{Deserialize, Serialize};
use specta::Type;
use sqlx::{FromRow, SqlitePool};

use crate::{
    error::{AppError, AppResult},
    files::copy::copy_batch_new,
};

use super::{pending_in_set, set_scope};

#[derive(Debug, Clone, Serialize, FromRow, Type)]
#[serde(rename_all = "camelCase")]
pub struct DraftImage {
    pub uid: String,
    pub item_name: String,
    pub task_id: Option<i64>,
    pub image_path: String,
    pub thumb_path: Option<String>,
    pub style: String,
    pub product_name: Option<String>,
    pub wants_live: bool,
    pub live_image: Option<String>,
    pub live_video: Option<String>,
    pub excluded: bool,
}

#[derive(Debug, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryDraft {
    pub id: String,
    pub state: String,
    pub revision: i64,
    pub editable: bool,
    pub waiting_review: i64,
    pub waiting_intake: bool,
    pub material_id: Option<String>,
    pub last_error: String,
    pub images: Vec<DraftImage>,
}

#[derive(FromRow)]
struct DraftState {
    state: String,
    set_key: Option<String>,
    frozen: bool,
    draft_revision: i64,
    material_id: Option<String>,
    last_error: String,
}

pub async fn read(pool: &SqlitePool, id: &str) -> AppResult<DeliveryDraft> {
    let row: DraftState = sqlx::query_as(
        "SELECT state,set_key,request_json IS NOT NULL AS frozen,draft_revision,material_id,last_error FROM poster_batches WHERE id=?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| AppError::InvalidInput("Poster 批次不存在".into()))?;
    let (waiting_review, waiting_intake) = if let Some(key) = &row.set_key {
        let scope = set_scope(key)?;
        if row.frozen {
            (0, false)
        } else {
            (
                pending_in_set(pool, key, &scope).await?,
                scope.waiting_intake(pool).await?,
            )
        }
    } else {
        (0, false)
    };
    let images = sqlx::query_as(
        "SELECT i.uid,i.item_name,i.task_id,COALESCE(w.image_path,i.image_path) AS image_path,w.thumb_path,
         i.style,i.product_name,i.wants_live,i.live_image,i.live_video,i.excluded
         FROM poster_images i LEFT JOIN accepted_works w ON w.id=i.work_id AND w.uid=i.uid
         WHERE i.batch_id=? ORDER BY i.draft_position IS NULL,i.draft_position,i.task_id,i.work_id",
    )
    .bind(id)
    .fetch_all(pool)
    .await?;
    Ok(DeliveryDraft {
        id: id.into(),
        editable: row.set_key.is_some()
            && !row.frozen
            && matches!(row.state.as_str(), "awaiting" | "queued" | "failed"),
        state: row.state,
        revision: row.draft_revision,
        waiting_review,
        waiting_intake,
        material_id: row.material_id,
        last_error: row.last_error,
        images,
    })
}

#[derive(Debug, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DraftPlacement {
    pub uid: String,
    pub excluded: bool,
}

/// 调用方按 review_lock → poster_lock 顺序持锁，兼容刚验收加入新成员和冻结交付的并发。
pub async fn save(
    pool: &SqlitePool,
    id: &str,
    revision: i64,
    placements: &[DraftPlacement],
    confirm: bool,
) -> AppResult<()> {
    let current = read(pool, id).await?;
    if !current.editable || current.revision != revision {
        return Err(AppError::InvalidInput(
            "图文已更新或开始交付，请刷新草稿".into(),
        ));
    }
    let displayed: HashSet<&str> = placements.iter().map(|item| item.uid.as_str()).collect();
    let known: HashSet<&str> = current
        .images
        .iter()
        .map(|item| item.uid.as_str())
        .collect();
    if displayed.len() != placements.len() || displayed != known {
        return Err(AppError::InvalidInput(
            "草稿里的图片已更新，请重新载入再保存".into(),
        ));
    }
    if confirm {
        if !super::load_settings(pool).await?.auto_import {
            return Err(AppError::InvalidInput("请先开启 Poster 交付".into()));
        }
        if current.waiting_review > 0 || current.waiting_intake {
            return Err(AppError::InvalidInput(
                "等本组收齐并完成验收后再确认交付".into(),
            ));
        }
        if placements.iter().all(|item| item.excluded) {
            return Err(AppError::InvalidInput("至少保留一张图片才可交付".into()));
        }
        for placement in placements.iter().filter(|item| !item.excluded) {
            let image = current
                .images
                .iter()
                .find(|image| image.uid == placement.uid);
            if image.is_some_and(|item| {
                item.wants_live
                    && !(file_present(&item.live_image) && file_present(&item.live_video))
            }) {
                return Err(AppError::InvalidInput(
                    "至少一张实况图的 JPG 或 MOV 未就绪".into(),
                ));
            }
        }
    }
    let mut tx = pool.begin().await?;
    let updated = sqlx::query(
        "UPDATE poster_batches SET draft_revision=draft_revision+1,state=?,draft_confirmed=?,last_error=''
         WHERE id=? AND draft_revision=? AND request_json IS NULL AND state IN ('awaiting','queued','failed')",
    )
    .bind(if confirm { "queued" } else { "awaiting" })
    .bind(confirm)
    .bind(id)
    .bind(revision)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::InvalidInput("图文状态已变化，请刷新草稿".into()));
    }
    for (index, item) in placements.iter().enumerate() {
        sqlx::query(
            "UPDATE poster_images SET draft_position=?,excluded=? WHERE batch_id=? AND uid=?",
        )
        .bind(index as i64)
        .bind(item.excluded)
        .bind(id)
        .bind(&item.uid)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

fn file_present(path: &Option<String>) -> bool {
    path.as_deref().is_some_and(|p| Path::new(p).is_file())
}

#[derive(Debug, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DraftExport {
    pub directory: String,
    pub exported: i64,
    pub failures: Vec<String>,
}

pub fn export(draft: DeliveryDraft, directory: &Path, static_only: bool) -> AppResult<DraftExport> {
    if !directory.is_absolute() || !directory.is_dir() {
        return Err(AppError::InvalidInput("导出文件夹不存在或无法访问".into()));
    }
    if !draft.images.iter().any(|item| !item.excluded) {
        return Err(AppError::InvalidInput("没有可导出的交付图片".into()));
    }
    let output = tempfile::Builder::new()
        .prefix("GenDesk-图文-")
        .tempdir_in(directory)?;
    let mut entries = Vec::new();
    let mut failures = Vec::new();
    for (index, item) in draft
        .images
        .iter()
        .filter(|item| !item.excluded)
        .enumerate()
    {
        let position = index + 1;
        let jobs = if item.wants_live && !static_only {
            match (item.live_image.as_deref(), item.live_video.as_deref()) {
                (Some(jpg), Some(mov)) => vec![
                    (
                        Path::new(jpg).to_path_buf(),
                        output.path().join(format!("{position:02}.jpg")),
                    ),
                    (
                        Path::new(mov).to_path_buf(),
                        output.path().join(format!("{position:02}.mov")),
                    ),
                ],
                _ => {
                    failures.push(format!(
                        "{position:02} · {} · 缺少实况 JPG 或 MOV",
                        item.item_name
                    ));
                    continue;
                }
            }
        } else {
            let source = Path::new(&item.image_path);
            let ext = crate::files::output_ext_from_path(&item.image_path);
            vec![(
                source.to_path_buf(),
                output.path().join(format!("{position:02}.{ext}")),
            )]
        };
        if let Err(error) = copy_batch_new(&jobs) {
            failures.push(format!("{position:02} · {} · {error}", item.item_name));
            continue;
        }
        let files = jobs
            .iter()
            .filter_map(|(_, dest)| {
                dest.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .collect::<Vec<_>>();
        entries.push(
            serde_json::json!({"position": position, "cover": position == 1,
            "name": item.item_name, "files": files}),
        );
    }
    let index = serde_json::json!({"batchId": draft.id, "files": entries, "failures": failures});
    let manifest = output.path().join("交付清单.json");
    std::fs::write(manifest, serde_json::to_vec_pretty(&index)?)?;
    Ok(DraftExport {
        directory: output.keep().to_string_lossy().into_owned(),
        exported: entries.len() as i64,
        failures,
    })
}
