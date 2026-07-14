//! 素材包数据仓（asset_packs）。生命周期存储态只有 new|active|retired；
//! 「已用尽/冷却中/回可用」为派生态，由使用台账在查询时计算，不落库。

#![allow(dead_code)]

use sqlx::{FromRow, SqliteConnection, SqlitePool};

use crate::db::now_unix;

#[derive(Debug, Clone, FromRow)]
pub struct PackRow {
    pub id: i64,
    pub sku_id: i64,
    pub kind: String,
    pub dir_rel: String,
    pub files_json: String,
    pub cover: Option<String>,
    pub lifecycle: String,
    pub source: String,
    pub note: String,
    pub created_at: i64,
    pub updated_at: i64,
}

pub struct NewPack {
    pub sku_id: i64,
    pub kind: String,
    pub dir_rel: String,
    pub files_json: String,
    pub cover: Option<String>,
    pub source: String,
}

pub async fn list_by_sku(pool: &SqlitePool, sku_id: i64) -> Result<Vec<PackRow>, sqlx::Error> {
    sqlx::query_as::<_, PackRow>(
        "SELECT * FROM asset_packs WHERE sku_id = ?1 ORDER BY created_at DESC, id DESC",
    )
    .bind(sku_id)
    .fetch_all(pool)
    .await
}

pub async fn get(pool: &SqlitePool, id: i64) -> Result<Option<PackRow>, sqlx::Error> {
    sqlx::query_as::<_, PackRow>("SELECT * FROM asset_packs WHERE id = ?1")
        .bind(id)
        .fetch_optional(pool)
        .await
}

/// 是否存在同 dir_rel 的包（归集去重）。
pub async fn find_by_dir(pool: &SqlitePool, dir_rel: &str) -> Result<Option<PackRow>, sqlx::Error> {
    sqlx::query_as::<_, PackRow>("SELECT * FROM asset_packs WHERE dir_rel = ?1")
        .bind(dir_rel)
        .fetch_optional(pool)
        .await
}

pub async fn insert(pool: &SqlitePool, input: &NewPack) -> Result<i64, sqlx::Error> {
    insert_conn(&mut *pool.acquire().await?, input).await
}

/// 事务/连接内插入（收录管线用）。
///
/// lifecycle 初值 **active**：文件齐备即可发（封面本就可选），入库即参与排期。
/// 存储态仍保留 `new`，留给未来需要人工过目的来源——写入方显式指定即可，
/// UI 上 new 包有「标为可用」按钮转 active。
pub async fn insert_conn(conn: &mut SqliteConnection, input: &NewPack) -> Result<i64, sqlx::Error> {
    let now = now_unix();
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO asset_packs (sku_id, kind, dir_rel, files_json, cover, lifecycle, source, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 'active', ?6, ?7, ?7) RETURNING id",
    )
    .bind(input.sku_id)
    .bind(&input.kind)
    .bind(&input.dir_rel)
    .bind(&input.files_json)
    .bind(&input.cover)
    .bind(&input.source)
    .bind(now)
    .fetch_one(&mut *conn)
    .await
}

pub async fn set_lifecycle(pool: &SqlitePool, id: i64, lifecycle: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE asset_packs SET lifecycle = ?2, updated_at = ?3 WHERE id = ?1")
        .bind(id)
        .bind(lifecycle)
        .bind(now_unix())
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn update_fields(
    pool: &SqlitePool,
    id: i64,
    note: Option<&str>,
    cover: Option<Option<&str>>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE asset_packs SET
            note  = COALESCE(?2, note),
            cover = CASE WHEN ?3 = 1 THEN ?4 ELSE cover END,
            updated_at = ?5
         WHERE id = ?1",
    )
    .bind(id)
    .bind(note)
    .bind(cover.is_some() as i64)
    .bind(cover.flatten())
    .bind(now_unix())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn delete(pool: &SqlitePool, id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM asset_packs WHERE id = ?1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// 是否被状态 ≠ closed 的任务单引用（锁定判定，前置事实 7）。P2 起 daily_sets/publish_tasks
/// 填充后生效；P1 无引用恒返回 false。
pub async fn is_locked(pool: &SqlitePool, pack_id: i64) -> Result<bool, sqlx::Error> {
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM publish_tasks pt
         JOIN daily_sets ds ON ds.id = pt.set_id
         JOIN task_sheets s ON s.id = pt.sheet_id
         WHERE ds.pack_id = ?1 AND s.status != 'closed'",
    )
    .bind(pack_id)
    .fetch_one(pool)
    .await?;
    Ok(n > 0)
}
