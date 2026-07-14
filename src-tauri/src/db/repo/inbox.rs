//! 收件箱收录记录数据仓（inbox_items）。
//! state：ingested（成功归档）/ unclaimed（未知 SKU 待认领）/ failed（解析失败待人工确认）。

#![allow(dead_code)]

use sqlx::{FromRow, SqliteConnection, SqlitePool};

use crate::db::now_unix;

#[derive(Debug, Clone, FromRow)]
pub struct InboxItemRow {
    pub id: i64,
    pub file_rel: String,
    pub kind: Option<String>,
    pub sku_code: Option<String>,
    pub state: String,
    pub detail_json: Option<String>,
    pub created_at: i64,
}

pub struct NewInboxItem {
    pub file_rel: String,
    pub kind: Option<String>,
    pub sku_code: Option<String>,
    pub state: String,
    pub detail_json: Option<String>,
}

/// 按状态列出（state 为空则全部），新→旧。
pub async fn list(
    pool: &SqlitePool,
    state: Option<&str>,
) -> Result<Vec<InboxItemRow>, sqlx::Error> {
    match state {
        Some(s) => {
            sqlx::query_as::<_, InboxItemRow>(
                "SELECT * FROM inbox_items WHERE state = ?1 ORDER BY created_at DESC, id DESC",
            )
            .bind(s)
            .fetch_all(pool)
            .await
        }
        None => {
            sqlx::query_as::<_, InboxItemRow>(
                "SELECT * FROM inbox_items ORDER BY created_at DESC, id DESC",
            )
            .fetch_all(pool)
            .await
        }
    }
}

pub async fn get(pool: &SqlitePool, id: i64) -> Result<Option<InboxItemRow>, sqlx::Error> {
    sqlx::query_as::<_, InboxItemRow>("SELECT * FROM inbox_items WHERE id = ?1")
        .bind(id)
        .fetch_optional(pool)
        .await
}

/// 已存在同 file_rel 的记录（重复事件去重）。
pub async fn find_by_rel(
    pool: &SqlitePool,
    file_rel: &str,
) -> Result<Option<InboxItemRow>, sqlx::Error> {
    sqlx::query_as::<_, InboxItemRow>("SELECT * FROM inbox_items WHERE file_rel = ?1")
        .bind(file_rel)
        .fetch_optional(pool)
        .await
}

pub async fn insert_conn(
    conn: &mut SqliteConnection,
    input: &NewInboxItem,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO inbox_items (file_rel, kind, sku_code, state, detail_json, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6) RETURNING id",
    )
    .bind(&input.file_rel)
    .bind(&input.kind)
    .bind(&input.sku_code)
    .bind(&input.state)
    .bind(&input.detail_json)
    .bind(now_unix())
    .fetch_one(&mut *conn)
    .await
}

pub async fn insert(pool: &SqlitePool, input: &NewInboxItem) -> Result<i64, sqlx::Error> {
    insert_conn(&mut *pool.acquire().await?, input).await
}

/// 待认领 / 解析失败计数（资产库徽章）。
pub async fn count_pending(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT COUNT(*) FROM inbox_items WHERE state IN ('unclaimed','failed')")
        .fetch_one(pool)
        .await
}

pub async fn delete(pool: &SqlitePool, id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM inbox_items WHERE id = ?1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}
