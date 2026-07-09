//! 参考图数据仓。

// 数据层 API 先于 M3 消费者落地；未使用项在对应里程碑接入后收紧。
#![allow(dead_code)]

use sqlx::{FromRow, SqlitePool};

use crate::db::now_unix;

#[derive(Debug, Clone, FromRow)]
pub struct RefImageRow {
    pub id: i64,
    pub name: String,
    pub group_id: Option<i64>,
    pub file_path: String,
    pub thumb_path: String,
    pub width: i64,
    pub height: i64,
    pub file_size: i64,
    pub created_at: i64,
    pub deleted_at: Option<i64>,
}

pub struct NewRefImage {
    pub name: String,
    pub group_id: Option<i64>,
    pub file_path: String,
    pub thumb_path: String,
    pub width: i64,
    pub height: i64,
    pub file_size: i64,
}

pub async fn insert(pool: &SqlitePool, r: &NewRefImage) -> Result<i64, sqlx::Error> {
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO ref_images (name, group_id, file_path, thumb_path, width, height, file_size, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) RETURNING id",
    )
    .bind(&r.name)
    .bind(r.group_id)
    .bind(&r.file_path)
    .bind(&r.thumb_path)
    .bind(r.width)
    .bind(r.height)
    .bind(r.file_size)
    .bind(now_unix())
    .fetch_one(pool)
    .await?;
    Ok(id)
}

pub async fn list_active(pool: &SqlitePool) -> Result<Vec<RefImageRow>, sqlx::Error> {
    sqlx::query_as::<_, RefImageRow>(
        "SELECT * FROM ref_images WHERE deleted_at IS NULL ORDER BY created_at DESC, id DESC",
    )
    .fetch_all(pool)
    .await
}

pub async fn set_group(
    pool: &SqlitePool,
    id: i64,
    group_id: Option<i64>,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE ref_images SET group_id = ?2 WHERE id = ?1")
        .bind(id)
        .bind(group_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn get(pool: &SqlitePool, id: i64) -> Result<Option<RefImageRow>, sqlx::Error> {
    sqlx::query_as::<_, RefImageRow>("SELECT * FROM ref_images WHERE id = ?1")
        .bind(id)
        .fetch_optional(pool)
        .await
}

/// 软删除（deleted_at），返回原行供搬运文件进废纸篓。
pub async fn soft_delete(pool: &SqlitePool, id: i64) -> Result<Option<RefImageRow>, sqlx::Error> {
    let row = get(pool, id).await?;
    if row.is_some() {
        sqlx::query("UPDATE ref_images SET deleted_at = ?2 WHERE id = ?1")
            .bind(id)
            .bind(now_unix())
            .execute(pool)
            .await?;
    }
    Ok(row)
}

pub async fn update_file(
    pool: &SqlitePool,
    id: i64,
    file_path: &str,
    thumb_path: &str,
    width: i64,
    height: i64,
    file_size: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE ref_images SET file_path = ?2, thumb_path = ?3, width = ?4, height = ?5, file_size = ?6 WHERE id = ?1",
    )
    .bind(id)
    .bind(file_path)
    .bind(thumb_path)
    .bind(width)
    .bind(height)
    .bind(file_size)
    .execute(pool)
    .await?;
    Ok(())
}

/// 使用次数（batch_refs）与产出通过作品数（accepted_works）。
pub async fn usage_stats(pool: &SqlitePool, id: i64) -> Result<(i64, i64), sqlx::Error> {
    let used: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM batch_refs WHERE ref_image_id = ?1")
        .bind(id)
        .fetch_one(pool)
        .await?;
    let works: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM accepted_works WHERE ref_image_id = ?1")
            .bind(id)
            .fetch_one(pool)
            .await?;
    Ok((used, works))
}
