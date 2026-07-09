//! 通过作品数据仓（accepted_works，快照式冗余）。

#![allow(dead_code)]

use sqlx::{FromRow, SqliteConnection, SqlitePool};

use crate::db::now_unix;

#[derive(Debug, Clone, FromRow)]
pub struct AcceptedWorkRow {
    pub id: i64,
    pub task_id: i64,
    pub image_path: String,
    pub thumb_path: String,
    pub prompt_id: Option<i64>,
    pub prompt_text: String,
    pub group_id: Option<i64>,
    pub ref_image_id: Option<i64>,
    pub batch_id: Option<i64>,
    pub favorite: i64,
    pub accepted_at: i64,
}

pub struct NewWork {
    pub task_id: i64,
    pub image_path: String,
    pub thumb_path: String,
    pub prompt_id: i64,
    pub prompt_text: String,
    pub group_id: Option<i64>,
    pub ref_image_id: i64,
    pub batch_id: i64,
}

pub async fn insert(conn: &mut SqliteConnection, w: &NewWork) -> Result<i64, sqlx::Error> {
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO accepted_works (task_id, image_path, thumb_path, prompt_id, prompt_text,
            group_id, ref_image_id, batch_id, accepted_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) RETURNING id",
    )
    .bind(w.task_id)
    .bind(&w.image_path)
    .bind(&w.thumb_path)
    .bind(w.prompt_id)
    .bind(&w.prompt_text)
    .bind(w.group_id)
    .bind(w.ref_image_id)
    .bind(w.batch_id)
    .bind(now_unix())
    .fetch_one(&mut *conn)
    .await?;
    Ok(id)
}

pub async fn get(pool: &SqlitePool, id: i64) -> Result<Option<AcceptedWorkRow>, sqlx::Error> {
    sqlx::query_as::<_, AcceptedWorkRow>("SELECT * FROM accepted_works WHERE id = ?1")
        .bind(id)
        .fetch_optional(pool)
        .await
}

pub async fn toggle_favorite(pool: &SqlitePool, id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE accepted_works SET favorite = 1 - favorite WHERE id = ?1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// 删除作品记录，返回其行（供上层搬运文件进废纸篓）。
pub async fn delete(pool: &SqlitePool, id: i64) -> Result<Option<AcceptedWorkRow>, sqlx::Error> {
    let row = get(pool, id).await?;
    if row.is_some() {
        sqlx::query("DELETE FROM accepted_works WHERE id = ?1")
            .bind(id)
            .execute(pool)
            .await?;
    }
    Ok(row)
}

pub async fn count(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM accepted_works")
        .fetch_one(pool)
        .await
}
