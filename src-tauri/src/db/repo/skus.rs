//! SKU 档案数据仓（发布模块执行计划 §3）。
//!
//! 列表聚合查询一次出三池余量 + 最近发布时间；预警阈值判定在命令层（读设置）。

// 数据层 API 先于消费者落地；未使用项在对应任务接入后收紧。
#![allow(dead_code)]

use sqlx::{FromRow, SqlitePool};

use crate::db::now_unix;

/// SKU 档案行（对应 `skus` 表全列）。
#[derive(Debug, Clone, FromRow)]
pub struct SkuRow {
    pub id: i64,
    pub code: String,
    pub style_name: String,
    pub product_name: String,
    pub tier: String,
    pub topics_json: String,
    pub platforms_json: Option<String>,
    pub status: String,
    pub is_general: i64,
    pub note: String,
    /// 收件箱文件夹别名（空串=无别名），一对一映射到 code。
    pub folder_alias: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// SKU + 三池余量 + 最近发布（列表聚合，见执行计划 4.1 list_skus）。
#[derive(Debug, Clone, FromRow)]
pub struct SkuAggRow {
    pub id: i64,
    pub code: String,
    pub style_name: String,
    pub product_name: String,
    pub tier: String,
    pub topics_json: String,
    pub platforms_json: Option<String>,
    pub status: String,
    pub is_general: i64,
    pub note: String,
    pub folder_alias: String,
    pub created_at: i64,
    pub updated_at: i64,
    /// 可用素材包数（非退役）。
    pub material_count: i64,
    /// 启用标题条目数。
    pub title_count: i64,
    /// 启用正文条目数。
    pub body_count: i64,
    /// 可用图集包数（>0 时正文池才纳入预警）。
    pub gallery_count: i64,
    /// 最近发布时间（Unix 秒；无台账记录为 NULL）。
    pub last_published: Option<i64>,
}

/// 新建 SKU 输入。
pub struct NewSku {
    pub code: String,
    pub style_name: String,
    pub product_name: String,
    pub tier: String,
    pub topics_json: String,
    pub platforms_json: Option<String>,
    pub note: String,
}

const AGG_SELECT: &str = "SELECT s.id, s.code, s.style_name, s.product_name, s.tier,
        s.topics_json, s.platforms_json, s.status, s.is_general, s.note, s.folder_alias,
        s.created_at, s.updated_at,
        (SELECT COUNT(*) FROM asset_packs p WHERE p.sku_id = s.id AND p.lifecycle != 'retired') AS material_count,
        (SELECT COUNT(*) FROM text_items t WHERE t.sku_id = s.id AND t.kind = 'title' AND t.enabled = 1) AS title_count,
        (SELECT COUNT(*) FROM text_items t WHERE t.sku_id = s.id AND t.kind = 'body' AND t.enabled = 1) AS body_count,
        (SELECT COUNT(*) FROM asset_packs p WHERE p.sku_id = s.id AND p.kind = 'gallery' AND p.lifecycle != 'retired') AS gallery_count,
        (SELECT MAX(l.published_at) FROM usage_ledger l WHERE l.sku_id = s.id) AS last_published
    FROM skus s";

/// 全部 SKU 聚合行（通用分组置于最后；命令层再按 tier/预警/搜索过滤）。
pub async fn list_agg(pool: &SqlitePool) -> Result<Vec<SkuAggRow>, sqlx::Error> {
    sqlx::query_as::<_, SkuAggRow>(&format!(
        "{AGG_SELECT} ORDER BY s.is_general ASC, s.created_at DESC, s.id DESC"
    ))
    .fetch_all(pool)
    .await
}

/// 单个 SKU 聚合行。
pub async fn get_agg(pool: &SqlitePool, id: i64) -> Result<Option<SkuAggRow>, sqlx::Error> {
    sqlx::query_as::<_, SkuAggRow>(&format!("{AGG_SELECT} WHERE s.id = ?1"))
        .bind(id)
        .fetch_optional(pool)
        .await
}

pub async fn get(pool: &SqlitePool, id: i64) -> Result<Option<SkuRow>, sqlx::Error> {
    sqlx::query_as::<_, SkuRow>("SELECT * FROM skus WHERE id = ?1")
        .bind(id)
        .fetch_optional(pool)
        .await
}

/// 按编码查库（大小写不敏感）。Windows 文件系统大小写不敏感，`sf-1` 与 `SF-1` 在
/// 资产库里是同一个目录，故编码唯一性与查找一律 NOCASE（唯一索引 idx_skus_code_nocase）。
pub async fn find_by_code(pool: &SqlitePool, code: &str) -> Result<Option<SkuRow>, sqlx::Error> {
    sqlx::query_as::<_, SkuRow>("SELECT * FROM skus WHERE code = ?1 COLLATE NOCASE")
        .bind(code)
        .fetch_optional(pool)
        .await
}

/// 按收件箱文件夹别名精确查库（空 token 返回 None，避免匹配到空别名行）。
/// NOCASE：别名多为中文（SQLite NOCASE 只折叠 ASCII），ASCII 别名同样大小写不敏感。
pub async fn find_by_alias(pool: &SqlitePool, alias: &str) -> Result<Option<SkuRow>, sqlx::Error> {
    let alias = alias.trim();
    if alias.is_empty() {
        return Ok(None);
    }
    sqlx::query_as::<_, SkuRow>("SELECT * FROM skus WHERE folder_alias = ?1 COLLATE NOCASE")
        .bind(alias)
        .fetch_optional(pool)
        .await
}

/// 先按编码、未命中再按别名查库（收件箱识别 SKU 归属用）。
pub async fn find_by_code_or_alias(
    pool: &SqlitePool,
    token: &str,
) -> Result<Option<SkuRow>, sqlx::Error> {
    if let Some(row) = find_by_code(pool, token).await? {
        return Ok(Some(row));
    }
    find_by_alias(pool, token).await
}

/// 设置/清除文件夹别名（空串=清除）。唯一性由分区唯一索引 + 命令层预检共同保证。
pub async fn set_alias(pool: &SqlitePool, id: i64, alias: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE skus SET folder_alias = ?2, updated_at = ?3 WHERE id = ?1")
        .bind(id)
        .bind(alias.trim())
        .bind(now_unix())
        .execute(pool)
        .await?;
    Ok(())
}

/// 内置「通用」分组 id。
pub async fn general_id(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar::<_, i64>("SELECT id FROM skus WHERE is_general = 1 LIMIT 1")
        .fetch_one(pool)
        .await
}

pub async fn insert(pool: &SqlitePool, input: &NewSku) -> Result<i64, sqlx::Error> {
    let now = now_unix();
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO skus (code, style_name, product_name, tier, topics_json, platforms_json, note, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8) RETURNING id",
    )
    .bind(&input.code)
    .bind(&input.style_name)
    .bind(&input.product_name)
    .bind(&input.tier)
    .bind(&input.topics_json)
    .bind(&input.platforms_json)
    .bind(&input.note)
    .bind(now)
    .fetch_one(pool)
    .await
}

/// 部分更新档案字段（None 保持不变）。
#[allow(clippy::too_many_arguments)]
pub async fn update_fields(
    pool: &SqlitePool,
    id: i64,
    style_name: Option<&str>,
    product_name: Option<&str>,
    tier: Option<&str>,
    topics_json: Option<&str>,
    platforms_json: Option<Option<&str>>,
    note: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE skus SET
            style_name     = COALESCE(?2, style_name),
            product_name   = COALESCE(?3, product_name),
            tier           = COALESCE(?4, tier),
            topics_json    = COALESCE(?5, topics_json),
            platforms_json = CASE WHEN ?6 = 1 THEN ?7 ELSE platforms_json END,
            note           = COALESCE(?8, note),
            updated_at     = ?9
         WHERE id = ?1",
    )
    .bind(id)
    .bind(style_name)
    .bind(product_name)
    .bind(tier)
    .bind(topics_json)
    .bind(platforms_json.is_some() as i64)
    .bind(platforms_json.flatten())
    .bind(note)
    .bind(now_unix())
    .execute(pool)
    .await?;
    Ok(())
}

/// 设置状态（active|paused），通用分组不允许停发。
pub async fn set_status(pool: &SqlitePool, id: i64, status: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE skus SET status = ?2, updated_at = ?3 WHERE id = ?1 AND is_general = 0")
        .bind(id)
        .bind(status)
        .bind(now_unix())
        .execute(pool)
        .await?;
    Ok(())
}

/// 仅更新话题标签 JSON（收件箱采纳前 5 个话题时用）。
pub async fn set_topics(pool: &SqlitePool, id: i64, topics_json: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE skus SET topics_json = ?2, updated_at = ?3 WHERE id = ?1")
        .bind(id)
        .bind(topics_json)
        .bind(now_unix())
        .execute(pool)
        .await?;
    Ok(())
}
