//! 提示词分组 / 提示词数据仓（与参考图库共用分组）。

// 数据层 API 先于 M3 消费者落地；未使用项在对应里程碑接入后收紧。
#![allow(dead_code)]

use sqlx::{FromRow, SqliteConnection, SqlitePool};

use crate::db::now_unix;

#[derive(Debug, Clone, FromRow)]
pub struct GroupRow {
    pub id: i64,
    pub name: String,
    pub prefix: String,
    pub scene: String,
    pub is_temp: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone, FromRow)]
pub struct PromptRow {
    pub id: i64,
    pub group_id: i64,
    pub code: String,
    pub text: String,
    pub favorite: i64,
    pub edited: i64,
    pub status: String,
    pub source: String,
    pub created_at: i64,
    pub updated_at: i64,
}

pub async fn list_groups(pool: &SqlitePool) -> Result<Vec<GroupRow>, sqlx::Error> {
    sqlx::query_as::<_, GroupRow>("SELECT * FROM prompt_groups ORDER BY created_at ASC, id ASC")
        .fetch_all(pool)
        .await
}

pub async fn find_group_by_prefix(
    pool: &SqlitePool,
    prefix: &str,
) -> Result<Option<GroupRow>, sqlx::Error> {
    sqlx::query_as::<_, GroupRow>("SELECT * FROM prompt_groups WHERE prefix = ?1")
        .bind(prefix)
        .fetch_optional(pool)
        .await
}

/// 在事务中创建分组，返回其 id。
pub async fn create_group(
    conn: &mut SqliteConnection,
    name: &str,
    prefix: &str,
    scene: &str,
    is_temp: bool,
) -> Result<i64, sqlx::Error> {
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO prompt_groups (name, prefix, scene, is_temp, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5) RETURNING id",
    )
    .bind(name)
    .bind(prefix)
    .bind(scene)
    .bind(is_temp as i64)
    .bind(now_unix())
    .fetch_one(&mut *conn)
    .await?;
    Ok(id)
}

/// 在事务中插入一条提示词。
pub async fn insert_prompt(
    conn: &mut SqliteConnection,
    group_id: i64,
    code: &str,
    text: &str,
    source: &str,
) -> Result<i64, sqlx::Error> {
    let now = now_unix();
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO prompts (group_id, code, text, source, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5) RETURNING id",
    )
    .bind(group_id)
    .bind(code)
    .bind(text)
    .bind(source)
    .bind(now)
    .fetch_one(&mut *conn)
    .await?;
    Ok(id)
}

pub async fn count_in_group(pool: &SqlitePool, group_id: i64) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM prompts WHERE group_id = ?1 AND status = 'active'",
    )
    .bind(group_id)
    .fetch_one(pool)
    .await
}
