//! 通过作品数据仓（accepted_works，快照式冗余）。

#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqliteConnection, SqlitePool};

use crate::db::now_unix;

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct AcceptedWorkRow {
    pub id: i64,
    /// 0008 起可空：批次被清理后作品仍在，只是不再指向任何任务。
    /// 提示词成为消耗品之后这会是**常态**而不是例外。
    pub task_id: Option<i64>,
    pub image_path: String,
    pub thumb_path: String,
    pub prompt_id: Option<i64>,
    pub prompt_text: String,
    pub group_id: Option<i64>,
    pub ref_image_id: Option<i64>,
    pub batch_id: Option<i64>,
    pub favorite: i64,
    pub accepted_at: i64,
    /// 编号与组名的快照（0027）。提示词是消耗品，会随批次一起删掉——
    /// 现读 prompts/prompt_groups 的话，作品会在上游被清理的那一刻丢掉自己的身份。
    #[serde(default)]
    pub prompt_code: String,
    #[serde(default)]
    pub group_name: String,
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
    pub prompt_code: String,
    pub group_name: String,
}

pub async fn insert(conn: &mut SqliteConnection, w: &NewWork) -> Result<i64, sqlx::Error> {
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO accepted_works (task_id, image_path, thumb_path, prompt_id, prompt_text,
            group_id, ref_image_id, batch_id, accepted_at, prompt_code, group_name)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11) RETURNING id",
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
    .bind(&w.prompt_code)
    .bind(&w.group_name)
    .fetch_one(&mut *conn)
    .await?;
    Ok(id)
}

/// 整行 → 废纸篓还原载荷（0027）。序列化失败返回 None：那只会让这一条无法还原，
/// 不该连带把「删除」这个动作一起弄失败。
pub fn to_payload(row: &AcceptedWorkRow) -> Option<String> {
    serde_json::to_string(row).ok()
}

/// 从废纸篓载荷把作品写回原位（0027）。
///
/// **连 id 一起写回**：v2v_clips.work_id 是不设 FK 的锚点（0020），换个新 id 等于把
/// 那条视频认领给了别人。id 在删除时就空出来了，除非期间有人手工塞了行，那种情况
/// INSERT 会自己撞主键失败——比静默换 id 好。
pub async fn restore(pool: &SqlitePool, row: &AcceptedWorkRow) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO accepted_works (id, task_id, image_path, thumb_path, prompt_id, prompt_text,
            group_id, ref_image_id, batch_id, favorite, accepted_at, prompt_code, group_name)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
    )
    .bind(row.id)
    .bind(row.task_id)
    .bind(&row.image_path)
    .bind(&row.thumb_path)
    .bind(row.prompt_id)
    .bind(&row.prompt_text)
    .bind(row.group_id)
    .bind(row.ref_image_id)
    .bind(row.batch_id)
    .bind(row.favorite)
    .bind(row.accepted_at)
    .bind(&row.prompt_code)
    .bind(&row.group_name)
    .execute(pool)
    .await?;
    Ok(())
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
