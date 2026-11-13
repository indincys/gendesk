//! 生图工单收件台账（0023）。薄 SQL；规则在 `intake/`。

use sqlx::SqlitePool;

use crate::db::now_unix;
use crate::intake::ingest::Applied;
use crate::intake::JobView;

/// 该工单是否已经处理过（任何状态都算）。
///
/// error 与 hold 也挡住重投：失败的工单里可能已经有一半东西进了库，而 hold 是在等人
/// 表态——两种情况自动重来都不对。恢复由收录层按检查点处理，只有未导入的 hold 可在确认后重新记账。
pub async fn exists(pool: &SqlitePool, job_id: &str) -> Result<bool, sqlx::Error> {
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM intake_jobs WHERE job_id = ?1")
        .bind(job_id)
        .fetch_one(pool)
        .await?;
    Ok(n > 0)
}

/// 动手之前先记账。返回行 id。
pub async fn insert_running<'e, E: sqlx::Executor<'e, Database = sqlx::Sqlite>>(
    pool: E,
    job_id: &str,
    dir_name: &str,
) -> Result<i64, sqlx::Error> {
    let now = now_unix();
    let id = sqlx::query(
        "INSERT INTO intake_jobs (job_id, dir_name, status, created_at, updated_at)
         VALUES (?1, ?2, 'running', ?3, ?3)",
    )
    .bind(job_id)
    .bind(dir_name)
    .bind(now)
    .execute(pool)
    .await?
    .last_insert_rowid();
    Ok(id)
}

/// JSON 数组序列化；失败退化成 `[]`（台账是展示物，不该因为一次序列化失败让整单算失败）。
fn to_json<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "[]".into())
}

pub async fn mark_done<'e, E: sqlx::Executor<'e, Database = sqlx::Sqlite>>(
    pool: E,
    id: i64,
    done: &Applied,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE intake_jobs
            SET status = 'done', batch_ids = ?2, task_count = ?3, group_count = ?4,
                ref_count = ?5, params_json = ?6, wire_json = ?7, updated_at = ?8
          WHERE id = ?1",
    )
    .bind(id)
    .bind(to_json(&done.batch_ids))
    .bind(done.task_count)
    .bind(done.group_count)
    .bind(done.ref_count)
    .bind(to_json(&done.params_json))
    .bind(to_json(&done.wire_json))
    .bind(now_unix())
    .execute(pool)
    .await?;
    Ok(())
}

/// 写一句完成说明（外部成图导入单用它告诉人「没调用生图」）。
pub async fn set_message<'e, E: sqlx::Executor<'e, Database = sqlx::Sqlite>>(
    pool: E,
    id: i64,
    message: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE intake_jobs SET message = ?2 WHERE id = ?1")
        .bind(id)
        .bind(message)
        .execute(pool)
        .await?;
    Ok(())
}

/// Persist the last committed stage counts. Its identities are in intake_checkpoints.
/// A final batch can only become visible in the same transaction as a done ledger.
pub async fn mark_error(
    pool: &SqlitePool,
    id: i64,
    message: &str,
    partial: &Applied,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE intake_jobs
            SET status = 'error', message = ?2, batch_ids = ?3, task_count = ?4,
                group_count = ?5, ref_count = ?6, params_json = ?7, wire_json = ?8,
                updated_at = ?9
          WHERE id = ?1",
    )
    .bind(id)
    .bind(message)
    .bind(to_json(&partial.batch_ids))
    .bind(partial.task_count)
    .bind(partial.group_count)
    .bind(partial.ref_count)
    .bind(to_json(&partial.params_json))
    .bind(to_json(&partial.wire_json))
    .bind(now_unix())
    .execute(pool)
    .await?;
    Ok(())
}

/// 超阈值待确认。**此时库里没有导入任何东西**，故只记数字与说明。
pub async fn mark_hold(
    pool: &SqlitePool,
    id: i64,
    message: &str,
    task_count: i64,
    group_count: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE intake_jobs
            SET status = 'hold', message = ?2, task_count = ?3, group_count = ?4, updated_at = ?5
          WHERE id = ?1",
    )
    .bind(id)
    .bind(message)
    .bind(task_count)
    .bind(group_count)
    .bind(now_unix())
    .execute(pool)
    .await?;
    Ok(())
}

const SELECT: &str = "SELECT id, job_id, dir_name, status, batch_ids, task_count, group_count,
                             ref_count, params_json, wire_json, message, created_at
                        FROM intake_jobs";

type Row = (
    i64,
    String,
    String,
    String,
    String,
    i64,
    i64,
    i64,
    String,
    String,
    String,
    i64,
);

/// JSON 数组反序列化；坏了就当空（台账读不动不该让设置页整块打不开）。
fn from_json<T: serde::de::DeserializeOwned + Default>(s: &str) -> T {
    serde_json::from_str(s).unwrap_or_default()
}

fn to_view(r: Row) -> JobView {
    JobView {
        id: r.0,
        job_id: r.1,
        dir_name: r.2,
        status: r.3,
        batch_ids: from_json(&r.4),
        task_count: r.5,
        group_count: r.6,
        ref_count: r.7,
        params_json: from_json(&r.8),
        wire_json: from_json(&r.9),
        message: r.10,
        created_at: r.11,
    }
}

pub async fn get(pool: &SqlitePool, id: i64) -> Result<JobView, sqlx::Error> {
    let r: Row = sqlx::query_as(&format!("{SELECT} WHERE id = ?1"))
        .bind(id)
        .fetch_one(pool)
        .await?;
    Ok(to_view(r))
}

/// 按行 id 查一份台账；重试等允许旧界面重复到达的入口必须用它。
///
/// `fetch_one` 的 `RowNotFound` 是数据库实现细节，不该因为另一轮重试已经先删掉旧行
/// 就直接冒到界面上。
pub async fn get_optional(pool: &SqlitePool, id: i64) -> Result<Option<JobView>, sqlx::Error> {
    let r: Option<Row> = sqlx::query_as(&format!("{SELECT} WHERE id = ?1"))
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(r.map(to_view))
}

pub async fn get_by_job_id(
    pool: &SqlitePool,
    job_id: &str,
) -> Result<Option<JobView>, sqlx::Error> {
    let r: Option<Row> = sqlx::query_as(&format!("{SELECT} WHERE job_id = ?1"))
        .bind(job_id)
        .fetch_optional(pool)
        .await?;
    Ok(r.map(to_view))
}

/// 该目录名是否已有台账（并发提交时，输的一方据此安静跳过）。
pub async fn dir_known(pool: &SqlitePool, dir_name: &str) -> Result<bool, sqlx::Error> {
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM intake_jobs WHERE dir_name = ?1")
        .bind(dir_name)
        .fetch_one(pool)
        .await?;
    Ok(n > 0)
}

/// 最近若干条（测试回读台账用）。
#[cfg(test)]
pub async fn list_recent(pool: &SqlitePool, limit: i64) -> Result<Vec<JobView>, sqlx::Error> {
    let rows: Vec<Row> = sqlx::query_as(&format!(
        "{SELECT} ORDER BY created_at DESC, id DESC LIMIT ?1"
    ))
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(to_view).collect())
}

/// Remove only an unimported hold row during confirmation. Retry must retain identities.
pub async fn delete(pool: &SqlitePool, id: i64) -> Result<bool, sqlx::Error> {
    let n = sqlx::query("DELETE FROM intake_jobs WHERE id = ?1")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected();
    Ok(n > 0)
}

pub async fn checkpoint(pool: &SqlitePool, job_id: &str) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT state_json FROM intake_checkpoints WHERE job_id = ?")
        .bind(job_id)
        .fetch_optional(pool)
        .await
}

pub async fn save_checkpoint(
    conn: &mut sqlx::SqliteConnection,
    job_id: &str,
    state: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO intake_checkpoints (job_id, state_json, updated_at) VALUES (?1, ?2, ?3) ON CONFLICT(job_id) DO UPDATE SET state_json=excluded.state_json, updated_at=excluded.updated_at")
        .bind(job_id).bind(state).bind(now_unix()).execute(conn).await?;
    Ok(())
}

pub async fn mark_running<'e, E: sqlx::Executor<'e, Database = sqlx::Sqlite>>(
    pool: E,
    id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE intake_jobs SET status='running', message='', updated_at=?2 WHERE id=?1")
        .bind(id)
        .bind(now_unix())
        .execute(pool)
        .await?;
    Ok(())
}

/// Bind a previously unparseable, proven-empty order to its now-known identity atomically.
pub async fn rebind_empty_job(
    pool: &SqlitePool,
    id: i64,
    job_id: &str,
    state: &str,
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "DELETE FROM intake_checkpoints WHERE job_id=(SELECT job_id FROM intake_jobs WHERE id=?)",
    )
    .bind(id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE intake_jobs SET job_id=?2 WHERE id=?1")
        .bind(id)
        .bind(job_id)
        .execute(&mut *tx)
        .await?;
    save_checkpoint(&mut tx, job_id, state).await?;
    tx.commit().await?;
    Ok(())
}
