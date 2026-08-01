//! 任务包物化：先准备完整目录，再记账，最后写 READY.txt。

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::NaiveDateTime;
use serde::Serialize;
use sqlx::{FromRow, SqlitePool};

use crate::db::now_unix;
use crate::error::{AppError, AppResult};
use crate::publish::paths::{self, RelPath};
use crate::publish::platform::Platform;
use crate::publish::sheet_json::{
    self, DouyinOptions, ExportTask, ProductRef, ShipinhaoOptions, TaskSheetJson, XhsOptions,
};
use crate::publish::validate;

static EXPORT_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ExportResult {
    pub directory: String,
    pub task_count: i64,
}

#[derive(Debug, Clone, FromRow)]
struct SheetMeta {
    id: i64,
    date: String,
    title: String,
    status: String,
    export_dir: Option<String>,
    product_code: String,
    product_name: String,
    cart_enabled: i64,
    douyin_product_url: String,
    douyin_short_title: String,
}

#[derive(Debug, Clone, FromRow)]
struct PostMeta {
    id: i64,
    content_code: String,
    title_text: Option<String>,
    body_text: Option<String>,
    topics_json: String,
}

#[derive(Debug, Clone, FromRow)]
struct TaskMeta {
    task_code: String,
    platform: String,
    scheduled_at: String,
}

#[derive(Debug, Clone, FromRow)]
struct ImageMeta {
    path_rel: String,
    sku_code: String,
    music_keyword: String,
    ord: i64,
}

struct PartialDirGuard<'a> {
    path: &'a Path,
    root: &'a Path,
}

impl Drop for PartialDirGuard<'_> {
    fn drop(&mut self) {
        let _ = remove_known_dir(self.path, self.root);
    }
}

fn remove_known_dir(path: &Path, root: &Path) -> AppResult<()> {
    if path.starts_with(root) && path != root && path.exists() {
        std::fs::remove_dir_all(path)?;
    }
    Ok(())
}

fn guide(sheet: &SheetMeta, tasks: &[ExportTask]) -> String {
    format!(
        "# {}\n\n商品：{}（{}）\n\n任务数：{}\n\nRPA 仅在 READY.txt 出现后读取任务单.json，并把执行结果逐行追加到回执.jsonl。\n",
        sheet.title,
        sheet.product_name,
        sheet.product_code,
        tasks.len()
    )
}

async fn claim_export(pool: &SqlitePool, sheet_id: i64) -> AppResult<String> {
    let token = format!(
        "{}-{}-{}",
        std::process::id(),
        now_unix(),
        EXPORT_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    let claimed = sqlx::query(
        "UPDATE task_sheets SET export_token=?2,updated_at=?3
         WHERE id=?1 AND status='confirmed' AND export_token IS NULL",
    )
    .bind(sheet_id)
    .bind(&token)
    .bind(now_unix())
    .execute(pool)
    .await?
    .rows_affected();
    if claimed == 1 {
        return Ok(token);
    }
    let state: Option<(String, Option<String>)> =
        sqlx::query_as("SELECT status,export_token FROM task_sheets WHERE id=?1")
            .bind(sheet_id)
            .fetch_optional(pool)
            .await?;
    Err(AppError::InvalidInput(match state {
        Some((status, Some(_))) if status == "confirmed" => "任务单正在导出，请勿重复操作".into(),
        Some((status, _)) if status == "exported" => {
            "任务单已导出且 READY 包处于活动状态，拒绝重导出；请先收回结果".into()
        }
        Some((status, _)) if status == "reconciling" || status == "closed" => {
            "任务单已进入回执或关闭阶段，不能导出".into()
        }
        Some(_) => "任务单必须先确认才能导出".into(),
        None => "任务单不存在".into(),
    }))
}

pub async fn export(
    pool: &SqlitePool,
    root: &Path,
    exec_root: &str,
    path_style: paths::PathStyle,
    sheet_id: i64,
    now: NaiveDateTime,
) -> AppResult<ExportResult> {
    let token = claim_export(pool, sheet_id).await?;
    let result = export_claimed(pool, root, exec_root, path_style, sheet_id, now, &token).await;
    if result.is_err() {
        let _ = sqlx::query(
            "UPDATE task_sheets SET export_token=NULL
             WHERE id=?1 AND export_token=?2",
        )
        .bind(sheet_id)
        .bind(&token)
        .execute(pool)
        .await;
    }
    result
}

#[allow(clippy::too_many_arguments)] // 导出上下文 + 原子占用 token
async fn export_claimed(
    pool: &SqlitePool,
    root: &Path,
    exec_root: &str,
    path_style: paths::PathStyle,
    sheet_id: i64,
    now: NaiveDateTime,
    export_token: &str,
) -> AppResult<ExportResult> {
    let sheet = sqlx::query_as::<_, SheetMeta>(
        "SELECT s.id,s.date,s.title,s.status,s.export_dir,p.code AS product_code,p.name AS product_name,
                p.cart_enabled,p.douyin_product_url,p.douyin_short_title
         FROM task_sheets s JOIN products p ON p.id=s.product_id WHERE s.id=?1",
    )
    .bind(sheet_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| AppError::InvalidInput("任务单不存在".into()))?;
    if sheet.status != "confirmed" {
        return Err(AppError::InvalidInput("任务单必须先确认才能导出".into()));
    }
    let packages_root = RelPath::new(paths::TASK_PACKAGES).to_local(root);
    std::fs::create_dir_all(&packages_root)?;
    let final_dir = packages_root.join(&sheet.title);
    let receipt = final_dir.join(paths::RECEIPT_JSONL);
    if receipt.metadata().is_ok_and(|meta| meta.len() > 0) {
        return Err(AppError::InvalidInput(
            "回执.jsonl 已有内容，拒绝覆盖；请先收回结果".into(),
        ));
    }
    if let Some(previous) = sheet.export_dir.as_deref() {
        let previous_receipt = Path::new(previous).join(paths::RECEIPT_JSONL);
        if previous_receipt.metadata().is_ok_and(|meta| meta.len() > 0) {
            return Err(AppError::InvalidInput(
                "上次导出的回执.jsonl 已有内容，拒绝重导出；请先收回结果".into(),
            ));
        }
    }
    let partial_dir = packages_root.join(format!(".{}-partial-{}", sheet.title, sheet.id));
    remove_known_dir(&partial_dir, &packages_root)?;
    std::fs::create_dir_all(&partial_dir)?;
    let _partial_guard = PartialDirGuard {
        path: &partial_dir,
        root: &packages_root,
    };

    let posts = sqlx::query_as::<_, PostMeta>(
        "SELECT id,content_code,title_text,body_text,topics_json FROM posts WHERE sheet_id=?1 ORDER BY seq,id",
    )
    .bind(sheet_id)
    .fetch_all(pool)
    .await?;
    let mut export_tasks = Vec::new();
    let mut missing_paths = HashSet::new();
    let mut copied_by_post: HashMap<i64, Vec<(String, ImageMeta)>> = HashMap::new();
    for post in &posts {
        let images = sqlx::query_as::<_, ImageMeta>(
            "SELECT a.path_rel,s.code AS sku_code,s.music_keyword,pi.ord
             FROM post_images pi JOIN image_assets a ON a.id=pi.asset_id JOIN skus s ON s.id=a.sku_id
             WHERE pi.post_id=?1 ORDER BY pi.ord",
        )
        .bind(post.id)
        .fetch_all(pool)
        .await?;
        let image_dir = partial_dir.join(paths::IMAGES_DIR).join(&post.content_code);
        std::fs::create_dir_all(&image_dir)?;
        let mut copied = Vec::new();
        for image in images {
            let source = RelPath::new(&image.path_rel).to_local(root);
            let ext = paths::ascii_ext(&image.path_rel);
            let filename = format!("{:02}.{ext}", image.ord + 1);
            let partial_dest = image_dir.join(&filename);
            let exec_rel = RelPath::from_parts([
                paths::TASK_PACKAGES,
                sheet.title.as_str(),
                paths::IMAGES_DIR,
                post.content_code.as_str(),
                filename.as_str(),
            ]);
            let final_string = paths::exec_join(exec_root, &exec_rel, path_style);
            if !source.is_file() {
                missing_paths.insert(final_string.clone());
            } else {
                std::fs::copy(source, partial_dest)?;
            }
            copied.push((final_string, image));
        }
        copied_by_post.insert(post.id, copied);
    }

    for post in &posts {
        let images = copied_by_post.get(&post.id).cloned().unwrap_or_default();
        let image_paths: Vec<String> = images.iter().map(|(path, _)| path.clone()).collect();
        let first = images.first();
        let task_rows = sqlx::query_as::<_, TaskMeta>(
            "SELECT task_code,platform,scheduled_at FROM publish_tasks WHERE post_id=?1 ORDER BY scheduled_at,platform",
        )
        .bind(post.id)
        .fetch_all(pool)
        .await?;
        for task in task_rows {
            let platform = Platform::from_code(&task.platform).ok_or_else(|| {
                AppError::InvalidInput(format!("任务 {} 平台非法", task.task_code))
            })?;
            let is_douyin = platform == Platform::Douyin;
            let is_xhs = platform == Platform::Xhs;
            let is_shipinhao = platform == Platform::Shipinhao;
            let topics = serde_json::from_str(&post.topics_json).unwrap_or_default();
            let music = first
                .map(|(_, image)| image.music_keyword.trim().to_string())
                .filter(|value| !value.is_empty());
            export_tasks.push(ExportTask {
                task_id: task.task_code,
                content_id: post.content_code.clone(),
                platform: platform.zh().to_string(),
                mode: "图文".into(),
                scheduled_at: task.scheduled_at,
                title: if platform == Platform::Kuaishou {
                    None
                } else {
                    post.title_text.clone()
                },
                description: post.body_text.clone().unwrap_or_default(),
                topics,
                image_paths: image_paths.clone(),
                cover_path: is_douyin.then(|| image_paths.first().cloned()).flatten(),
                music_keyword: (is_douyin || is_shipinhao).then_some(music).flatten(),
                cart: is_douyin && sheet.cart_enabled != 0,
                douyin: is_douyin.then(|| DouyinOptions {
                    product_url: sheet.douyin_product_url.clone(),
                    short_title: sheet.douyin_short_title.clone(),
                    visibility: "公开".into(),
                    allow_save: false,
                }),
                xhs: is_xhs.then_some(XhsOptions {
                    original: true,
                    allow_co_create: false,
                    allow_copy: false,
                }),
                shipinhao: is_shipinhao.then_some(ShipinhaoOptions {
                    location: "不显示位置".into(),
                }),
                note: format!("商品{} · SKU {}", sheet.product_code, {
                    let mut codes = images
                        .iter()
                        .map(|(_, image)| image.sku_code.clone())
                        .collect::<HashSet<_>>()
                        .into_iter()
                        .collect::<Vec<_>>();
                    codes.sort();
                    codes.join(", ")
                }),
            });
        }
    }
    if let Err(errors) = validate::validate_tasks(&export_tasks, now, &missing_paths) {
        remove_known_dir(&partial_dir, &packages_root)?;
        return Err(AppError::InvalidInput(errors.join("\n")));
    }
    let json = TaskSheetJson {
        schema: "gendesk.tasksheet/1".into(),
        sheet_id: format!("{}-{}", sheet.product_code, sheet.date.replace('-', "")),
        product: ProductRef {
            code: sheet.product_code.clone(),
            name: sheet.product_name.clone(),
        },
        generated_at: now.format("%Y-%m-%d %H:%M").to_string(),
        tasks: export_tasks.clone(),
    };
    std::fs::write(
        partial_dir.join(paths::TASK_JSON),
        sheet_json::to_pretty_json(&json)?,
    )?;
    std::fs::write(
        partial_dir.join(paths::EXEC_GUIDE),
        guide(&sheet, &export_tasks),
    )?;
    std::fs::write(partial_dir.join(paths::RECEIPT_JSONL), b"")?;
    remove_known_dir(&final_dir, &packages_root)?;
    std::fs::rename(&partial_dir, &final_dir)?;

    let mut tx = pool.begin().await?;
    let advanced = sqlx::query(
        "UPDATE task_sheets SET status='exported',export_dir=?2,exported_at=?3,
                updated_at=?3
         WHERE id=?1 AND status='confirmed' AND export_token=?4",
    )
    .bind(sheet_id)
    .bind(final_dir.to_string_lossy().to_string())
    .bind(now_unix())
    .bind(export_token)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if advanced != 1 {
        return Err(AppError::InvalidInput(
            "导出占用已失效，拒绝发布不确定的任务包".into(),
        ));
    }
    sqlx::query(
        "UPDATE image_assets SET state='used',updated_at=?2
         WHERE state='held' AND post_id IN (SELECT id FROM posts WHERE sheet_id=?1)",
    )
    .bind(sheet_id)
    .bind(now_unix())
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE text_items SET state='used'
         WHERE state='held' AND post_id IN (SELECT id FROM posts WHERE sheet_id=?1)",
    )
    .bind(sheet_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    if let Err(err) = std::fs::write(final_dir.join(paths::READY), b"ready\n") {
        let mut rollback = pool.begin().await?;
        let reverted = sqlx::query(
            "UPDATE task_sheets
             SET status='confirmed',export_dir=NULL,exported_at=NULL,updated_at=?2,export_token=NULL
             WHERE id=?1 AND status='exported' AND export_token=?3",
        )
        .bind(sheet_id)
        .bind(now_unix())
        .bind(export_token)
        .execute(&mut *rollback)
        .await?
        .rows_affected();
        if reverted == 1 {
            sqlx::query("UPDATE image_assets SET state='held' WHERE state='used' AND post_id IN (SELECT id FROM posts WHERE sheet_id=?1)")
                .bind(sheet_id)
                .execute(&mut *rollback)
                .await?;
            sqlx::query("UPDATE text_items SET state='held' WHERE state='used' AND post_id IN (SELECT id FROM posts WHERE sheet_id=?1)")
                .bind(sheet_id)
                .execute(&mut *rollback)
                .await?;
        }
        rollback.commit().await?;
        return Err(AppError::Io(format!(
            "READY.txt 写入失败，已回滚导出状态：{err}"
        )));
    }

    // READY 是 RPA 可见性的唯一开关。它成功落盘前 token 始终保持占用，阻止退回、
    // 恢复和收回回执；落盘后再按 token 所有权释放，消除“已导出但 READY 未出现”的窗口。
    let released = sqlx::query(
        "UPDATE task_sheets SET export_token=NULL,updated_at=?3
         WHERE id=?1 AND status='exported' AND export_token=?2",
    )
    .bind(sheet_id)
    .bind(export_token)
    .bind(now_unix())
    .execute(pool)
    .await?
    .rows_affected();
    if released != 1 {
        return Err(AppError::InvalidInput(
            "任务包已生成，但导出占用释放失败；请刷新状态后再操作".into(),
        ));
    }
    Ok(ExportResult {
        directory: final_dir.to_string_lossy().to_string(),
        task_count: export_tasks.len() as i64,
    })
}
