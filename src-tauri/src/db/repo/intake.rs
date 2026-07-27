//! 生图工单收件台账（0023）。薄 SQL；规则在 `intake/`。

use sqlx::SqlitePool;

use crate::db::now_unix;
use crate::intake::ingest::Applied;
use crate::intake::JobView;

/// 该工单是否已经处理过（任何状态都算）。
///
/// error 与 hold 也挡住重投：失败的工单里可能已经有一半东西进了库，而 hold 是在等人
/// 表态——两种情况自动重来都不对。重来是人的决定（设置页「重试」/「确认开跑」删掉这行）。
pub async fn exists(pool: &SqlitePool, job_id: &str) -> Result<bool, sqlx::Error> {
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM intake_jobs WHERE job_id = ?1")
        .bind(job_id)
        .fetch_one(pool)
        .await?;
    Ok(n > 0)
}

/// 动手之前先记账。返回行 id。
pub async fn insert_running(
    pool: &SqlitePool,
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

pub async fn mark_done(pool: &SqlitePool, id: i64, done: &Applied) -> Result<(), sqlx::Error> {
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

/// 失败：连**已经导入了多少**一起记下来。
///
/// 收录不是一个能整体回滚的事务：参考图要拷文件、建缩略图，建批要发编号，
/// 而中途失败时前面那些已经落库、批次可能已经在跑。原来这一行只存一句错误原文，
/// 于是「这份工单到底做到哪了」在库里没有任何痕迹，回执还写着「没有导入任何东西」
/// —— 人照着那句话删掉台账行重投，就会得到第二份提示词和第二个批次。
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

/// 最近若干条（设置页列表）。
pub async fn list_recent(pool: &SqlitePool, limit: i64) -> Result<Vec<JobView>, sqlx::Error> {
    let rows: Vec<Row> = sqlx::query_as(&format!(
        "{SELECT} ORDER BY created_at DESC, id DESC LIMIT ?1"
    ))
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(to_view).collect())
}

/// 删掉一行 = 允许该工单重新收录（设置页「重试」/「确认开跑」）。返回是否删到。
pub async fn delete(pool: &SqlitePool, id: i64) -> Result<bool, sqlx::Error> {
    let n = sqlx::query("DELETE FROM intake_jobs WHERE id = ?1")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected();
    Ok(n > 0)
}
