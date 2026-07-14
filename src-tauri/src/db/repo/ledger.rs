//! 使用台账数据仓（usage_ledger）。套装粒度、冗余展开列，查重窗口判定不 join。
//! P1 无写入（对账在 P3 接入）；派生查询在此就位，供素材生命周期与发布历史使用。

#![allow(dead_code)]

use sqlx::{FromRow, SqliteConnection, SqlitePool};

#[derive(Debug, Clone, FromRow)]
pub struct LedgerRow {
    pub id: i64,
    pub date: String,
    pub sku_id: i64,
    pub pack_id: i64,
    pub title_id: i64,
    pub body_id: Option<i64>,
    pub platform: String,
    pub account_id: i64,
    pub task_code: String,
    pub published_at: i64,
    pub url: Option<String>,
}

pub struct NewLedger {
    pub date: String,
    pub sku_id: i64,
    pub pack_id: i64,
    pub title_id: i64,
    pub body_id: Option<i64>,
    pub platform: String,
    pub account_id: i64,
    pub task_code: String,
    pub published_at: i64,
    pub url: Option<String>,
}

pub async fn insert_conn(conn: &mut SqliteConnection, r: &NewLedger) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO usage_ledger
           (date, sku_id, pack_id, title_id, body_id, platform, account_id, task_code, published_at, url)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) RETURNING id",
    )
    .bind(&r.date)
    .bind(r.sku_id)
    .bind(r.pack_id)
    .bind(r.title_id)
    .bind(r.body_id)
    .bind(&r.platform)
    .bind(r.account_id)
    .bind(&r.task_code)
    .bind(r.published_at)
    .bind(&r.url)
    .fetch_one(&mut *conn)
    .await
}

/// 某 SKU 的发布历史（SKU 详情分页读）。
pub async fn history_by_sku(
    pool: &SqlitePool,
    sku_id: i64,
    limit: i64,
    offset: i64,
) -> Result<Vec<LedgerRow>, sqlx::Error> {
    sqlx::query_as::<_, LedgerRow>(
        "SELECT * FROM usage_ledger WHERE sku_id = ?1
         ORDER BY published_at DESC, id DESC LIMIT ?2 OFFSET ?3",
    )
    .bind(sku_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
}

/// 查重窗口占用：某素材包在某平台、`since`（Unix 秒）之后的最近一次发布时间。
/// 用于「已用尽/回可用日期」派生（前置事实 5/6）。
pub async fn last_publish_in_window(
    conn: &mut SqliteConnection,
    pack_id: i64,
    platform: &str,
    since: i64,
) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar::<_, Option<i64>>(
        "SELECT MAX(published_at) FROM usage_ledger
         WHERE pack_id = ?1 AND platform = ?2 AND published_at >= ?3",
    )
    .bind(pack_id)
    .bind(platform)
    .bind(since)
    .fetch_one(&mut *conn)
    .await
}
