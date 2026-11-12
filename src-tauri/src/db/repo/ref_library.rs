//! 参考图库数据仓（0051）。薄 SQL 封装；抽图与冷却规则在 `crate::ref_library`。

use sqlx::{FromRow, SqliteConnection, SqlitePool};

#[derive(Debug, Clone, FromRow)]
pub struct StyleRow {
    pub id: i64,
    pub name: String,
    pub product: String,
    pub note: String,
}

#[derive(Debug, Clone, FromRow)]
pub struct CategoryRow {
    pub id: i64,
    pub name: String,
    pub cooldown_days: i64,
    pub sort_order: i64,
}

/// 图库图片连同文件、款式与分类信息。
#[derive(Debug, Clone, FromRow)]
pub struct ImageRow {
    pub id: i64,
    pub style_id: i64,
    pub category_id: i64,
    pub ref_image_id: i64,
    pub note: String,
    pub enabled: bool,
    pub last_used_at: Option<i64>,
    pub use_count: i64,
    pub created_at: i64,
    pub deleted_at: Option<i64>,
    pub name: String,
    pub file_path: String,
    pub thumb_path: String,
    pub width: i64,
    pub height: i64,
    /// 0054：`file_path` 是用户自己的原图（映射），不是拷进数据目录的副本。
    pub linked: bool,
    pub style: String,
    pub product: String,
    pub category: String,
    pub cooldown_days: i64,
}

const IMAGE_SELECT: &str = "SELECT i.id, i.style_id, i.category_id, i.ref_image_id,
        i.note, i.enabled, i.last_used_at, i.use_count, i.created_at, i.deleted_at,
        r.name, r.file_path, r.thumb_path, r.width, r.height, r.linked,
        s.name AS style, s.product, c.name AS category, c.cooldown_days
     FROM ref_library_images i
     JOIN ref_images r ON r.id = i.ref_image_id
     JOIN ref_library_styles s ON s.id = i.style_id
     JOIN ref_library_categories c ON c.id = i.category_id";

/// 每个（款式, 分类）的图片数：总数、启用数、此刻不在冷却期的启用数。
#[derive(Debug, Clone, FromRow)]
pub struct CountRow {
    pub style_id: i64,
    pub category_id: i64,
    pub total: i64,
    pub enabled: i64,
    pub available: i64,
}

pub async fn list_styles(pool: &SqlitePool) -> Result<Vec<StyleRow>, sqlx::Error> {
    sqlx::query_as("SELECT * FROM ref_library_styles ORDER BY product, name")
        .fetch_all(pool)
        .await
}

pub async fn get_style(pool: &SqlitePool, id: i64) -> Result<Option<StyleRow>, sqlx::Error> {
    sqlx::query_as("SELECT * FROM ref_library_styles WHERE id = ?1")
        .bind(id)
        .fetch_optional(pool)
        .await
}

pub async fn styles_named(pool: &SqlitePool, name: &str) -> Result<Vec<StyleRow>, sqlx::Error> {
    sqlx::query_as("SELECT * FROM ref_library_styles WHERE name = ?1 ORDER BY product")
        .bind(name)
        .fetch_all(pool)
        .await
}

pub async fn insert_style(
    pool: &SqlitePool,
    name: &str,
    product: &str,
    note: &str,
    now: i64,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "INSERT INTO ref_library_styles (name, product, note, created_at)
         VALUES (?1, ?2, ?3, ?4) RETURNING id",
    )
    .bind(name)
    .bind(product)
    .bind(note)
    .bind(now)
    .fetch_one(pool)
    .await
}

pub async fn update_style(
    pool: &SqlitePool,
    id: i64,
    name: &str,
    product: &str,
    note: &str,
) -> Result<bool, sqlx::Error> {
    let n = sqlx::query(
        "UPDATE ref_library_styles SET name = ?2, product = ?3, note = ?4 WHERE id = ?1",
    )
    .bind(id)
    .bind(name)
    .bind(product)
    .bind(note)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(n > 0)
}

/// 只删从没放过图的款式（含已移出的图）：使用台账还指着那些图。
pub async fn delete_empty_style(pool: &SqlitePool, id: i64) -> Result<bool, sqlx::Error> {
    let n = sqlx::query(
        "DELETE FROM ref_library_styles WHERE id = ?1
           AND NOT EXISTS (SELECT 1 FROM ref_library_images WHERE style_id = ?1)",
    )
    .bind(id)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(n > 0)
}

pub async fn list_categories(pool: &SqlitePool) -> Result<Vec<CategoryRow>, sqlx::Error> {
    sqlx::query_as("SELECT * FROM ref_library_categories ORDER BY sort_order, id")
        .fetch_all(pool)
        .await
}

pub async fn category_named(
    pool: &SqlitePool,
    name: &str,
) -> Result<Option<CategoryRow>, sqlx::Error> {
    sqlx::query_as("SELECT * FROM ref_library_categories WHERE name = ?1")
        .bind(name)
        .fetch_optional(pool)
        .await
}

pub async fn get_category(pool: &SqlitePool, id: i64) -> Result<Option<CategoryRow>, sqlx::Error> {
    sqlx::query_as("SELECT * FROM ref_library_categories WHERE id = ?1")
        .bind(id)
        .fetch_optional(pool)
        .await
}

pub async fn insert_category(
    pool: &SqlitePool,
    name: &str,
    cooldown_days: i64,
    sort_order: i64,
    now: i64,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "INSERT INTO ref_library_categories (name, cooldown_days, sort_order, created_at)
         VALUES (?1, ?2, ?3, ?4) RETURNING id",
    )
    .bind(name)
    .bind(cooldown_days)
    .bind(sort_order)
    .bind(now)
    .fetch_one(pool)
    .await
}

pub async fn update_category(
    pool: &SqlitePool,
    id: i64,
    name: &str,
    cooldown_days: i64,
    sort_order: i64,
) -> Result<bool, sqlx::Error> {
    let n = sqlx::query(
        "UPDATE ref_library_categories SET name = ?2, cooldown_days = ?3, sort_order = ?4
         WHERE id = ?1",
    )
    .bind(id)
    .bind(name)
    .bind(cooldown_days)
    .bind(sort_order)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(n > 0)
}

pub async fn delete_empty_category(pool: &SqlitePool, id: i64) -> Result<bool, sqlx::Error> {
    let n = sqlx::query(
        "DELETE FROM ref_library_categories WHERE id = ?1
           AND NOT EXISTS (SELECT 1 FROM ref_library_images WHERE category_id = ?1)",
    )
    .bind(id)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(n > 0)
}

pub async fn max_category_order(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT COALESCE(MAX(sort_order), 0) FROM ref_library_categories")
        .fetch_one(pool)
        .await
}

/// 按（款式, 分类）汇总未移出的图片。`now` 决定冷却期判断。
pub async fn counts(pool: &SqlitePool, now: i64) -> Result<Vec<CountRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT i.style_id, i.category_id, COUNT(*) AS total,
                COALESCE(SUM(i.enabled), 0) AS enabled,
                COALESCE(SUM(CASE WHEN i.enabled = 1 AND (i.last_used_at IS NULL
                    OR i.last_used_at <= ?1 - c.cooldown_days * 86400) THEN 1 ELSE 0 END), 0)
                  AS available
         FROM ref_library_images i
         JOIN ref_library_categories c ON c.id = i.category_id
         WHERE i.deleted_at IS NULL
         GROUP BY i.style_id, i.category_id",
    )
    .bind(now)
    .fetch_all(pool)
    .await
}

pub async fn find_by_hash(
    pool: &SqlitePool,
    style_id: i64,
    content_hash: &str,
) -> Result<Option<ImageRow>, sqlx::Error> {
    sqlx::query_as(&format!(
        "{IMAGE_SELECT} WHERE i.style_id = ?1 AND i.content_hash = ?2"
    ))
    .bind(style_id)
    .bind(content_hash)
    .fetch_optional(pool)
    .await
}

pub async fn insert_image(
    conn: &mut SqliteConnection,
    style_id: i64,
    category_id: i64,
    ref_image_id: i64,
    content_hash: &str,
    now: i64,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "INSERT INTO ref_library_images (style_id, category_id, ref_image_id, content_hash, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5) RETURNING id",
    )
    .bind(style_id)
    .bind(category_id)
    .bind(ref_image_id)
    .bind(content_hash)
    .bind(now)
    .fetch_one(conn)
    .await
}

/// 列图片。`style_id` / `category_id` 为空即不按它过滤；默认不含已移出的图。
pub async fn list_images(
    pool: &SqlitePool,
    style_id: Option<i64>,
    category_id: Option<i64>,
    include_deleted: bool,
) -> Result<Vec<ImageRow>, sqlx::Error> {
    sqlx::query_as(&format!(
        "{IMAGE_SELECT}
         WHERE (?1 IS NULL OR i.style_id = ?1) AND (?2 IS NULL OR i.category_id = ?2)
           AND (?3 OR i.deleted_at IS NULL)
         ORDER BY c.sort_order, i.created_at DESC, i.id DESC"
    ))
    .bind(style_id)
    .bind(category_id)
    .bind(include_deleted)
    .fetch_all(pool)
    .await
}

/// 把刚入库的 ref_images 行标成映射（0054）。与 insert 同事务。
pub async fn mark_linked(
    conn: &mut SqliteConnection,
    ref_image_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE ref_images SET linked = 1 WHERE id = ?1")
        .bind(ref_image_id)
        .execute(conn)
        .await?;
    Ok(())
}

/// 旧版本拷进来的副本 → 改指向用户原图。只改还是副本的行（`linked = 0`），返回改前的副本路径。
pub async fn relink(
    conn: &mut SqliteConnection,
    ref_image_id: i64,
    source: &str,
) -> Result<Option<String>, sqlx::Error> {
    let old: Option<String> =
        sqlx::query_scalar("SELECT file_path FROM ref_images WHERE id = ?1 AND linked = 0")
            .bind(ref_image_id)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(old) = old else { return Ok(None) };
    sqlx::query("UPDATE ref_images SET file_path = ?2, linked = 1 WHERE id = ?1 AND linked = 0")
        .bind(ref_image_id)
        .bind(source)
        .execute(&mut *conn)
        .await?;
    Ok(Some(old))
}

pub async fn get_image(
    conn: &mut SqliteConnection,
    id: i64,
) -> Result<Option<ImageRow>, sqlx::Error> {
    sqlx::query_as(&format!("{IMAGE_SELECT} WHERE i.id = ?1"))
        .bind(id)
        .fetch_optional(conn)
        .await
}

/// 随机抽取的候选：启用且未移出。
pub async fn candidates(
    conn: &mut SqliteConnection,
    style_id: i64,
    category_id: i64,
) -> Result<Vec<ImageRow>, sqlx::Error> {
    sqlx::query_as(&format!(
        "{IMAGE_SELECT}
         WHERE i.style_id = ?1 AND i.category_id = ?2 AND i.enabled = 1 AND i.deleted_at IS NULL
         ORDER BY i.id"
    ))
    .bind(style_id)
    .bind(category_id)
    .fetch_all(conn)
    .await
}

pub async fn set_category(
    conn: &mut SqliteConnection,
    id: i64,
    category_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE ref_library_images SET category_id = ?2 WHERE id = ?1")
        .bind(id)
        .bind(category_id)
        .execute(conn)
        .await?;
    Ok(())
}

pub async fn set_enabled(
    conn: &mut SqliteConnection,
    id: i64,
    enabled: bool,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE ref_library_images SET enabled = ?2 WHERE id = ?1")
        .bind(id)
        .bind(enabled)
        .execute(conn)
        .await?;
    Ok(())
}

pub async fn set_note(conn: &mut SqliteConnection, id: i64, note: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE ref_library_images SET note = ?2 WHERE id = ?1")
        .bind(id)
        .bind(note)
        .execute(conn)
        .await?;
    Ok(())
}

/// 移出 / 放回图库。`deleted_at = None` 即放回。
pub async fn set_deleted(
    conn: &mut SqliteConnection,
    id: i64,
    deleted_at: Option<i64>,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE ref_library_images SET deleted_at = ?2 WHERE id = ?1")
        .bind(id)
        .bind(deleted_at)
        .execute(conn)
        .await?;
    Ok(())
}

/// 已移出的同一张图被重新导入：放回并改到新分类。
pub async fn revive(
    conn: &mut SqliteConnection,
    id: i64,
    category_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE ref_library_images SET deleted_at = NULL, enabled = 1, category_id = ?2 WHERE id = ?1",
    )
    .bind(id)
    .bind(category_id)
    .execute(conn)
    .await?;
    Ok(())
}

pub struct Usage<'a> {
    pub image_id: i64,
    pub used_at: i64,
    pub source: &'a str,
    pub job_id: Option<&'a str>,
    pub item_name: Option<&'a str>,
    /// 条目内第几张参考图（1 起，与 Image 1、Image 2 对应）。
    pub slot: Option<i64>,
    /// 冷却期内或同单重复使用（可用图不够时的退路）。
    pub fallback: bool,
    /// 抽中时实际执行的冷却天数（降级后的那一档）；手动标记为空。
    pub cooldown_days: Option<i64>,
}

/// 记一次使用：台账一行 + 冗余的最近使用时间与次数，同一连接内完成。
pub async fn record_usage(conn: &mut SqliteConnection, u: &Usage<'_>) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO ref_library_usage
           (image_id, used_at, source, job_id, item_name, slot, fallback, cooldown_days)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )
    .bind(u.image_id)
    .bind(u.used_at)
    .bind(u.source)
    .bind(u.job_id)
    .bind(u.item_name)
    .bind(u.slot)
    .bind(u.fallback)
    .bind(u.cooldown_days)
    .execute(&mut *conn)
    .await?;
    sqlx::query(
        "UPDATE ref_library_images
         SET last_used_at = MAX(COALESCE(last_used_at, 0), ?2), use_count = use_count + 1
         WHERE id = ?1",
    )
    .bind(u.image_id)
    .bind(u.used_at)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

#[derive(Debug, Clone, FromRow)]
pub struct UsageRow {
    pub image_id: i64,
    pub used_at: i64,
    pub source: String,
    pub job_id: Option<String>,
    pub item_name: Option<String>,
    pub slot: Option<i64>,
    pub fallback: bool,
    pub cooldown_days: Option<i64>,
}

pub async fn usage_for_job(pool: &SqlitePool, job_id: &str) -> Result<Vec<UsageRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT image_id, used_at, source, job_id, item_name, slot, fallback, cooldown_days
         FROM ref_library_usage WHERE job_id = ?1 ORDER BY id",
    )
    .bind(job_id)
    .fetch_all(pool)
    .await
}

pub async fn usage_for_image(
    pool: &SqlitePool,
    image_id: i64,
    limit: i64,
) -> Result<Vec<UsageRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT image_id, used_at, source, job_id, item_name, slot, fallback, cooldown_days
         FROM ref_library_usage WHERE image_id = ?1 ORDER BY used_at DESC, id DESC LIMIT ?2",
    )
    .bind(image_id)
    .bind(limit)
    .fetch_all(pool)
    .await
}

/// 发给上游 / 导出时用的文件：有摆正或压缩过的上传副本就用它，否则用原图。
/// 与调度器 `ref_path` 同一口径。
pub async fn upload_source(
    conn: &mut SqliteConnection,
    ref_image_id: i64,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT COALESCE(upload_path, file_path) FROM ref_images WHERE id = ?1")
        .bind(ref_image_id)
        .fetch_optional(conn)
        .await
}
