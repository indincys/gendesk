//! 标题池 / 正文池数据仓（合表 text_items，见执行计划前置事实 20）。

#![allow(dead_code)]

use sqlx::{FromRow, SqliteConnection, SqlitePool};

use crate::db::now_unix;

#[derive(Debug, Clone, FromRow)]
pub struct TextItemRow {
    pub id: i64,
    pub sku_id: i64,
    pub kind: String,
    pub text: String,
    pub platform: String,
    pub source: String,
    pub enabled: i64,
    pub use_count: i64,
    pub created_at: i64,
}

pub struct NewTextItem {
    pub sku_id: i64,
    pub kind: String,
    pub text: String,
    pub platform: String,
    pub source: String,
}

/// 列出某 SKU 某类型（title|body）的条目，新→旧。
pub async fn list(
    pool: &SqlitePool,
    sku_id: i64,
    kind: &str,
) -> Result<Vec<TextItemRow>, sqlx::Error> {
    sqlx::query_as::<_, TextItemRow>(
        "SELECT * FROM text_items WHERE sku_id = ?1 AND kind = ?2 ORDER BY created_at DESC, id DESC",
    )
    .bind(sku_id)
    .bind(kind)
    .fetch_all(pool)
    .await
}

/// 启用中的条目（套装选取用）。
pub async fn list_enabled(
    conn: &mut SqliteConnection,
    sku_id: i64,
    kind: &str,
) -> Result<Vec<TextItemRow>, sqlx::Error> {
    sqlx::query_as::<_, TextItemRow>(
        "SELECT * FROM text_items WHERE sku_id = ?1 AND kind = ?2 AND enabled = 1
         ORDER BY use_count ASC, id ASC",
    )
    .bind(sku_id)
    .bind(kind)
    .fetch_all(&mut *conn)
    .await
}

pub async fn insert(pool: &SqlitePool, input: &NewTextItem) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO text_items (sku_id, kind, text, platform, source, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6) RETURNING id",
    )
    .bind(input.sku_id)
    .bind(&input.kind)
    .bind(&input.text)
    .bind(&input.platform)
    .bind(&input.source)
    .bind(now_unix())
    .fetch_one(pool)
    .await
}

/// 事务内插入（收录管线用）。
pub async fn insert_tx(
    conn: &mut SqliteConnection,
    input: &NewTextItem,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO text_items (sku_id, kind, text, platform, source, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6) RETURNING id",
    )
    .bind(input.sku_id)
    .bind(&input.kind)
    .bind(&input.text)
    .bind(&input.platform)
    .bind(&input.source)
    .bind(now_unix())
    .fetch_one(&mut *conn)
    .await
}

pub async fn update_fields(
    pool: &SqlitePool,
    id: i64,
    text: Option<&str>,
    platform: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE text_items SET
            text     = COALESCE(?2, text),
            platform = COALESCE(?3, platform)
         WHERE id = ?1",
    )
    .bind(id)
    .bind(text)
    .bind(platform)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn set_enabled(pool: &SqlitePool, id: i64, enabled: bool) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE text_items SET enabled = ?2 WHERE id = ?1")
        .bind(id)
        .bind(enabled as i64)
        .execute(pool)
        .await?;
    Ok(())
}

/// 使用计数 +1（套装被采纳发布时）。
pub async fn bump_use_count(conn: &mut SqliteConnection, id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE text_items SET use_count = use_count + 1 WHERE id = ?1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// 物理删除文本条目（引用校验在命令层：被套装引用的不可删，只能停用）。
pub async fn delete(pool: &SqlitePool, id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM text_items WHERE id = ?1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// 同 SKU、同类型、同文本是否已存在（入库查重：AI 反复生成同一句是常态）。
pub async fn exists_same(
    conn: &mut SqliteConnection,
    sku_id: i64,
    kind: &str,
    text: &str,
) -> Result<bool, sqlx::Error> {
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM text_items WHERE sku_id = ?1 AND kind = ?2 AND text = ?3",
    )
    .bind(sku_id)
    .bind(kind)
    .bind(text)
    .fetch_one(&mut *conn)
    .await?;
    Ok(n > 0)
}
