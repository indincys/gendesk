//! A sample and its later submissions share a Poster group while keeping their own review runs.
use serde::{Deserialize, Serialize};
use sqlx::{Sqlite, SqlitePool, Transaction};

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    pub id: String,
    #[serde(rename = "final")]
    pub final_import: bool,
}

pub fn validate_id(id: &str) -> Result<(), String> {
    if id.is_empty()
        || id.len() > 80
        || !id.as_bytes()[0].is_ascii_alphanumeric()
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return Err(
            "posterBatch.id 要用 1～80 个英文字母、数字、横线或下划线，首字符必须是字母或数字"
                .into(),
        );
    }
    Ok(())
}

fn missing(id: &str) -> AppError {
    AppError::InvalidInput(format!(
        "找不到 Poster 批次 {id}，请核对试样清单里的 posterBatch.id"
    ))
}

pub async fn check(pool: &SqlitePool, spec: &Spec) -> AppResult<()> {
    let closed: Option<bool> =
        sqlx::query_scalar("SELECT closed FROM poster_intake_groups WHERE id=?")
            .bind(&spec.id)
            .fetch_optional(pool)
            .await?;
    match closed {
        Some(true) => Err(AppError::InvalidInput(format!(
            "Poster 批次 {} 已收齐，不能再追加",
            spec.id
        ))),
        None if spec.final_import => Err(missing(&spec.id)),
        _ => Ok(()),
    }
}

/// Attach and, on the last submission, close atomically with the new tasks.
pub async fn attach(tx: &mut Transaction<'_, Sqlite>, batch_id: i64, spec: &Spec) -> AppResult<()> {
    if !spec.final_import {
        sqlx::query("INSERT INTO poster_intake_groups (id) VALUES (?) ON CONFLICT(id) DO NOTHING")
            .bind(&spec.id)
            .execute(&mut **tx)
            .await?;
    }
    let changed = sqlx::query("UPDATE poster_intake_groups SET closed=? WHERE id=? AND closed=0")
        .bind(spec.final_import)
        .bind(&spec.id)
        .execute(&mut **tx)
        .await?;
    if changed.rows_affected() != 1 {
        return Err(AppError::InvalidInput(format!(
            "Poster 批次 {} 不存在或已收齐，不能再追加",
            spec.id
        )));
    }
    sqlx::query("UPDATE batches SET poster_group_id=? WHERE id=?")
        .bind(&spec.id)
        .bind(batch_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

pub async fn finish(pool: &SqlitePool, id: &str) -> AppResult<()> {
    validate_id(id).map_err(AppError::InvalidInput)?;
    let changed = sqlx::query("UPDATE poster_intake_groups SET closed=1 WHERE id=?")
        .bind(id)
        .execute(pool)
        .await?;
    if changed.rows_affected() != 1 {
        return Err(missing(id));
    }
    Ok(())
}
