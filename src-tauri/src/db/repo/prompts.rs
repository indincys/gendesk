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
    pub title: Option<String>,
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
    title: Option<&str>,
    text: &str,
    source: &str,
) -> Result<i64, sqlx::Error> {
    let now = now_unix();
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO prompts (group_id, code, title, text, source, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6) RETURNING id",
    )
    .bind(group_id)
    .bind(code)
    .bind(title)
    .bind(text)
    .bind(source)
    .bind(now)
    .fetch_one(&mut *conn)
    .await?;
    Ok(id)
}

/// 分组内全部 active 提示词（id + 正文），供批次展开。
pub async fn list_active_prompts(
    pool: &SqlitePool,
    group_id: i64,
) -> Result<Vec<(i64, String)>, sqlx::Error> {
    sqlx::query_as::<_, (i64, String)>(
        "SELECT id, text FROM prompts WHERE group_id = ?1 AND status = 'active' ORDER BY id ASC",
    )
    .bind(group_id)
    .fetch_all(pool)
    .await
}

/// 验收通过时写回微调文本并标记 edited（R8）。
pub async fn apply_edit(pool: &SqlitePool, prompt_id: i64, text: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE prompts SET text = ?2, edited = 1, updated_at = ?3 WHERE id = ?1")
        .bind(prompt_id)
        .bind(text)
        .bind(crate::db::now_unix())
        .execute(pool)
        .await?;
    Ok(())
}

/// 分组转正式（临时组首次验收通过，R7）。
pub async fn promote_group(pool: &SqlitePool, group_id: i64) -> Result<bool, sqlx::Error> {
    let affected =
        sqlx::query("UPDATE prompt_groups SET is_temp = 0 WHERE id = ?1 AND is_temp = 1")
            .bind(group_id)
            .execute(pool)
            .await?
            .rows_affected();
    Ok(affected > 0)
}

pub async fn list_by_group(
    pool: &SqlitePool,
    group_id: i64,
) -> Result<Vec<PromptRow>, sqlx::Error> {
    sqlx::query_as::<_, PromptRow>(
        "SELECT * FROM prompts WHERE group_id = ?1 AND status = 'active' ORDER BY id ASC",
    )
    .bind(group_id)
    .fetch_all(pool)
    .await
}

pub async fn search(pool: &SqlitePool, query: &str) -> Result<Vec<PromptRow>, sqlx::Error> {
    let like = format!("%{query}%");
    sqlx::query_as::<_, PromptRow>(
        "SELECT * FROM prompts WHERE status = 'active' AND (code LIKE ?1 OR text LIKE ?1)
         ORDER BY id ASC LIMIT 300",
    )
    .bind(like)
    .fetch_all(pool)
    .await
}

pub async fn get(pool: &SqlitePool, id: i64) -> Result<Option<PromptRow>, sqlx::Error> {
    sqlx::query_as::<_, PromptRow>("SELECT * FROM prompts WHERE id = ?1")
        .bind(id)
        .fetch_optional(pool)
        .await
}

pub async fn toggle_favorite(pool: &SqlitePool, id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE prompts SET favorite = 1 - favorite WHERE id = ?1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// 置 trash 状态，返回 (code, title, group_id) 供废纸篓快照与编号回收。
#[allow(clippy::type_complexity)]
pub async fn set_trash(
    pool: &SqlitePool,
    id: i64,
) -> Result<Option<(String, Option<String>, i64)>, sqlx::Error> {
    let row = get(pool, id).await?;
    if let Some(r) = &row {
        sqlx::query("UPDATE prompts SET status = 'trash', updated_at = ?2 WHERE id = ?1")
            .bind(id)
            .bind(crate::db::now_unix())
            .execute(pool)
            .await?;
        return Ok(Some((r.code.clone(), r.title.clone(), r.group_id)));
    }
    Ok(None)
}

pub async fn count_in_group(pool: &SqlitePool, group_id: i64) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM prompts WHERE group_id = ?1 AND status = 'active'",
    )
    .bind(group_id)
    .fetch_one(pool)
    .await
}
