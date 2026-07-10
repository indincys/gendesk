//! API Key 数据仓（技术文档 5.1 / 执行计划 1.4）。库中只存引用，Key 本体在钥匙串。

// 数据层 API 先于 M2/M3 消费者落地；未使用项在对应里程碑接入后收紧。
#![allow(dead_code)]

use sqlx::{FromRow, SqlitePool};

use crate::db::now_unix;

#[derive(Debug, Clone, FromRow)]
pub struct ApiKeyRow {
    pub id: i64,
    pub name: String,
    pub keyring_account: String,
    pub base_url: String,
    pub model: String,
    pub concurrency_limit: i64,
    pub enabled: i64,
    pub created_at: i64,
    /// 每分钟请求上限（E18）；NULL = 不限速。
    pub rpm_limit: Option<i64>,
    /// 是否因连续鉴权/欠费失败被自动熔断（E18）。1 = 已熔断（同时 enabled=0）。
    pub circuit_broken: i64,
}

pub struct NewApiKey {
    pub name: String,
    pub keyring_account: String,
    pub base_url: String,
    pub model: String,
    pub concurrency_limit: i64,
    pub rpm_limit: Option<i64>,
}

pub async fn list(pool: &SqlitePool) -> Result<Vec<ApiKeyRow>, sqlx::Error> {
    sqlx::query_as::<_, ApiKeyRow>("SELECT * FROM api_keys ORDER BY created_at ASC, id ASC")
        .fetch_all(pool)
        .await
}

pub async fn get(pool: &SqlitePool, id: i64) -> Result<Option<ApiKeyRow>, sqlx::Error> {
    sqlx::query_as::<_, ApiKeyRow>("SELECT * FROM api_keys WHERE id = ?1")
        .bind(id)
        .fetch_optional(pool)
        .await
}

pub async fn insert(pool: &SqlitePool, k: &NewApiKey) -> Result<i64, sqlx::Error> {
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO api_keys (name, keyring_account, base_url, model, concurrency_limit, rpm_limit, enabled, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7) RETURNING id",
    )
    .bind(&k.name)
    .bind(&k.keyring_account)
    .bind(&k.base_url)
    .bind(&k.model)
    .bind(k.concurrency_limit)
    .bind(k.rpm_limit)
    .bind(now_unix())
    .fetch_one(pool)
    .await?;
    Ok(id)
}

pub async fn update_fields(
    pool: &SqlitePool,
    id: i64,
    name: Option<&str>,
    base_url: Option<&str>,
    model: Option<&str>,
    concurrency_limit: Option<i64>,
    rpm_limit: Option<Option<i64>>,
) -> Result<(), sqlx::Error> {
    // 逐字段 COALESCE 更新，未提供的保持原值。rpm_limit 用 Option<Option>：
    // 外层 None = 不改；Some(inner) = 改为 inner（inner None 即清除限速）。
    sqlx::query(
        "UPDATE api_keys SET
            name = COALESCE(?2, name),
            base_url = COALESCE(?3, base_url),
            model = COALESCE(?4, model),
            concurrency_limit = COALESCE(?5, concurrency_limit),
            rpm_limit = CASE WHEN ?6 = 1 THEN ?7 ELSE rpm_limit END
         WHERE id = ?1",
    )
    .bind(id)
    .bind(name)
    .bind(base_url)
    .bind(model)
    .bind(concurrency_limit)
    .bind(rpm_limit.is_some() as i64)
    .bind(rpm_limit.flatten())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn set_enabled(pool: &SqlitePool, id: i64, enabled: bool) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE api_keys SET enabled = ?2 WHERE id = ?1")
        .bind(id)
        .bind(enabled as i64)
        .execute(pool)
        .await?;
    Ok(())
}

/// 熔断某 Key（E18）：停用 + 置熔断位，供调度器连续 Auth/欠费失败达阈值时调用。
pub async fn trip_circuit(pool: &SqlitePool, id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE api_keys SET enabled = 0, circuit_broken = 1 WHERE id = ?1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// 恢复被熔断的 Key（E18）：清熔断位 + 重新启用。
pub async fn recover_circuit(pool: &SqlitePool, id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE api_keys SET enabled = 1, circuit_broken = 0 WHERE id = ?1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn delete(pool: &SqlitePool, id: i64) -> Result<Option<String>, sqlx::Error> {
    // 返回被删 Key 的 keyring_account，供上层清理钥匙串。
    let account: Option<String> =
        sqlx::query_scalar("DELETE FROM api_keys WHERE id = ?1 RETURNING keyring_account")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    Ok(account)
}

/// 近 `window` 次尝试的成功率与样本量（执行计划 1.4：近 50 次）。
pub async fn success_rate(
    pool: &SqlitePool,
    api_key_id: i64,
    window: i64,
) -> Result<(f64, i64), sqlx::Error> {
    let outcomes: Vec<(String,)> = sqlx::query_as(
        "SELECT outcome FROM task_attempts WHERE api_key_id = ?1 ORDER BY started_at DESC LIMIT ?2",
    )
    .bind(api_key_id)
    .bind(window)
    .fetch_all(pool)
    .await?;
    let total = outcomes.len() as i64;
    if total == 0 {
        return Ok((0.0, 0));
    }
    let ok = outcomes.iter().filter(|(o,)| o == "success").count() as f64;
    Ok((ok / total as f64, total))
}
