//! 设置持久化（key='app' 的单行 JSON）。类型化逻辑在 commands/settings.rs。

use sqlx::SqlitePool;

const KEY: &str = "app";

/// 读取设置 JSON 原文（不存在返回 None）。
pub async fn get_raw(pool: &SqlitePool) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(String,)> = sqlx::query_as("SELECT value_json FROM settings WHERE key = ?1")
        .bind(KEY)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|(v,)| v))
}

/// 写入设置 JSON 原文（upsert）。
pub async fn set_raw(pool: &SqlitePool, json: &str) -> Result<(), sqlx::Error> {
    set_by_key(pool, KEY, json).await
}

/// 读取任意 key 的设置 JSON（发布模块用 key='publish'）。
pub async fn get_by_key(pool: &SqlitePool, key: &str) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(String,)> = sqlx::query_as("SELECT value_json FROM settings WHERE key = ?1")
        .bind(key)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|(v,)| v))
}

/// 写入任意 key 的设置 JSON（upsert）。
pub async fn set_by_key(pool: &SqlitePool, key: &str, json: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO settings (key, value_json) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json",
    )
    .bind(key)
    .bind(json)
    .execute(pool)
    .await?;
    Ok(())
}
