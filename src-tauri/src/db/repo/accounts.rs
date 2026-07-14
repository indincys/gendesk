//! 账号档案数据仓（accounts）。P2 编排消费；P1 建骨架就位。

#![allow(dead_code)]

use sqlx::{FromRow, SqlitePool};

use crate::db::now_unix;

#[derive(Debug, Clone, FromRow)]
pub struct AccountRow {
    pub id: i64,
    pub platform: String,
    pub name: String,
    pub daily_limit: i64,
    pub slots_json: Option<String>,
    pub status: String,
    pub created_at: i64,
}

pub struct NewAccount {
    pub platform: String,
    pub name: String,
    pub daily_limit: i64,
    pub slots_json: Option<String>,
}

pub async fn list(pool: &SqlitePool) -> Result<Vec<AccountRow>, sqlx::Error> {
    sqlx::query_as::<_, AccountRow>("SELECT * FROM accounts ORDER BY platform ASC, name ASC")
        .fetch_all(pool)
        .await
}

pub async fn insert(pool: &SqlitePool, input: &NewAccount) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO accounts (platform, name, daily_limit, slots_json, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5) RETURNING id",
    )
    .bind(&input.platform)
    .bind(&input.name)
    .bind(input.daily_limit)
    .bind(&input.slots_json)
    .bind(now_unix())
    .fetch_one(pool)
    .await
}

pub async fn update_fields(
    pool: &SqlitePool,
    id: i64,
    name: Option<&str>,
    daily_limit: Option<i64>,
    slots_json: Option<Option<&str>>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE accounts SET
            name        = COALESCE(?2, name),
            daily_limit = COALESCE(?3, daily_limit),
            slots_json  = CASE WHEN ?4 = 1 THEN ?5 ELSE slots_json END
         WHERE id = ?1",
    )
    .bind(id)
    .bind(name)
    .bind(daily_limit)
    .bind(slots_json.is_some() as i64)
    .bind(slots_json.flatten())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn set_status(pool: &SqlitePool, id: i64, status: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE accounts SET status = ?2 WHERE id = ?1")
        .bind(id)
        .bind(status)
        .execute(pool)
        .await?;
    Ok(())
}

/// 物理删除账号（引用校验在命令层：有历史任务的账号不可删，只能停用）。
pub async fn delete(pool: &SqlitePool, id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM accounts WHERE id = ?1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}
