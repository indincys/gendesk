//! 任务单配置、组稿、全局排期与确认页编辑命令。

use std::collections::{HashMap, HashSet};

use chrono::{Local, NaiveDate, NaiveDateTime};
use serde::{Deserialize, Serialize};
use specta::Type;
use sqlx::{FromRow, SqlitePool};
use tauri::State;

use crate::commands::publish_settings;
use crate::db::repo::sheets as sheet_repo;
use crate::error::{AppError, AppResult};
use crate::publish::composer::{self, ComposeInput, SkuPool, TextPool, TopicCandidate};
use crate::publish::paths;
use crate::publish::platform::Platform;
use crate::publish::product;
use crate::publish::schedule::{self, FixedSlot, SchedulePost};
use crate::publish::{exporter, settle};
use crate::state::AppState;

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SheetConfigView {
    pub id: i64,
    pub product_id: i64,
    pub product_code: String,
    pub product_name: String,
    pub name: String,
    pub sku_scope: Vec<i64>,
    pub platforms: Vec<String>,
    pub posts_per_day: i64,
    pub images_per_post: i64,
    pub mixed_count: i64,
    pub anchors: Vec<String>,
    pub jitter_min: i64,
    pub min_gap_min: i64,
    pub target_day: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SheetConfigInput {
    pub product_id: i64,
    pub name: String,
    pub sku_scope: Vec<i64>,
    pub platforms: Vec<String>,
    pub posts_per_day: i64,
    pub images_per_post: i64,
    pub mixed_count: i64,
    pub anchors: Vec<String>,
    pub jitter_min: i64,
    pub min_gap_min: i64,
    pub target_day: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SheetSummaryView {
    pub id: i64,
    pub date: String,
    pub product_id: i64,
    pub product_code: String,
    pub product_name: String,
    pub title: String,
    pub status: String,
    pub post_count: i64,
    pub shortages: Vec<composer::Shortage>,
    pub export_dir: Option<String>,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PostImageView {
    pub asset_id: i64,
    pub sku_id: i64,
    pub sku_code: String,
    pub ord: i64,
    pub path: String,
    pub thumb: String,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PublishTaskView {
    pub id: i64,
    pub task_code: String,
    pub platform: String,
    pub platform_zh: String,
    pub scheduled_at: String,
    pub status: String,
    pub fail_kind: Option<String>,
    pub result_msg: Option<String>,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PostView {
    pub id: i64,
    pub content_code: String,
    pub seq: i64,
    pub kind: String,
    pub title_id: Option<i64>,
    pub body_id: Option<i64>,
    pub title: String,
    pub body: String,
    pub topics: Vec<String>,
    pub edited: bool,
    pub images: Vec<PostImageView>,
    pub tasks: Vec<PublishTaskView>,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SheetDetailView {
    pub summary: SheetSummaryView,
    pub posts: Vec<PostView>,
}

#[derive(Debug, Clone, FromRow)]
struct ConfigJoinRow {
    id: i64,
    product_id: i64,
    product_code: String,
    product_name: String,
    name: String,
    sku_scope_json: String,
    platforms_json: String,
    posts_per_day: i64,
    images_per_post: i64,
    mixed_count: i64,
    anchors_json: String,
    jitter_min: i64,
    min_gap_min: i64,
    target_day: String,
    enabled: i64,
}

#[derive(Debug, Clone, FromRow)]
struct ProductForCompose {
    id: i64,
    code: String,
}

#[derive(Debug, Clone, FromRow)]
struct RegenerateRow {
    id: i64,
    product_id: i64,
    sku_scope_json: String,
    posts_per_day: i64,
    images_per_post: i64,
    mixed_count: i64,
    target_day: String,
    enabled: i64,
    date: String,
    product_code: String,
}

#[derive(Debug, Clone, FromRow)]
struct ScheduleRow {
    post_id: i64,
    seq: i64,
    date: String,
    anchors_json: String,
    jitter_min: i64,
    min_gap_min: i64,
    config_platforms: String,
    product_platforms: String,
    product_code: String,
}

fn parse_vec<T: serde::de::DeserializeOwned>(json: &str) -> Vec<T> {
    serde_json::from_str(json).unwrap_or_default()
}

fn validate_config(input: &SheetConfigInput) -> AppResult<()> {
    if input.name.trim().is_empty() {
        return Err(AppError::InvalidInput("配置名不能为空".into()));
    }
    if input.posts_per_day <= 0
        || input.images_per_post <= 0
        || input.mixed_count < 0
        || input.mixed_count > input.posts_per_day
    {
        return Err(AppError::InvalidInput("篇数、图片数或混合篇数非法".into()));
    }
    if input.anchors.is_empty()
        || input
            .anchors
            .iter()
            .any(|value| schedule::parse_hhmm(value).is_none())
    {
        return Err(AppError::InvalidInput(
            "至少配置一个 HH:MM 发布时间锚点".into(),
        ));
    }
    let unique_anchors: HashSet<&str> = input.anchors.iter().map(String::as_str).collect();
    if unique_anchors.len() != input.anchors.len() {
        return Err(AppError::InvalidInput("发布时间锚点不能重复".into()));
    }
    if input.jitter_min < 0 || input.min_gap_min < 1 {
        return Err(AppError::InvalidInput(
            "抖动不能为负，同平台间隔至少 1 分钟".into(),
        ));
    }
    if !matches!(input.target_day.as_str(), "same" | "next") {
        return Err(AppError::InvalidInput("目标日必须是 same 或 next".into()));
    }
    if !input.platforms.is_empty() {
        product::validate_platforms(&input.platforms)?;
    }
    if input.sku_scope.iter().collect::<HashSet<_>>().len() != input.sku_scope.len() {
        return Err(AppError::InvalidInput("SKU 范围不能重复".into()));
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn list_sheet_configs(state: State<'_, AppState>) -> AppResult<Vec<SheetConfigView>> {
    let rows = sqlx::query_as::<_, ConfigJoinRow>(
        "SELECT c.id,c.product_id,p.code AS product_code,p.name AS product_name,c.name,
                c.sku_scope_json,c.platforms_json,c.posts_per_day,c.images_per_post,c.mixed_count,
                c.anchors_json,c.jitter_min,c.min_gap_min,c.target_day,c.enabled
         FROM sheet_configs c JOIN products p ON p.id=c.product_id ORDER BY p.code,c.id",
    )
    .fetch_all(&state.db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| SheetConfigView {
            id: row.id,
            product_id: row.product_id,
            product_code: row.product_code,
            product_name: row.product_name,
            name: row.name,
            sku_scope: parse_vec(&row.sku_scope_json),
            platforms: parse_vec(&row.platforms_json),
            posts_per_day: row.posts_per_day,
            images_per_post: row.images_per_post,
            mixed_count: row.mixed_count,
            anchors: parse_vec(&row.anchors_json),
            jitter_min: row.jitter_min,
            min_gap_min: row.min_gap_min,
            target_day: row.target_day,
            enabled: row.enabled != 0,
        })
        .collect())
}

#[tauri::command]
#[specta::specta]
pub async fn save_sheet_config(
    state: State<'_, AppState>,
    id: Option<i64>,
    input: SheetConfigInput,
) -> AppResult<i64> {
    validate_config(&input)?;
    let product_platforms_json: Option<String> =
        sqlx::query_scalar("SELECT platforms_json FROM products WHERE id=?1 AND status='active'")
            .bind(input.product_id)
            .fetch_optional(&state.db)
            .await?;
    let Some(product_platforms_json) = product_platforms_json else {
        return Err(AppError::InvalidInput("商品不存在或已停用".into()));
    };
    let product_platforms: HashSet<String> =
        parse_vec(&product_platforms_json).into_iter().collect();
    if input
        .platforms
        .iter()
        .any(|platform| !product_platforms.contains(platform))
    {
        return Err(AppError::InvalidInput(
            "任务单平台必须属于商品已启用的平台".into(),
        ));
    }
    let product_skus: HashSet<i64> = sqlx::query_scalar(
        "SELECT id FROM skus WHERE product_id=?1 AND is_general=0 AND status='active'",
    )
    .bind(input.product_id)
    .fetch_all(&state.db)
    .await?
    .into_iter()
    .collect();
    if input.sku_scope.iter().any(|id| !product_skus.contains(id)) {
        return Err(AppError::InvalidInput(
            "SKU 范围包含不属于当前商品或已停用的 SKU".into(),
        ));
    }
    let conflict: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sheet_configs WHERE product_id=?1 AND id!=COALESCE(?2,-1)",
    )
    .bind(input.product_id)
    .bind(id)
    .fetch_one(&state.db)
    .await?;
    if conflict > 0 {
        return Err(AppError::InvalidInput(
            "一个商品只能有一份任务单配置".into(),
        ));
    }
    let now = crate::db::now_unix();
    let sku_scope = serde_json::to_string(&input.sku_scope)?;
    let platforms = serde_json::to_string(&input.platforms)?;
    let anchors = serde_json::to_string(&input.anchors)?;
    if let Some(id) = id {
        sqlx::query(
            "UPDATE sheet_configs SET product_id=?2,name=?3,sku_scope_json=?4,platforms_json=?5,
             posts_per_day=?6,images_per_post=?7,mixed_count=?8,anchors_json=?9,jitter_min=?10,
             min_gap_min=?11,target_day=?12,enabled=?13,updated_at=?14 WHERE id=?1",
        )
        .bind(id)
        .bind(input.product_id)
        .bind(input.name.trim())
        .bind(sku_scope)
        .bind(platforms)
        .bind(input.posts_per_day)
        .bind(input.images_per_post)
        .bind(input.mixed_count)
        .bind(anchors)
        .bind(input.jitter_min)
        .bind(input.min_gap_min)
        .bind(&input.target_day)
        .bind(input.enabled as i64)
        .bind(now)
        .execute(&state.db)
        .await?;
        Ok(id)
    } else {
        Ok(sqlx::query_scalar(
            "INSERT INTO sheet_configs(product_id,name,sku_scope_json,platforms_json,posts_per_day,
             images_per_post,mixed_count,anchors_json,jitter_min,min_gap_min,target_day,enabled,created_at,updated_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?13) RETURNING id",
        )
        .bind(input.product_id)
        .bind(input.name.trim())
        .bind(sku_scope)
        .bind(platforms)
        .bind(input.posts_per_day)
        .bind(input.images_per_post)
        .bind(input.mixed_count)
        .bind(anchors)
        .bind(input.jitter_min)
        .bind(input.min_gap_min)
        .bind(&input.target_day)
        .bind(input.enabled as i64)
        .bind(now)
        .fetch_one(&state.db)
        .await?)
    }
}

fn date_code(date: &str) -> AppResult<String> {
    let parsed = NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .map_err(|_| AppError::InvalidInput("日期格式必须是 YYYY-MM-DD".into()))?;
    Ok(parsed.format("%y%m%d").to_string())
}

async fn compose_one(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    config: &sheet_repo::ConfigRow,
    product: &ProductForCompose,
    date: &str,
) -> AppResult<i64> {
    let existing: Option<(i64, String)> =
        sqlx::query_as("SELECT id,status FROM task_sheets WHERE date=?1 AND product_id=?2")
            .bind(date)
            .bind(product.id)
            .fetch_optional(&mut **tx)
            .await?;
    let now = crate::db::now_unix();
    let title = format!("{}商品-任务单-{date}", product.code);
    let sheet_id = match existing {
        Some((id, status)) => {
            if status != "draft" {
                return Err(AppError::InvalidInput(format!(
                    "{title} 已不是草稿，不能重新生成"
                )));
            }
            sqlx::query(
                "UPDATE image_assets SET state='free',post_id=NULL,updated_at=?2
                 WHERE state='held' AND post_id IN (SELECT id FROM posts WHERE sheet_id=?1 AND edited=0)",
            )
            .bind(id)
            .bind(now)
            .execute(&mut **tx)
            .await?;
            sqlx::query(
                "UPDATE text_items SET state='free',post_id=NULL
                 WHERE state='held' AND post_id IN (SELECT id FROM posts WHERE sheet_id=?1 AND edited=0)",
            )
            .bind(id)
            .execute(&mut **tx)
            .await?;
            sqlx::query("DELETE FROM publish_tasks WHERE post_id IN (SELECT id FROM posts WHERE sheet_id=?1 AND edited=0)")
                .bind(id)
                .execute(&mut **tx)
                .await?;
            sqlx::query("DELETE FROM posts WHERE sheet_id=?1 AND edited=0")
                .bind(id)
                .execute(&mut **tx)
                .await?;
            id
        }
        None => {
            sqlx::query_scalar(
                "INSERT INTO task_sheets(date,product_id,config_id,title,created_at,updated_at)
                 VALUES(?1,?2,?3,?4,?5,?5) RETURNING id",
            )
            .bind(date)
            .bind(product.id)
            .bind(config.id)
            .bind(&title)
            .bind(now)
            .fetch_one(&mut **tx)
            .await?
        }
    };

    let preserved: Vec<(i64, String)> =
        sqlx::query_as("SELECT seq,kind FROM posts WHERE sheet_id=?1 AND edited=1 ORDER BY seq")
            .bind(sheet_id)
            .fetch_all(&mut **tx)
            .await?;
    let preserved_seqs: HashSet<i64> = preserved.iter().map(|(seq, _)| *seq).collect();
    let open_seqs: Vec<i64> = (0..config.posts_per_day)
        .filter(|seq| !preserved_seqs.contains(seq))
        .collect();
    let preserved_mixed = preserved.iter().filter(|(_, kind)| kind == "mixed").count();
    let sku_scope: Vec<i64> = parse_vec(&config.sku_scope_json);
    let image_rows: Vec<(i64, String, i64)> = sqlx::query_as(
        "SELECT s.id,s.tier,a.id FROM skus s JOIN image_assets a ON a.sku_id=s.id
         WHERE s.product_id=?1 AND s.status='active' AND a.state='free'
         ORDER BY CASE s.tier WHEN 'hot' THEN 0 WHEN 'warm' THEN 1 ELSE 2 END,s.id,a.created_at,a.id",
    )
    .bind(product.id)
    .fetch_all(&mut **tx)
    .await?;
    let mut sku_map: HashMap<i64, SkuPool> = HashMap::new();
    for (sku_id, tier, asset_id) in image_rows {
        if !sku_scope.is_empty() && !sku_scope.contains(&sku_id) {
            continue;
        }
        sku_map
            .entry(sku_id)
            .or_insert_with(|| SkuPool {
                sku_id,
                tier,
                image_ids: Vec::new(),
            })
            .image_ids
            .push(asset_id);
    }
    let titles: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM text_items WHERE product_id=?1 AND kind='title' AND enabled=1 AND state='free' ORDER BY created_at,id",
    )
    .bind(product.id)
    .fetch_all(&mut **tx)
    .await?;
    let bodies: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM text_items WHERE product_id=?1 AND kind='body' AND enabled=1 AND state='free' ORDER BY created_at,id",
    )
    .bind(product.id)
    .fetch_all(&mut **tx)
    .await?;
    let topic_rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT scope,sku_ids_json,tags_json FROM topic_groups
         WHERE enabled=1 AND (product_id=?1 OR scope='general')
         ORDER BY CASE scope WHEN 'combo' THEN 0 WHEN 'product' THEN 1 ELSE 2 END,id",
    )
    .bind(product.id)
    .fetch_all(&mut **tx)
    .await?;
    let output = composer::compose(&ComposeInput {
        posts_per_day: open_seqs.len(),
        images_per_post: config.images_per_post as usize,
        mixed_count: (config.mixed_count as usize).saturating_sub(preserved_mixed),
        skus: sku_map.into_values().collect(),
        texts: TextPool {
            title_ids: titles,
            body_ids: bodies,
        },
        topics: topic_rows
            .into_iter()
            .map(|(scope, skus, tags)| TopicCandidate {
                scope,
                sku_ids: parse_vec(&skus),
                tags: parse_vec(&tags),
            })
            .collect(),
    });
    let day_code = date_code(date)?;
    for composed in &output.posts {
        let seq = open_seqs[composed.seq];
        let content_code = format!("C{day_code}-{}-{:02}", product.code, seq + 1);
        let title_text: String = sqlx::query_scalar("SELECT text FROM text_items WHERE id=?1")
            .bind(composed.title_id)
            .fetch_one(&mut **tx)
            .await?;
        let body_text: String = sqlx::query_scalar("SELECT text FROM text_items WHERE id=?1")
            .bind(composed.body_id)
            .fetch_one(&mut **tx)
            .await?;
        let post_id: i64 = sqlx::query_scalar(
            "INSERT INTO posts(sheet_id,content_code,seq,kind,title_id,body_id,title_text,body_text,topics_json)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9) RETURNING id",
        )
        .bind(sheet_id)
        .bind(content_code)
        .bind(seq)
        .bind(&composed.kind)
        .bind(composed.title_id)
        .bind(composed.body_id)
        .bind(title_text)
        .bind(body_text)
        .bind(serde_json::to_string(&composed.topics)?)
        .fetch_one(&mut **tx)
        .await?;
        for (ord, asset_id) in composed.image_ids.iter().enumerate() {
            sqlx::query("INSERT INTO post_images(post_id,asset_id,ord) VALUES(?1,?2,?3)")
                .bind(post_id)
                .bind(asset_id)
                .bind(ord as i64)
                .execute(&mut **tx)
                .await?;
            let n = sqlx::query("UPDATE image_assets SET state='held',post_id=?2,updated_at=?3 WHERE id=?1 AND state='free'")
                .bind(asset_id)
                .bind(post_id)
                .bind(now)
                .execute(&mut **tx)
                .await?
                .rows_affected();
            if n != 1 {
                return Err(AppError::InvalidInput(
                    "图片素材被并发占用，请重新生成".into(),
                ));
            }
        }
        for text_id in [composed.title_id, composed.body_id] {
            let n = sqlx::query(
                "UPDATE text_items SET state='held',post_id=?2 WHERE id=?1 AND state='free'",
            )
            .bind(text_id)
            .bind(post_id)
            .execute(&mut **tx)
            .await?
            .rows_affected();
            if n != 1 {
                return Err(AppError::InvalidInput("文案被并发占用，请重新生成".into()));
            }
        }
    }
    sqlx::query("UPDATE task_sheets SET title=?2,shortage_json=?3,updated_at=?4 WHERE id=?1")
        .bind(sheet_id)
        .bind(title)
        .bind(serde_json::to_string(&output.shortages)?)
        .bind(now)
        .execute(&mut **tx)
        .await?;
    Ok(sheet_id)
}

async fn reschedule_draft_date(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    date: &str,
    now: NaiveDateTime,
) -> AppResult<()> {
    sqlx::query("DELETE FROM publish_tasks WHERE sheet_id IN (SELECT id FROM task_sheets WHERE date=?1 AND status='draft')")
        .bind(date)
        .execute(&mut **tx)
        .await?;
    let rows = sqlx::query_as::<_, ScheduleRow>(
        "SELECT po.id AS post_id,po.seq,s.date,c.anchors_json,c.jitter_min,c.min_gap_min,
                c.platforms_json AS config_platforms,p.platforms_json AS product_platforms,
                p.code AS product_code
         FROM posts po JOIN task_sheets s ON s.id=po.sheet_id
         JOIN sheet_configs c ON c.id=s.config_id JOIN products p ON p.id=s.product_id
         WHERE s.date=?1 AND s.status='draft' ORDER BY po.seq,p.code,po.id",
    )
    .bind(date)
    .fetch_all(&mut **tx)
    .await?;
    let mut schedule_inputs = Vec::new();
    let mut row_by_post = HashMap::new();
    for row in rows {
        let mut platforms: Vec<String> = parse_vec(&row.config_platforms);
        if platforms.is_empty() {
            platforms = parse_vec(&row.product_platforms);
        }
        let task_date = NaiveDate::parse_from_str(&row.date, "%Y-%m-%d")
            .map_err(|_| AppError::InvalidInput("任务单日期损坏".into()))?;
        schedule_inputs.push(SchedulePost {
            post_id: row.post_id,
            seq: row.seq as usize,
            date: task_date,
            anchors: parse_vec(&row.anchors_json),
            jitter_min: row.jitter_min,
            min_gap_min: row.min_gap_min,
            platforms,
        });
        row_by_post.insert(row.post_id, row);
    }
    let fixed_rows: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT t.platform,t.scheduled_at,c.min_gap_min
         FROM publish_tasks t JOIN task_sheets s ON s.id=t.sheet_id
         JOIN sheet_configs c ON c.id=s.config_id",
    )
    .fetch_all(&mut **tx)
    .await?;
    let fixed = fixed_rows
        .into_iter()
        .map(|(platform, at, min_gap_min)| {
            let scheduled_at = NaiveDateTime::parse_from_str(&at, "%Y-%m-%d %H:%M")
                .map_err(|_| AppError::InvalidInput("已确认任务单含损坏的排期时间".into()))?;
            Ok(FixedSlot {
                platform,
                scheduled_at,
                min_gap_min,
            })
        })
        .collect::<AppResult<Vec<_>>>()?;
    let scheduled = schedule::schedule_all_with_fixed(&schedule_inputs, now, &fixed)
        .map_err(AppError::InvalidInput)?;
    let day_code = date_code(date)?;
    for task in scheduled {
        let row = row_by_post
            .get(&task.post_id)
            .ok_or_else(|| AppError::InvalidInput("排期结果找不到原篇".into()))?;
        let platform = Platform::from_code(&task.platform)
            .ok_or_else(|| AppError::InvalidInput("排期结果含未知平台".into()))?;
        let task_code = format!(
            "T{day_code}-{}-{:02}-{}",
            row.product_code,
            row.seq + 1,
            platform.zh()
        );
        sqlx::query(
            "INSERT INTO publish_tasks(sheet_id,post_id,task_code,platform,scheduled_at,updated_at)
             VALUES((SELECT sheet_id FROM posts WHERE id=?1),?1,?2,?3,?4,?5)",
        )
        .bind(task.post_id)
        .bind(task_code)
        .bind(task.platform)
        .bind(task.scheduled_at.format("%Y-%m-%d %H:%M").to_string())
        .bind(crate::db::now_unix())
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn generate_for_date_filtered(
    pool: &SqlitePool,
    date: &str,
    now: NaiveDateTime,
    target_day: Option<&str>,
) -> AppResult<Vec<i64>> {
    NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .map_err(|_| AppError::InvalidInput("日期格式必须是 YYYY-MM-DD".into()))?;
    let configs = sheet_repo::list_configs(pool)
        .await?
        .into_iter()
        .filter(|config| {
            config.enabled != 0 && target_day.map_or(true, |target| config.target_day == target)
        })
        .collect::<Vec<_>>();
    if configs.is_empty() {
        return Err(AppError::InvalidInput("没有启用的任务单配置".into()));
    }
    let mut tx = pool.begin().await?;
    let mut sheet_ids = Vec::new();
    for config in &configs {
        let product = sqlx::query_as::<_, ProductForCompose>(
            "SELECT id,code FROM products WHERE id=?1 AND status='active'",
        )
        .bind(config.product_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(product) = product else { continue };
        let existing_status: Option<String> =
            sqlx::query_scalar("SELECT status FROM task_sheets WHERE date=?1 AND product_id=?2")
                .bind(date)
                .bind(product.id)
                .fetch_optional(&mut *tx)
                .await?;
        if existing_status
            .as_deref()
            .is_some_and(|status| status != "draft")
        {
            // 批量/定时生成是按商品容错的：已确认或已导出的商品只跳过，不能阻塞
            // 同日其他商品生成。显式单单重组仍由 regenerate_sheet 严格校验。
            continue;
        }
        sheet_ids.push(compose_one(&mut tx, config, &product, date).await?);
    }

    reschedule_draft_date(&mut tx, date, now).await?;
    tx.commit().await?;
    Ok(sheet_ids)
}

pub async fn generate_for_date(
    pool: &SqlitePool,
    date: &str,
    now: NaiveDateTime,
) -> AppResult<Vec<i64>> {
    generate_for_date_filtered(pool, date, now, None).await
}

pub async fn generate_for_target_day(
    pool: &SqlitePool,
    date: &str,
    now: NaiveDateTime,
    target_day: &str,
) -> AppResult<Vec<i64>> {
    generate_for_date_filtered(pool, date, now, Some(target_day)).await
}

#[tauri::command]
#[specta::specta]
pub async fn generate_sheets(state: State<'_, AppState>, date: String) -> AppResult<Vec<i64>> {
    let now = Local::now().naive_local();
    generate_for_date(&state.db, &date, now).await
}

async fn summary(pool: &SqlitePool, row: sheet_repo::SheetRow) -> AppResult<SheetSummaryView> {
    let (product_code, product_name): (String, String) =
        sqlx::query_as("SELECT code,name FROM products WHERE id=?1")
            .bind(row.product_id)
            .fetch_one(pool)
            .await?;
    let post_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM posts WHERE sheet_id=?1")
        .bind(row.id)
        .fetch_one(pool)
        .await?;
    Ok(SheetSummaryView {
        id: row.id,
        date: row.date,
        product_id: row.product_id,
        product_code,
        product_name,
        title: row.title,
        status: row.status,
        post_count,
        shortages: serde_json::from_str(&row.shortage_json).unwrap_or_default(),
        export_dir: row.export_dir,
    })
}

#[tauri::command]
#[specta::specta]
pub async fn list_task_sheets(state: State<'_, AppState>) -> AppResult<Vec<SheetSummaryView>> {
    let mut out = Vec::new();
    for row in sheet_repo::list_sheets(&state.db).await? {
        out.push(summary(&state.db, row).await?);
    }
    Ok(out)
}

#[tauri::command]
#[specta::specta]
pub async fn get_publish_badges(
    state: State<'_, AppState>,
) -> AppResult<crate::publish::events::PublishBadgesEvent> {
    let (unclaimed, pending_sheets, pending_reconcile): (i64, i64, i64) = sqlx::query_as(
        "SELECT
              (SELECT COUNT(*) FROM inbox_items WHERE state IN ('unclaimed','failed')),
              (SELECT COUNT(*) FROM task_sheets WHERE status IN ('draft','confirmed')),
              (SELECT COUNT(*) FROM task_sheets WHERE status IN ('exported','reconciling'))",
    )
    .fetch_one(&state.db)
    .await?;
    Ok(crate::publish::events::PublishBadgesEvent {
        unclaimed,
        pending_sheets,
        pending_reconcile,
    })
}

#[derive(Debug, Clone, FromRow)]
struct PostRow {
    id: i64,
    content_code: String,
    seq: i64,
    kind: String,
    title_id: Option<i64>,
    body_id: Option<i64>,
    title_text: Option<String>,
    body_text: Option<String>,
    topics_json: String,
    edited: i64,
}

type PublishTaskRow = (
    i64,
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
);

#[tauri::command]
#[specta::specta]
pub async fn get_task_sheet(state: State<'_, AppState>, id: i64) -> AppResult<SheetDetailView> {
    let sheet = sheet_repo::get_sheet(&state.db, id)
        .await?
        .ok_or_else(|| AppError::InvalidInput("任务单不存在".into()))?;
    let root = publish_settings::root_local(&state.db).await?;
    let summary = summary(&state.db, sheet).await?;
    let rows = sqlx::query_as::<_, PostRow>(
        "SELECT id,content_code,seq,kind,title_id,body_id,title_text,body_text,topics_json,edited
         FROM posts WHERE sheet_id=?1 ORDER BY seq,id",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    let mut posts = Vec::new();
    for row in rows {
        let image_rows: Vec<(i64, i64, String, i64, String, String)> = sqlx::query_as(
            "SELECT a.id,a.sku_id,s.code,pi.ord,a.path_rel,a.thumb_rel
             FROM post_images pi JOIN image_assets a ON a.id=pi.asset_id JOIN skus s ON s.id=a.sku_id
             WHERE pi.post_id=?1 ORDER BY pi.ord",
        )
        .bind(row.id)
        .fetch_all(&state.db)
        .await?;
        let images = image_rows
            .into_iter()
            .map(
                |(asset_id, sku_id, sku_code, ord, path_rel, thumb_rel)| PostImageView {
                    asset_id,
                    sku_id,
                    sku_code,
                    ord,
                    path: paths::RelPath::new(path_rel)
                        .to_local(&root)
                        .to_string_lossy()
                        .to_string(),
                    thumb: paths::RelPath::new(thumb_rel)
                        .to_local(&root)
                        .to_string_lossy()
                        .to_string(),
                },
            )
            .collect();
        let task_rows: Vec<PublishTaskRow> = sqlx::query_as(
            "SELECT id,task_code,platform,scheduled_at,status,fail_kind,result_msg
             FROM publish_tasks WHERE post_id=?1 ORDER BY scheduled_at,platform",
        )
        .bind(row.id)
        .fetch_all(&state.db)
        .await?;
        let tasks = task_rows
            .into_iter()
            .map(
                |(id, task_code, platform, scheduled_at, status, fail_kind, result_msg)| {
                    let platform_zh = Platform::from_code(&platform)
                        .map(|value| value.zh().to_string())
                        .unwrap_or_else(|| platform.clone());
                    PublishTaskView {
                        id,
                        task_code,
                        platform,
                        platform_zh,
                        scheduled_at,
                        status,
                        fail_kind,
                        result_msg,
                    }
                },
            )
            .collect();
        posts.push(PostView {
            id: row.id,
            content_code: row.content_code,
            seq: row.seq,
            kind: row.kind,
            title_id: row.title_id,
            body_id: row.body_id,
            title: row.title_text.unwrap_or_default(),
            body: row.body_text.unwrap_or_default(),
            topics: parse_vec(&row.topics_json),
            edited: row.edited != 0,
            images,
            tasks,
        });
    }
    Ok(SheetDetailView { summary, posts })
}

async fn ensure_draft(pool: &SqlitePool, post_id: i64) -> AppResult<i64> {
    let row: Option<(i64, String)> = sqlx::query_as(
        "SELECT s.id,s.status FROM task_sheets s JOIN posts p ON p.sheet_id=s.id WHERE p.id=?1",
    )
    .bind(post_id)
    .fetch_optional(pool)
    .await?;
    match row {
        Some((sheet_id, status)) if status == "draft" => Ok(sheet_id),
        Some(_) => Err(AppError::InvalidInput("只有草稿任务单可编辑".into())),
        None => Err(AppError::InvalidInput("篇不存在".into())),
    }
}

#[tauri::command]
#[specta::specta]
pub async fn update_post_text(
    state: State<'_, AppState>,
    post_id: i64,
    kind: String,
    text: String,
) -> AppResult<()> {
    ensure_draft(&state.db, post_id).await?;
    if text.trim().is_empty() || !matches!(kind.as_str(), "title" | "body") {
        return Err(AppError::InvalidInput("标题/正文不能为空".into()));
    }
    let column = if kind == "title" {
        "title_text"
    } else {
        "body_text"
    };
    let sql = format!("UPDATE posts SET {column}=?2,edited=1 WHERE id=?1");
    sqlx::query(&sql)
        .bind(post_id)
        .bind(text)
        .execute(&state.db)
        .await?;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn replace_post_copy(
    state: State<'_, AppState>,
    post_id: i64,
    kind: String,
    item_id: i64,
) -> AppResult<()> {
    ensure_draft(&state.db, post_id).await?;
    if !matches!(kind.as_str(), "title" | "body") {
        return Err(AppError::InvalidInput("文案类型非法".into()));
    }
    let mut tx = state.db.begin().await?;
    let (old_id, old_column, id_column, text_column) = if kind == "title" {
        let old: Option<i64> = sqlx::query_scalar("SELECT title_id FROM posts WHERE id=?1")
            .bind(post_id)
            .fetch_one(&mut *tx)
            .await?;
        (old, "title", "title_id", "title_text")
    } else {
        let old: Option<i64> = sqlx::query_scalar("SELECT body_id FROM posts WHERE id=?1")
            .bind(post_id)
            .fetch_one(&mut *tx)
            .await?;
        (old, "body", "body_id", "body_text")
    };
    let text: String = sqlx::query_scalar(
        "SELECT text FROM text_items WHERE id=?1 AND kind=?2 AND enabled=1 AND state='free'
         AND product_id=(SELECT s.product_id FROM posts p JOIN task_sheets s ON s.id=p.sheet_id WHERE p.id=?3)",
    )
    .bind(item_id)
    .bind(old_column)
    .bind(post_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::InvalidInput("所选文案已不可用".into()))?;
    if let Some(old_id) = old_id {
        sqlx::query("UPDATE text_items SET state='free',post_id=NULL WHERE id=?1 AND state='held' AND post_id=?2")
            .bind(old_id)
            .bind(post_id)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("UPDATE text_items SET state='held',post_id=?2 WHERE id=?1 AND state='free'")
        .bind(item_id)
        .bind(post_id)
        .execute(&mut *tx)
        .await?;
    let sql = format!("UPDATE posts SET {id_column}=?2,{text_column}=?3,edited=1 WHERE id=?1");
    sqlx::query(&sql)
        .bind(post_id)
        .bind(item_id)
        .bind(text)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn update_post_topics(
    state: State<'_, AppState>,
    post_id: i64,
    topics: Vec<String>,
) -> AppResult<()> {
    ensure_draft(&state.db, post_id).await?;
    let mut topics = topics
        .into_iter()
        .map(|topic| format!("#{}", topic.trim().trim_start_matches('#')))
        .filter(|topic| topic.len() > 1)
        .collect::<Vec<_>>();
    topics.sort();
    topics.dedup();
    sqlx::query("UPDATE posts SET topics_json=?2,edited=1 WHERE id=?1")
        .bind(post_id)
        .bind(serde_json::to_string(&topics)?)
        .execute(&state.db)
        .await?;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn replace_post_image(
    state: State<'_, AppState>,
    post_id: i64,
    ord: i64,
    asset_id: i64,
) -> AppResult<()> {
    ensure_draft(&state.db, post_id).await?;
    let mut tx = state.db.begin().await?;
    let old_id: i64 =
        sqlx::query_scalar("SELECT asset_id FROM post_images WHERE post_id=?1 AND ord=?2")
            .bind(post_id)
            .bind(ord)
            .fetch_one(&mut *tx)
            .await?;
    let (kind, new_sku_id): (String, i64) = sqlx::query_as(
        "SELECT p.kind,a.sku_id FROM posts p JOIN task_sheets s ON s.id=p.sheet_id
         JOIN image_assets a ON a.id=?2 JOIN skus sk ON sk.id=a.sku_id
         WHERE p.id=?1 AND sk.product_id=s.product_id",
    )
    .bind(post_id)
    .bind(asset_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::InvalidInput("所选图片不属于当前商品".into()))?;
    if kind == "mixed" {
        let duplicate_sku: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM post_images pi JOIN image_assets a ON a.id=pi.asset_id
             WHERE pi.post_id=?1 AND pi.ord!=?2 AND a.sku_id=?3",
        )
        .bind(post_id)
        .bind(ord)
        .bind(new_sku_id)
        .fetch_one(&mut *tx)
        .await?;
        if duplicate_sku > 0 {
            return Err(AppError::InvalidInput(
                "混搭篇每个 SKU 只能有一张图片".into(),
            ));
        }
    }
    let changed = sqlx::query(
        "UPDATE image_assets SET state='held',post_id=?2,updated_at=?3
         WHERE id=?1 AND state='free' AND sku_id IN (
           SELECT sk.id FROM skus sk WHERE sk.product_id=(
             SELECT s.product_id FROM posts p JOIN task_sheets s ON s.id=p.sheet_id WHERE p.id=?2
           )
         )",
    )
    .bind(asset_id)
    .bind(post_id)
    .bind(crate::db::now_unix())
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if changed != 1 {
        return Err(AppError::InvalidInput("所选图片已不可用".into()));
    }
    sqlx::query("UPDATE post_images SET asset_id=?3 WHERE post_id=?1 AND ord=?2")
        .bind(post_id)
        .bind(ord)
        .bind(asset_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE image_assets SET state='free',post_id=NULL,updated_at=?2 WHERE id=?1 AND state='held'")
        .bind(old_id)
        .bind(crate::db::now_unix())
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE posts SET edited=1 WHERE id=?1")
        .bind(post_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn reorder_post_images(
    state: State<'_, AppState>,
    post_id: i64,
    asset_ids: Vec<i64>,
) -> AppResult<()> {
    ensure_draft(&state.db, post_id).await?;
    let current: Vec<i64> =
        sqlx::query_scalar("SELECT asset_id FROM post_images WHERE post_id=?1 ORDER BY ord")
            .bind(post_id)
            .fetch_all(&state.db)
            .await?;
    let mut a = current.clone();
    let mut b = asset_ids.clone();
    a.sort_unstable();
    b.sort_unstable();
    if a != b {
        return Err(AppError::InvalidInput("排序列表与当前图片不一致".into()));
    }
    let mut tx = state.db.begin().await?;
    sqlx::query("DELETE FROM post_images WHERE post_id=?1")
        .bind(post_id)
        .execute(&mut *tx)
        .await?;
    for (ord, asset_id) in asset_ids.iter().enumerate() {
        sqlx::query("INSERT INTO post_images(post_id,asset_id,ord) VALUES(?1,?2,?3)")
            .bind(post_id)
            .bind(asset_id)
            .bind(ord as i64)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("UPDATE posts SET edited=1 WHERE id=?1")
        .bind(post_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn delete_post(state: State<'_, AppState>, post_id: i64) -> AppResult<()> {
    ensure_draft(&state.db, post_id).await?;
    let mut tx = state.db.begin().await?;
    sqlx::query("UPDATE image_assets SET state='free',post_id=NULL,updated_at=?2 WHERE post_id=?1 AND state='held'")
        .bind(post_id)
        .bind(crate::db::now_unix())
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "UPDATE text_items SET state='free',post_id=NULL WHERE post_id=?1 AND state='held'",
    )
    .bind(post_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM posts WHERE id=?1")
        .bind(post_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn append_post(state: State<'_, AppState>, sheet_id: i64) -> AppResult<i64> {
    let row: Option<(String, i64, String, i64, String)> = sqlx::query_as(
        "SELECT s.date,s.product_id,p.code,c.images_per_post,c.sku_scope_json
         FROM task_sheets s JOIN products p ON p.id=s.product_id
         JOIN sheet_configs c ON c.id=s.config_id
         WHERE s.id=?1 AND s.status='draft'",
    )
    .bind(sheet_id)
    .fetch_optional(&state.db)
    .await?;
    let Some((date, product_id, product_code, images_per_post, sku_scope_json)) = row else {
        return Err(AppError::InvalidInput("只有草稿任务单可增加篇".into()));
    };
    let mut tx = state.db.begin().await?;
    let sku_scope: Vec<i64> = parse_vec(&sku_scope_json);
    let image_rows: Vec<(i64, String, i64)> = sqlx::query_as(
        "SELECT s.id,s.tier,a.id FROM skus s JOIN image_assets a ON a.sku_id=s.id
         WHERE s.product_id=?1 AND s.status='active' AND a.state='free'
         ORDER BY CASE s.tier WHEN 'hot' THEN 0 WHEN 'warm' THEN 1 ELSE 2 END,s.id,a.created_at,a.id",
    )
    .bind(product_id)
    .fetch_all(&mut *tx)
    .await?;
    let mut sku_map: HashMap<i64, SkuPool> = HashMap::new();
    for (sku_id, tier, asset_id) in image_rows {
        if !sku_scope.is_empty() && !sku_scope.contains(&sku_id) {
            continue;
        }
        sku_map
            .entry(sku_id)
            .or_insert_with(|| SkuPool {
                sku_id,
                tier,
                image_ids: Vec::new(),
            })
            .image_ids
            .push(asset_id);
    }
    let titles: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM text_items WHERE product_id=?1 AND kind='title' AND enabled=1 AND state='free'
         ORDER BY created_at,id",
    )
    .bind(product_id)
    .fetch_all(&mut *tx)
    .await?;
    let bodies: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM text_items WHERE product_id=?1 AND kind='body' AND enabled=1 AND state='free'
         ORDER BY created_at,id",
    )
    .bind(product_id)
    .fetch_all(&mut *tx)
    .await?;
    let topic_rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT scope,sku_ids_json,tags_json FROM topic_groups
         WHERE enabled=1 AND (product_id=?1 OR scope='general')
         ORDER BY CASE scope WHEN 'combo' THEN 0 WHEN 'product' THEN 1 ELSE 2 END,id",
    )
    .bind(product_id)
    .fetch_all(&mut *tx)
    .await?;
    let output = composer::compose(&ComposeInput {
        posts_per_day: 1,
        images_per_post: images_per_post as usize,
        mixed_count: 0,
        skus: sku_map.into_values().collect(),
        texts: TextPool {
            title_ids: titles,
            body_ids: bodies,
        },
        topics: topic_rows
            .into_iter()
            .map(|(scope, skus, tags)| TopicCandidate {
                scope,
                sku_ids: parse_vec(&skus),
                tags: parse_vec(&tags),
            })
            .collect(),
    });
    let composed = output.posts.first().ok_or_else(|| {
        AppError::InvalidInput(
            output
                .shortages
                .iter()
                .map(|item| item.detail.clone())
                .collect::<Vec<_>>()
                .join("；"),
        )
    })?;
    let seq: i64 =
        sqlx::query_scalar("SELECT COALESCE(MAX(seq),-1)+1 FROM posts WHERE sheet_id=?1")
            .bind(sheet_id)
            .fetch_one(&mut *tx)
            .await?;
    let content_code = format!("C{}-{}-{:02}", date_code(&date)?, product_code, seq + 1);
    let title_text: String = sqlx::query_scalar("SELECT text FROM text_items WHERE id=?1")
        .bind(composed.title_id)
        .fetch_one(&mut *tx)
        .await?;
    let body_text: String = sqlx::query_scalar("SELECT text FROM text_items WHERE id=?1")
        .bind(composed.body_id)
        .fetch_one(&mut *tx)
        .await?;
    let post_id: i64 = sqlx::query_scalar(
        "INSERT INTO posts(sheet_id,content_code,seq,kind,title_id,body_id,title_text,body_text,topics_json,edited)
         VALUES(?1,?2,?3,'single',?4,?5,?6,?7,?8,1) RETURNING id",
    )
    .bind(sheet_id)
    .bind(content_code)
    .bind(seq)
    .bind(composed.title_id)
    .bind(composed.body_id)
    .bind(title_text)
    .bind(body_text)
    .bind(serde_json::to_string(&composed.topics)?)
    .fetch_one(&mut *tx)
    .await?;
    for (ord, asset_id) in composed.image_ids.iter().enumerate() {
        sqlx::query("INSERT INTO post_images(post_id,asset_id,ord) VALUES(?1,?2,?3)")
            .bind(post_id)
            .bind(asset_id)
            .bind(ord as i64)
            .execute(&mut *tx)
            .await?;
        let changed = sqlx::query(
            "UPDATE image_assets SET state='held',post_id=?2,updated_at=?3
             WHERE id=?1 AND state='free'",
        )
        .bind(asset_id)
        .bind(post_id)
        .bind(crate::db::now_unix())
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if changed != 1 {
            return Err(AppError::InvalidInput("图片素材被并发占用，请重试".into()));
        }
    }
    for text_id in [composed.title_id, composed.body_id] {
        let changed = sqlx::query(
            "UPDATE text_items SET state='held',post_id=?2 WHERE id=?1 AND state='free'",
        )
        .bind(text_id)
        .bind(post_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if changed != 1 {
            return Err(AppError::InvalidInput("文案被并发占用，请重试".into()));
        }
    }
    reschedule_draft_date(&mut tx, &date, Local::now().naive_local()).await?;
    tx.commit().await?;
    Ok(post_id)
}

#[tauri::command]
#[specta::specta]
pub async fn regenerate_sheet(state: State<'_, AppState>, sheet_id: i64) -> AppResult<Vec<i64>> {
    let mut tx = state.db.begin().await?;
    let row = sqlx::query_as::<_, RegenerateRow>(
        "SELECT c.id,c.product_id,c.sku_scope_json,c.posts_per_day,c.images_per_post,
                c.mixed_count,c.target_day,c.enabled,s.date,p.code
         FROM task_sheets s JOIN sheet_configs c ON c.id=s.config_id
         JOIN products p ON p.id=s.product_id
         WHERE s.id=?1 AND s.status='draft'",
    )
    .bind(sheet_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        return Err(AppError::InvalidInput("只有草稿任务单可以重新组稿".into()));
    };
    let config = sheet_repo::ConfigRow {
        id: row.id,
        product_id: row.product_id,
        sku_scope_json: row.sku_scope_json,
        posts_per_day: row.posts_per_day,
        images_per_post: row.images_per_post,
        mixed_count: row.mixed_count,
        target_day: row.target_day,
        enabled: row.enabled,
    };
    let product = ProductForCompose {
        id: config.product_id,
        code: row.product_code,
    };
    let composed_id = compose_one(&mut tx, &config, &product, &row.date).await?;
    reschedule_draft_date(&mut tx, &row.date, Local::now().naive_local()).await?;
    tx.commit().await?;
    Ok(vec![composed_id])
}

#[tauri::command]
#[specta::specta]
pub async fn confirm_sheet(state: State<'_, AppState>, sheet_id: i64) -> AppResult<()> {
    let n = sqlx::query(
        "UPDATE task_sheets SET status='confirmed',updated_at=?2
         WHERE id=?1 AND status='draft'
           AND export_token IS NULL
           AND EXISTS(SELECT 1 FROM posts WHERE sheet_id=task_sheets.id)",
    )
    .bind(sheet_id)
    .bind(crate::db::now_unix())
    .execute(&state.db)
    .await?
    .rows_affected();
    if n != 1 {
        let status: Option<String> =
            sqlx::query_scalar("SELECT status FROM task_sheets WHERE id=?1")
                .bind(sheet_id)
                .fetch_optional(&state.db)
                .await?;
        return Err(AppError::InvalidInput(match status.as_deref() {
            Some("draft") => "空任务单不能确认，请先增加一篇".into(),
            Some(_) => "只有草稿任务单可确认".into(),
            None => "任务单不存在".into(),
        }));
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn reopen_sheet(state: State<'_, AppState>, sheet_id: i64) -> AppResult<()> {
    reopen_sheet_inner(&state.db, sheet_id).await
}

async fn reopen_sheet_inner(pool: &SqlitePool, sheet_id: i64) -> AppResult<()> {
    let mut tx = pool.begin().await?;
    let reopened: Option<i64> = sqlx::query_scalar(
        "UPDATE task_sheets SET status='draft',updated_at=?2
         WHERE id=?1 AND status='confirmed' AND export_token IS NULL RETURNING id",
    )
    .bind(sheet_id)
    .bind(crate::db::now_unix())
    .fetch_optional(&mut *tx)
    .await?;
    if reopened.is_none() {
        return Err(AppError::InvalidInput(
            "只有已确认、未在导出且尚未生成任务包的任务单可退回编辑".into(),
        ));
    }
    tx.commit().await?;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn cancel_sheet(state: State<'_, AppState>, sheet_id: i64) -> AppResult<()> {
    let mut tx = state.db.begin().await?;
    let draft: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM task_sheets
             WHERE id=?1 AND status='draft' AND export_token IS NULL",
    )
    .bind(sheet_id)
    .fetch_optional(&mut *tx)
    .await?;
    if draft.is_none() {
        return Err(AppError::InvalidInput("只有草稿任务单可以取消".into()));
    }
    sqlx::query(
        "UPDATE image_assets SET state='free',post_id=NULL,updated_at=?2
         WHERE state='held' AND post_id IN (SELECT id FROM posts WHERE sheet_id=?1)",
    )
    .bind(sheet_id)
    .bind(crate::db::now_unix())
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE text_items SET state='free',post_id=NULL
         WHERE state='held' AND post_id IN (SELECT id FROM posts WHERE sheet_id=?1)",
    )
    .bind(sheet_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM task_sheets WHERE id=?1 AND status='draft'")
        .bind(sheet_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn export_task_sheet(
    state: State<'_, AppState>,
    sheet_id: i64,
) -> AppResult<exporter::ExportResult> {
    let settings = publish_settings::load(&state.db).await?;
    let root = publish_settings::root_local(&state.db).await?;
    if settings.root_exec.trim().is_empty() {
        return Err(AppError::InvalidInput(
            "尚未配置执行机根路径，请到设置页填写或选择“同本机”".into(),
        ));
    }
    let style = paths::PathStyle::from_str_or_default(&settings.path_style);
    if !paths::is_exec_root_absolute(&settings.root_exec, style) {
        return Err(AppError::InvalidInput("执行机根路径不是绝对路径".into()));
    }
    exporter::export(
        &state.db,
        &root,
        &settings.root_exec,
        style,
        sheet_id,
        Local::now().naive_local(),
    )
    .await
}

/// 活动包在首份回执前被外部删除/损坏时，把 used 安全退回 held，允许重新导出。
/// READY 仍存在、导出仍占用、已有回执或任何任务已有终态时一律拒绝，避免干扰 RPA。
#[tauri::command]
#[specta::specta]
pub async fn recover_missing_export(state: State<'_, AppState>, sheet_id: i64) -> AppResult<()> {
    recover_missing_export_inner(&state.db, sheet_id).await
}

async fn recover_missing_export_inner(pool: &SqlitePool, sheet_id: i64) -> AppResult<()> {
    let row: Option<(String, Option<String>, Option<String>)> =
        sqlx::query_as("SELECT status,export_dir,export_token FROM task_sheets WHERE id=?1")
            .bind(sheet_id)
            .fetch_optional(pool)
            .await?;
    let Some((status, Some(export_dir), export_token)) = row else {
        return Err(AppError::InvalidInput("任务单没有可恢复的导出记录".into()));
    };
    if export_token.is_some() {
        return Err(AppError::InvalidInput(
            "任务单仍在导出收尾中，请稍后再恢复".into(),
        ));
    }
    if status != "exported" {
        return Err(AppError::InvalidInput(
            "只有尚未收回回执的已导出任务单可恢复".into(),
        ));
    }
    let package = std::path::Path::new(&export_dir);
    let receipt = package.join(paths::RECEIPT_JSONL);
    if receipt.metadata().is_ok_and(|metadata| metadata.len() > 0) {
        return Err(AppError::InvalidInput(
            "回执已有内容，必须先收回结果，不能恢复导出".into(),
        ));
    }
    if package.join(paths::READY).is_file() {
        return Err(AppError::InvalidInput(
            "READY.txt 仍存在，RPA 可能正在读取；请先停止 RPA 并移除 READY.txt，再恢复损坏包"
                .into(),
        ));
    }
    let terminal: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM publish_tasks WHERE sheet_id=?1 AND status!='pending'",
    )
    .bind(sheet_id)
    .fetch_one(pool)
    .await?;
    if terminal > 0 {
        return Err(AppError::InvalidInput(
            "已有任务终态，必须走回执结算，不能恢复导出".into(),
        ));
    }
    let mut tx = pool.begin().await?;
    sqlx::query(
        "UPDATE image_assets SET state='held',updated_at=?2
         WHERE state='used' AND post_id IN (SELECT id FROM posts WHERE sheet_id=?1)",
    )
    .bind(sheet_id)
    .bind(crate::db::now_unix())
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE text_items SET state='held'
         WHERE state='used' AND post_id IN (SELECT id FROM posts WHERE sheet_id=?1)",
    )
    .bind(sheet_id)
    .execute(&mut *tx)
    .await?;
    let changed = sqlx::query(
        "UPDATE task_sheets SET status='confirmed',export_dir=NULL,exported_at=NULL,updated_at=?2
         WHERE id=?1 AND status='exported' AND export_token IS NULL",
    )
    .bind(sheet_id)
    .bind(crate::db::now_unix())
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if changed != 1 {
        return Err(AppError::InvalidInput("任务单状态已变化，请重试".into()));
    }
    tx.commit().await?;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn collect_sheet_receipts(
    state: State<'_, AppState>,
    sheet_id: i64,
) -> AppResult<settle::ReceiptImportResult> {
    settle::import_receipts(&state.db, sheet_id).await
}

#[tauri::command]
#[specta::specta]
pub async fn close_task_sheet(
    state: State<'_, AppState>,
    sheet_id: i64,
) -> AppResult<settle::CloseResult> {
    let root = publish_settings::root_local(&state.db).await?;
    settle::close(&state.db, &root, sheet_id).await
}

#[tauri::command]
#[specta::specta]
pub async fn open_task_sheet_dir(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    sheet_id: i64,
) -> AppResult<()> {
    use tauri_plugin_opener::OpenerExt;
    let dir: Option<String> = sqlx::query_scalar("SELECT export_dir FROM task_sheets WHERE id=?1")
        .bind(sheet_id)
        .fetch_optional(&state.db)
        .await?
        .flatten();
    let dir = dir.ok_or_else(|| AppError::InvalidInput("任务单尚未导出".into()))?;
    app.opener()
        .open_path(dir, None::<&str>)
        .map_err(|err| AppError::Io(err.to_string()))
}

#[cfg(test)]
// 测试断言失败即测试失败。
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::db::test_support::test_pool;
    use crate::publish::{paths::PathStyle, receipt::ReceiptLine};

    async fn seed_publish_flow(pool: &SqlitePool, root: &std::path::Path) -> i64 {
        let now = crate::db::now_unix();
        let product_id: i64 = sqlx::query_scalar(
            "INSERT INTO products(code,name,platforms_json,created_at,updated_at)
             VALUES('A','商品A','[\"xhs\"]',?1,?1) RETURNING id",
        )
        .bind(now)
        .fetch_one(pool)
        .await
        .unwrap();
        let sku_id: i64 = sqlx::query_scalar(
            "INSERT INTO skus(code,style_name,product_name,tier,topics_json,status,is_general,note,
              created_at,updated_at,folder_alias,product_id,music_keyword)
             VALUES('A-01','款式一','','hot','[]','active',0,'',?1,?1,'',?2,'') RETURNING id",
        )
        .bind(now)
        .bind(product_id)
        .fetch_one(pool)
        .await
        .unwrap();
        for index in 1..=4 {
            let rel = paths::RelPath::from_parts([
                paths::IMAGE_LIBRARY,
                "A-01",
                format!("source_{index}.jpg").as_str(),
            ]);
            let local = rel.to_local(root);
            std::fs::create_dir_all(local.parent().unwrap()).unwrap();
            std::fs::write(&local, format!("image-{index}")).unwrap();
            sqlx::query(
                "INSERT INTO image_assets(sku_id,path_rel,thumb_rel,source,state,created_at,updated_at)
                 VALUES(?1,?2,?2,'import','free',?3,?3)",
            )
            .bind(sku_id)
            .bind(rel.as_str())
            .bind(now + index)
            .execute(pool)
            .await
            .unwrap();
        }
        for index in 1..=2 {
            sqlx::query(
                "INSERT INTO text_items(product_id,kind,text,source,state,created_at)
                 VALUES(?1,'title',?2,'manual','free',?3)",
            )
            .bind(product_id)
            .bind(format!("标题{index}"))
            .bind(now + index)
            .execute(pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO text_items(product_id,kind,text,source,state,created_at)
                 VALUES(?1,'body',?2,'manual','free',?3)",
            )
            .bind(product_id)
            .bind(format!("正文{index}"))
            .bind(now + index)
            .execute(pool)
            .await
            .unwrap();
        }
        sqlx::query(
            "INSERT INTO topic_groups(product_id,scope,tags_json,created_at,updated_at)
             VALUES(?1,'product','[\"#话题\"]',?2,?2)",
        )
        .bind(product_id)
        .bind(now)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO sheet_configs(product_id,name,platforms_json,posts_per_day,images_per_post,
             mixed_count,anchors_json,jitter_min,min_gap_min,target_day,created_at,updated_at)
             VALUES(?1,'每日','[\"xhs\"]',2,2,0,'[\"10:00\",\"14:00\"]',0,30,'same',?2,?2)",
        )
        .bind(product_id)
        .bind(now)
        .execute(pool)
        .await
        .unwrap();
        product_id
    }

    #[tokio::test]
    async fn regenerate_preserves_edited_post_and_releases_every_other_material() {
        let (pool, dir) = test_pool().await;
        let root = dir.path().join("publish-root");
        std::fs::create_dir_all(&root).unwrap();
        let product_id = seed_publish_flow(&pool, &root).await;
        let generated_at =
            NaiveDateTime::parse_from_str("2026-08-01 08:00", "%Y-%m-%d %H:%M").unwrap();
        let sheet_id = generate_for_date(&pool, "2026-08-02", generated_at)
            .await
            .unwrap()[0];
        let posts: Vec<(i64, i64)> =
            sqlx::query_as("SELECT id,seq FROM posts WHERE sheet_id=?1 ORDER BY seq")
                .bind(sheet_id)
                .fetch_all(&pool)
                .await
                .unwrap();
        let edited_post = posts[0].0;
        let replaceable_post = posts[1].0;
        let edited_images: Vec<i64> =
            sqlx::query_scalar("SELECT asset_id FROM post_images WHERE post_id=?1 ORDER BY ord")
                .bind(edited_post)
                .fetch_all(&pool)
                .await
                .unwrap();
        let old_images: Vec<i64> =
            sqlx::query_scalar("SELECT asset_id FROM post_images WHERE post_id=?1 ORDER BY ord")
                .bind(replaceable_post)
                .fetch_all(&pool)
                .await
                .unwrap();
        let old_texts: (i64, i64) =
            sqlx::query_as("SELECT title_id,body_id FROM posts WHERE id=?1")
                .bind(replaceable_post)
                .fetch_one(&pool)
                .await
                .unwrap();
        sqlx::query("UPDATE posts SET title_text='人工保留标题',edited=1 WHERE id=?1")
            .bind(edited_post)
            .execute(&pool)
            .await
            .unwrap();

        let sku_id: i64 = sqlx::query_scalar("SELECT id FROM skus WHERE code='A-01'")
            .fetch_one(&pool)
            .await
            .unwrap();
        let mut new_images = Vec::new();
        for index in 5..=6 {
            let rel = paths::RelPath::from_parts([
                paths::IMAGE_LIBRARY,
                "A-01",
                format!("source_{index}.jpg").as_str(),
            ]);
            let local = rel.to_local(&root);
            std::fs::write(&local, format!("image-{index}")).unwrap();
            let id: i64 = sqlx::query_scalar(
                "INSERT INTO image_assets(sku_id,path_rel,thumb_rel,source,state,created_at,updated_at)
                 VALUES(?1,?2,?2,'import','free',0,0) RETURNING id",
            )
            .bind(sku_id)
            .bind(rel.as_str())
            .fetch_one(&pool)
            .await
            .unwrap();
            new_images.push(id);
        }
        for (kind, text) in [("title", "新标题"), ("body", "新正文")] {
            sqlx::query(
                "INSERT INTO text_items(product_id,kind,text,source,state,created_at)
                 VALUES(?1,?2,?3,'manual','free',0)",
            )
            .bind(product_id)
            .bind(kind)
            .bind(text)
            .execute(&pool)
            .await
            .unwrap();
        }

        generate_for_date(&pool, "2026-08-02", generated_at)
            .await
            .unwrap();
        let preserved: (String, i64) =
            sqlx::query_as("SELECT title_text,edited FROM posts WHERE id=?1")
                .bind(edited_post)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(preserved, ("人工保留标题".into(), 1));
        let preserved_images: Vec<i64> =
            sqlx::query_scalar("SELECT asset_id FROM post_images WHERE post_id=?1 ORDER BY ord")
                .bind(edited_post)
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(preserved_images, edited_images);

        let regenerated_images: Vec<i64> = sqlx::query_scalar(
            "SELECT pi.asset_id FROM post_images pi JOIN posts p ON p.id=pi.post_id
             WHERE p.sheet_id=?1 AND p.seq=1 ORDER BY pi.ord",
        )
        .bind(sheet_id)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(regenerated_images, new_images);
        for id in old_images {
            assert_eq!(
                sqlx::query_scalar::<_, String>("SELECT state FROM image_assets WHERE id=?1")
                    .bind(id)
                    .fetch_one(&pool)
                    .await
                    .unwrap(),
                "free"
            );
        }
        for id in [old_texts.0, old_texts.1] {
            assert_eq!(
                sqlx::query_scalar::<_, String>("SELECT state FROM text_items WHERE id=?1")
                    .bind(id)
                    .fetch_one(&pool)
                    .await
                    .unwrap(),
                "free"
            );
        }
    }

    #[tokio::test]
    async fn confirmed_product_does_not_block_other_products_in_batch_generation() {
        let (pool, dir) = test_pool().await;
        let root = dir.path().join("publish-root");
        std::fs::create_dir_all(&root).unwrap();
        let product_a = seed_publish_flow(&pool, &root).await;
        let generated_at =
            NaiveDateTime::parse_from_str("2026-08-01 08:00", "%Y-%m-%d %H:%M").unwrap();
        let sheet_a = generate_for_date(&pool, "2026-08-02", generated_at)
            .await
            .unwrap()[0];
        sqlx::query("UPDATE task_sheets SET status='confirmed' WHERE id=?1")
            .bind(sheet_a)
            .execute(&pool)
            .await
            .unwrap();

        let now = crate::db::now_unix();
        let product_b: i64 = sqlx::query_scalar(
            "INSERT INTO products(code,name,platforms_json,created_at,updated_at)
             VALUES('B','商品B','[\"xhs\"]',?1,?1) RETURNING id",
        )
        .bind(now)
        .fetch_one(&pool)
        .await
        .unwrap();
        let sku_b: i64 = sqlx::query_scalar(
            "INSERT INTO skus(code,style_name,product_name,tier,topics_json,status,is_general,note,
             created_at,updated_at,folder_alias,product_id,music_keyword)
             VALUES('B-01','款式B','','hot','[]','active',0,'',?1,?1,'',?2,'') RETURNING id",
        )
        .bind(now)
        .bind(product_b)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO image_assets(sku_id,path_rel,thumb_rel,source,state,created_at,updated_at)
             VALUES(?1,'图片素材库/B-01/one.jpg','图片素材库/B-01/one.jpg','import','free',?2,?2)",
        )
        .bind(sku_b)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        for (kind, text) in [("title", "B标题"), ("body", "B正文")] {
            sqlx::query(
                "INSERT INTO text_items(product_id,kind,text,source,state,created_at)
                 VALUES(?1,?2,?3,'manual','free',?4)",
            )
            .bind(product_b)
            .bind(kind)
            .bind(text)
            .bind(now)
            .execute(&pool)
            .await
            .unwrap();
        }
        sqlx::query(
            "INSERT INTO sheet_configs(product_id,name,platforms_json,posts_per_day,images_per_post,
             mixed_count,anchors_json,jitter_min,min_gap_min,target_day,created_at,updated_at)
             VALUES(?1,'B每日','[\"xhs\"]',1,1,0,'[\"16:00\"]',0,3,'same',?2,?2)",
        )
        .bind(product_b)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();

        let generated = generate_for_date(&pool, "2026-08-02", generated_at)
            .await
            .unwrap();
        assert_eq!(generated.len(), 1);
        let generated_product: i64 =
            sqlx::query_scalar("SELECT product_id FROM task_sheets WHERE id=?1")
                .bind(generated[0])
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(generated_product, product_b);
        let a_status: String =
            sqlx::query_scalar("SELECT status FROM task_sheets WHERE product_id=?1")
                .bind(product_a)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(a_status, "confirmed");
    }

    #[tokio::test]
    async fn missing_active_package_can_recover_before_receipts_start() {
        let (pool, dir) = test_pool().await;
        let root = dir.path().join("publish-root");
        std::fs::create_dir_all(&root).unwrap();
        seed_publish_flow(&pool, &root).await;
        let generated_at =
            NaiveDateTime::parse_from_str("2026-08-01 08:00", "%Y-%m-%d %H:%M").unwrap();
        let sheet_id = generate_for_date(&pool, "2026-08-02", generated_at)
            .await
            .unwrap()[0];
        sqlx::query("UPDATE task_sheets SET status='confirmed' WHERE id=?1")
            .bind(sheet_id)
            .execute(&pool)
            .await
            .unwrap();
        let exported = exporter::export(
            &pool,
            &root,
            r"D:\GenDesk",
            PathStyle::Windows,
            sheet_id,
            generated_at,
        )
        .await
        .unwrap();
        std::fs::remove_file(std::path::Path::new(&exported.directory).join(paths::TASK_JSON))
            .unwrap();
        assert!(recover_missing_export_inner(&pool, sheet_id)
            .await
            .unwrap_err()
            .to_string()
            .contains("READY.txt 仍存在"));
        std::fs::remove_file(std::path::Path::new(&exported.directory).join(paths::READY)).unwrap();
        recover_missing_export_inner(&pool, sheet_id).await.unwrap();
        let status: String = sqlx::query_scalar("SELECT status FROM task_sheets WHERE id=?1")
            .bind(sheet_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(status, "confirmed");
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM image_assets WHERE state='held'")
                .fetch_one(&pool)
                .await
                .unwrap(),
            4
        );
        exporter::export(
            &pool,
            &root,
            r"D:\GenDesk",
            PathStyle::Windows,
            sheet_id,
            generated_at,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn export_claim_blocks_reopen_until_the_owner_releases_it() {
        let (pool, dir) = test_pool().await;
        let root = dir.path().join("publish-root");
        std::fs::create_dir_all(&root).unwrap();
        seed_publish_flow(&pool, &root).await;
        let generated_at =
            NaiveDateTime::parse_from_str("2026-08-01 08:00", "%Y-%m-%d %H:%M").unwrap();
        let sheet_id = generate_for_date(&pool, "2026-08-02", generated_at)
            .await
            .unwrap()[0];
        sqlx::query(
            "UPDATE task_sheets SET status='confirmed',export_token='active-export' WHERE id=?1",
        )
        .bind(sheet_id)
        .execute(&pool)
        .await
        .unwrap();

        assert!(reopen_sheet_inner(&pool, sheet_id).await.is_err());
        let state: (String, Option<String>) =
            sqlx::query_as("SELECT status,export_token FROM task_sheets WHERE id=?1")
                .bind(sheet_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(state, ("confirmed".into(), Some("active-export".into())));

        sqlx::query("UPDATE task_sheets SET export_token=NULL WHERE id=?1")
            .bind(sheet_id)
            .execute(&pool)
            .await
            .unwrap();
        reopen_sheet_inner(&pool, sheet_id).await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT status FROM task_sheets WHERE id=?1")
                .bind(sheet_id)
                .fetch_one(&pool)
                .await
                .unwrap(),
            "draft"
        );
    }

    #[tokio::test]
    async fn startup_reconciles_both_export_crash_windows() {
        let (pool, dir) = test_pool().await;
        let root = dir.path().join("publish-root");
        std::fs::create_dir_all(&root).unwrap();
        seed_publish_flow(&pool, &root).await;
        let generated_at =
            NaiveDateTime::parse_from_str("2026-08-01 08:00", "%Y-%m-%d %H:%M").unwrap();
        let sheet_id = generate_for_date(&pool, "2026-08-02", generated_at)
            .await
            .unwrap()[0];
        sqlx::query("UPDATE task_sheets SET status='confirmed' WHERE id=?1")
            .bind(sheet_id)
            .execute(&pool)
            .await
            .unwrap();
        let exported = exporter::export(
            &pool,
            &root,
            r"D:\GenDesk",
            PathStyle::Windows,
            sheet_id,
            generated_at,
        )
        .await
        .unwrap();

        sqlx::query("UPDATE task_sheets SET export_token='crash-after-ready' WHERE id=?1")
            .bind(sheet_id)
            .execute(&pool)
            .await
            .unwrap();
        crate::db::recover_interrupted_exports(&pool).await.unwrap();
        let ready_state: (String, Option<String>) =
            sqlx::query_as("SELECT status,export_token FROM task_sheets WHERE id=?1")
                .bind(sheet_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(ready_state, ("exported".into(), None));

        std::fs::remove_file(std::path::Path::new(&exported.directory).join(paths::READY)).unwrap();
        sqlx::query("UPDATE task_sheets SET export_token='crash-before-ready' WHERE id=?1")
            .bind(sheet_id)
            .execute(&pool)
            .await
            .unwrap();
        crate::db::recover_interrupted_exports(&pool).await.unwrap();
        let interrupted_state: (String, Option<String>, Option<String>) =
            sqlx::query_as("SELECT status,export_dir,export_token FROM task_sheets WHERE id=?1")
                .bind(sheet_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(interrupted_state, ("confirmed".into(), None, None));
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM image_assets WHERE state='held'")
                .fetch_one(&pool)
                .await
                .unwrap(),
            4
        );
    }

    #[tokio::test]
    async fn json_export_receipts_and_close_form_one_material_lifecycle() {
        let (pool, dir) = test_pool().await;
        let root = dir.path().join("publish-root");
        std::fs::create_dir_all(&root).unwrap();
        seed_publish_flow(&pool, &root).await;
        let generated_at =
            NaiveDateTime::parse_from_str("2026-08-01 08:00", "%Y-%m-%d %H:%M").unwrap();
        let sheet_ids = generate_for_date(&pool, "2026-08-02", generated_at)
            .await
            .unwrap();
        assert_eq!(sheet_ids.len(), 1);
        let sheet_id = sheet_ids[0];
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM image_assets WHERE state='held'")
                .fetch_one(&pool)
                .await
                .unwrap(),
            4
        );
        sqlx::query("UPDATE task_sheets SET status='confirmed' WHERE id=?1")
            .bind(sheet_id)
            .execute(&pool)
            .await
            .unwrap();

        let (first_export, second_export) = tokio::join!(
            exporter::export(
                &pool,
                &root,
                r"D:\GenDesk",
                PathStyle::Windows,
                sheet_id,
                generated_at,
            ),
            exporter::export(
                &pool,
                &root,
                r"D:\GenDesk",
                PathStyle::Windows,
                sheet_id,
                generated_at,
            )
        );
        let result_pair = match (first_export, second_export) {
            (Ok(exported), Err(rejected)) | (Err(rejected), Ok(exported)) => {
                Some((exported, rejected))
            }
            _ => None,
        };
        assert!(result_pair.is_some(), "并发导出必须恰好一个成功、一个拒绝");
        let (exported, rejected) = result_pair.expect("上方断言已验证结果");
        let rejected = rejected.to_string();
        assert!(rejected.contains("正在导出") || rejected.contains("READY 包处于活动状态"));
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM image_assets WHERE state='used'")
                .fetch_one(&pool)
                .await
                .unwrap(),
            4
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM text_items WHERE state='used'")
                .fetch_one(&pool)
                .await
                .unwrap(),
            4
        );
        let package = std::path::PathBuf::from(&exported.directory);
        for name in [
            paths::TASK_JSON,
            paths::EXEC_GUIDE,
            paths::RECEIPT_JSONL,
            paths::READY,
        ] {
            assert!(package.join(name).is_file(), "任务包缺 {name}");
        }
        let ready_mtime = package
            .join(paths::READY)
            .metadata()
            .unwrap()
            .modified()
            .unwrap();
        for name in [paths::TASK_JSON, paths::EXEC_GUIDE, paths::RECEIPT_JSONL] {
            assert!(ready_mtime >= package.join(name).metadata().unwrap().modified().unwrap());
        }
        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(package.join(paths::TASK_JSON)).unwrap())
                .unwrap();
        assert_eq!(value["schema"], "gendesk.tasksheet/1");
        assert_eq!(value["tasks"].as_array().unwrap().len(), 2);
        assert!(value["tasks"][0]["imagePaths"][0]
            .as_str()
            .unwrap()
            .starts_with(r"D:\GenDesk\任务包\"));
        assert!(value["tasks"][0].get("coverPath").is_none());

        let empty_import = settle::import_receipts(&pool, sheet_id).await.unwrap();
        assert_eq!(empty_import.applied, 0);
        assert_eq!(empty_import.pending, 2);
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT status FROM task_sheets WHERE id=?1")
                .bind(sheet_id)
                .fetch_one(&pool)
                .await
                .unwrap(),
            "exported"
        );

        let tasks: Vec<(i64, String)> = sqlx::query_as(
            "SELECT post_id,task_code FROM publish_tasks WHERE sheet_id=?1 ORDER BY post_id",
        )
        .bind(sheet_id)
        .fetch_all(&pool)
        .await
        .unwrap();
        let receipt_lines = [
            ReceiptLine {
                task_id: tasks[0].1.clone(),
                status: "已完成".into(),
                fail_kind: None,
                message: "草稿已存".into(),
                finished_at: "2026-08-02 10:01".into(),
            },
            ReceiptLine {
                task_id: tasks[1].1.clone(),
                status: "失败".into(),
                fail_kind: Some("登录失效".into()),
                message: "cookie 过期".into(),
                finished_at: "2026-08-02 14:01".into(),
            },
        ];
        let jsonl = receipt_lines
            .iter()
            .map(|line| serde_json::to_string(line).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(package.join(paths::RECEIPT_JSONL), jsonl).unwrap();
        let reexport_error = exporter::export(
            &pool,
            &root,
            r"D:\GenDesk",
            PathStyle::Windows,
            sheet_id,
            generated_at,
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(reexport_error.contains("请先收回结果"));
        let imported = settle::import_receipts(&pool, sheet_id).await.unwrap();
        assert_eq!(
            (imported.done, imported.failed, imported.pending),
            (1, 1, 0)
        );
        let replay = settle::import_receipts(&pool, sheet_id).await.unwrap();
        assert_eq!(replay.applied, 0, "相同回执重读必须幂等");
        let mut tampered = receipt_lines.to_vec();
        tampered[0].status = "失败".into();
        tampered[0].fail_kind = Some("其他".into());
        let tampered_jsonl = tampered
            .iter()
            .map(|line| serde_json::to_string(line).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(package.join(paths::RECEIPT_JSONL), tampered_jsonl).unwrap();
        let terminal_error = settle::import_receipts(&pool, sheet_id)
            .await
            .unwrap_err()
            .to_string();
        assert!(terminal_error.contains("拒绝重放或改写"));
        let states: Vec<(i64, String)> = sqlx::query_as(
            "SELECT post_id,state FROM image_assets WHERE post_id IS NOT NULL ORDER BY post_id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert!(states
            .iter()
            .any(|(post, state)| *post == tasks[0].0 && state == "used"));
        assert!(!states.iter().any(|(post, _)| *post == tasks[1].0));
        let released_text_links: (Option<i64>, Option<i64>) =
            sqlx::query_as("SELECT title_id,body_id FROM posts WHERE id=?1")
                .bind(tasks[1].0)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(released_text_links, (None, None));

        let success_files: Vec<String> = sqlx::query_scalar(
            "SELECT a.path_rel FROM image_assets a JOIN post_images pi ON pi.asset_id=a.id
             WHERE pi.post_id=?1",
        )
        .bind(tasks[0].0)
        .fetch_all(&pool)
        .await
        .unwrap();
        let failed_files: Vec<String> = sqlx::query_scalar(
            "SELECT a.path_rel FROM image_assets a JOIN post_images pi ON pi.asset_id=a.id
             WHERE pi.post_id=?1",
        )
        .bind(tasks[1].0)
        .fetch_all(&pool)
        .await
        .unwrap();
        let closed = settle::close(&pool, &root, sheet_id).await.unwrap();
        let report: serde_json::Value = serde_json::from_str(&closed.report_json).unwrap();
        assert_eq!(report["登录失效"], 1);
        assert!(success_files
            .iter()
            .all(|rel| !paths::RelPath::new(rel).to_local(&root).exists()));
        assert!(failed_files
            .iter()
            .all(|rel| paths::RelPath::new(rel).to_local(&root).exists()));
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM text_items WHERE post_id IS NULL AND state='free'",
            )
            .fetch_one(&pool)
            .await
            .unwrap(),
            2,
            "失败篇的标题与正文回到 free；成功篇已淘汰"
        );
    }
}
