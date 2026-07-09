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
