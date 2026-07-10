//! 批次 / 任务 / 执行记录数据仓（引擎与命令共用）。

// 部分查询由 M2 引擎与 M3 页面分别消费；先落地。
#![allow(dead_code)]

use sqlx::{FromRow, SqliteConnection, SqlitePool};

use crate::db::now_unix;
use crate::engine::events::SummaryCounts;

#[derive(Debug, Clone, FromRow)]
pub struct BatchRow {
    pub id: i64,
    pub created_at: i64,
    pub output_dir: String,
    pub params_json: String,
    pub status: String,
}

#[derive(Debug, Clone, FromRow)]
pub struct TaskRow {
    pub id: i64,
    pub batch_id: i64,
    pub ref_image_id: i64,
    pub prompt_id: i64,
    pub prompt_text_snapshot: String,
    pub status: String,
    pub api_key_id: Option<i64>,
    pub error_type: Option<String>,
    pub error_message: Option<String>,
    pub retry_count: i64,
    pub result_image_path: Option<String>,
    pub result_thumb_path: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

// ---------------- 批次 ----------------

pub async fn create_batch(
    conn: &mut SqliteConnection,
    output_dir: &str,
    params_json: &str,
) -> Result<i64, sqlx::Error> {
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO batches (created_at, output_dir, params_json, status)
         VALUES (?1, ?2, ?3, 'running') RETURNING id",
    )
    .bind(now_unix())
    .bind(output_dir)
    .bind(params_json)
    .fetch_one(&mut *conn)
    .await?;
    Ok(id)
}

pub async fn add_batch_ref(
    conn: &mut SqliteConnection,
    batch_id: i64,
    ref_image_id: i64,
    prompt_group_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO batch_refs (batch_id, ref_image_id, prompt_group_id) VALUES (?1, ?2, ?3)",
    )
    .bind(batch_id)
    .bind(ref_image_id)
    .bind(prompt_group_id)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

pub async fn list_batches(pool: &SqlitePool) -> Result<Vec<BatchRow>, sqlx::Error> {
    sqlx::query_as::<_, BatchRow>("SELECT * FROM batches ORDER BY created_at DESC, id DESC")
        .fetch_all(pool)
        .await
}

pub async fn get_batch(pool: &SqlitePool, id: i64) -> Result<Option<BatchRow>, sqlx::Error> {
    sqlx::query_as::<_, BatchRow>("SELECT * FROM batches WHERE id = ?1")
        .bind(id)
        .fetch_optional(pool)
        .await
}

pub async fn set_batch_status(pool: &SqlitePool, id: i64, status: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE batches SET status = ?2 WHERE id = ?1")
        .bind(id)
        .bind(status)
        .execute(pool)
        .await?;
    Ok(())
}

/// 若批次内所有任务均为终态（pass/rej/fail）则归档，返回是否归档。
pub async fn archive_if_all_terminal(
    pool: &SqlitePool,
    batch_id: i64,
) -> Result<bool, sqlx::Error> {
    let pending: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM tasks WHERE batch_id = ?1 AND status NOT IN ('pass','rej','fail')",
    )
    .bind(batch_id)
    .fetch_one(pool)
    .await?;
    if pending == 0 {
        set_batch_status(pool, batch_id, "archived").await?;
        Ok(true)
    } else {
        Ok(false)
    }
}

// ---------------- 任务 ----------------

pub async fn insert_task(
    conn: &mut SqliteConnection,
    batch_id: i64,
    ref_image_id: i64,
    prompt_id: i64,
    snapshot: &str,
    draw_index: i64,
) -> Result<i64, sqlx::Error> {
    let now = now_unix();
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO tasks (batch_id, ref_image_id, prompt_id, prompt_text_snapshot, draw_index, status, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 'q', ?6, ?6) RETURNING id",
    )
    .bind(batch_id)
    .bind(ref_image_id)
    .bind(prompt_id)
    .bind(snapshot)
    .bind(draw_index)
    .bind(now)
    .fetch_one(&mut *conn)
    .await?;
    Ok(id)
}

pub async fn get_task(pool: &SqlitePool, id: i64) -> Result<Option<TaskRow>, sqlx::Error> {
    sqlx::query_as::<_, TaskRow>("SELECT * FROM tasks WHERE id = ?1")
        .bind(id)
        .fetch_optional(pool)
        .await
}

/// 取待派发的 'q' 任务（FIFO）。
pub async fn fetch_queued(pool: &SqlitePool, limit: i64) -> Result<Vec<TaskRow>, sqlx::Error> {
    sqlx::query_as::<_, TaskRow>("SELECT * FROM tasks WHERE status = 'q' ORDER BY id ASC LIMIT ?1")
        .bind(limit)
        .fetch_all(pool)
        .await
}

/// 通用状态置换（仅写 status + updated_at）。合法性由调度器状态机守卫。
pub async fn set_status(pool: &SqlitePool, id: i64, status: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE tasks SET status = ?2, updated_at = ?3 WHERE id = ?1")
        .bind(id)
        .bind(status)
        .bind(now_unix())
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn mark_running(pool: &SqlitePool, id: i64, api_key_id: i64) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE tasks SET status = 'run', api_key_id = ?2, error_type = NULL, error_message = NULL, updated_at = ?3 WHERE id = ?1",
    )
    .bind(id)
    .bind(api_key_id)
    .bind(now_unix())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn mark_review(
    pool: &SqlitePool,
    id: i64,
    result_image_path: &str,
    result_thumb_path: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE tasks SET status = 'rev', result_image_path = ?2, result_thumb_path = ?3,
            error_type = NULL, error_message = NULL, updated_at = ?4 WHERE id = ?1",
    )
    .bind(id)
    .bind(result_image_path)
    .bind(result_thumb_path)
    .bind(now_unix())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn mark_retry(
    pool: &SqlitePool,
    id: i64,
    retry_count: i64,
    error_type: &str,
    error_message: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE tasks SET status = 'retry', retry_count = ?2, error_type = ?3, error_message = ?4, updated_at = ?5 WHERE id = ?1",
    )
    .bind(id)
    .bind(retry_count)
    .bind(error_type)
    .bind(error_message)
    .bind(now_unix())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn mark_fail(
    pool: &SqlitePool,
    id: i64,
    error_type: &str,
    error_message: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE tasks SET status = 'fail', error_type = ?2, error_message = ?3, updated_at = ?4 WHERE id = ?1",
    )
    .bind(id)
    .bind(error_type)
    .bind(error_message)
    .bind(now_unix())
    .execute(pool)
    .await?;
    Ok(())
}

/// 重新入队（fail/retry/rev → q），清错误，保留 retry_count。
/// 用于手动/中断重试，以及验收页「微调重试」（rev 重新生成）。
pub async fn requeue(pool: &SqlitePool, id: i64) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE tasks SET status = 'q', error_type = NULL, error_message = NULL, updated_at = ?2
         WHERE id = ?1 AND status IN ('fail','retry','rev')",
    )
    .bind(id)
    .bind(now_unix())
    .execute(pool)
    .await?;
    Ok(())
}

/// 删除单个任务（「不需要了」）。生成中/重试中的任务拒绝删除（返回 None），避免与在途
/// worker 竞争；成功则返回其所属批次 id（供调用方重估归档 + 补发汇总）。
/// task_attempts 由外键 ON DELETE CASCADE 一并清除。
pub async fn delete_task(pool: &SqlitePool, id: i64) -> Result<Option<i64>, sqlx::Error> {
    let batch_id: Option<i64> = sqlx::query_scalar(
        "SELECT batch_id FROM tasks WHERE id = ?1 AND status NOT IN ('run','retry')",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    if let Some(bid) = batch_id {
        sqlx::query("DELETE FROM tasks WHERE id = ?1")
            .bind(id)
            .execute(pool)
            .await?;
        Ok(Some(bid))
    } else {
        Ok(None)
    }
}

/// 删除某批次全部失败任务。返回删除行数。
pub async fn delete_failed(pool: &SqlitePool, batch_id: i64) -> Result<i64, sqlx::Error> {
    let n = sqlx::query("DELETE FROM tasks WHERE batch_id = ?1 AND status = 'fail'")
        .bind(batch_id)
        .execute(pool)
        .await?
        .rows_affected();
    Ok(n as i64)
}

/// 五视觉组计数（批次汇总）。
pub async fn counts_for_batch(
    pool: &SqlitePool,
    batch_id: i64,
) -> Result<SummaryCounts, sqlx::Error> {
    let rows: Vec<(String, i64)> =
        sqlx::query_as("SELECT status, COUNT(*) FROM tasks WHERE batch_id = ?1 GROUP BY status")
            .bind(batch_id)
            .fetch_all(pool)
            .await?;
    let mut c = SummaryCounts::default();
    for (status, n) in rows {
        match status.as_str() {
            "q" => c.pending += n,
            "run" | "retry" => c.running += n,
            "fail" => c.failed += n,
            "rev" => c.review += n,
            "pass" => c.passed += n,
            "rej" => c.rejected += n,
            _ => {}
        }
        c.total += n;
    }
    Ok(c)
}

/// 中断恢复：run/retry → fail(Interrupted)。返回受影响任务 id。
pub async fn recover_interrupted(pool: &SqlitePool) -> Result<Vec<i64>, sqlx::Error> {
    let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM tasks WHERE status IN ('run','retry')")
        .fetch_all(pool)
        .await?;
    if !ids.is_empty() {
        sqlx::query(
            "UPDATE tasks SET status = 'fail', error_type = 'Interrupted',
                error_message = '上次退出时任务被中断，任务现场已保留，可点击重试继续', updated_at = ?1
             WHERE status IN ('run','retry')",
        )
        .bind(now_unix())
        .execute(pool)
        .await?;
    }
    Ok(ids)
}

/// 某 Key 近若干次成功尝试的耗时（ms），用于伪进度 expected 估算。
pub async fn key_success_durations(
    pool: &SqlitePool,
    api_key_id: i64,
    limit: i64,
) -> Result<Vec<i64>, sqlx::Error> {
    let v: Vec<(i64,)> = sqlx::query_as(
        "SELECT duration_ms FROM task_attempts
         WHERE api_key_id = ?1 AND outcome = 'success' AND duration_ms IS NOT NULL
         ORDER BY started_at DESC LIMIT ?2",
    )
    .bind(api_key_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(v.into_iter().map(|(d,)| d).collect())
}

// ---------------- 执行记录 ----------------

pub async fn insert_attempt(
    pool: &SqlitePool,
    task_id: i64,
    api_key_id: i64,
    started_at: i64,
) -> Result<i64, sqlx::Error> {
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO task_attempts (task_id, api_key_id, started_at, outcome)
         VALUES (?1, ?2, ?3, 'pending') RETURNING id",
    )
    .bind(task_id)
    .bind(api_key_id)
    .bind(started_at)
    .fetch_one(pool)
    .await?;
    Ok(id)
}

// task_attempts 落库字段较多，集中一处写入；参数即列，无需再拆结构。
#[allow(clippy::too_many_arguments)]
pub async fn finish_attempt(
    pool: &SqlitePool,
    attempt_id: i64,
    finished_at: i64,
    outcome: &str,
    error_type: Option<&str>,
    error_message: Option<&str>,
    http_status: Option<i64>,
    duration_ms: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE task_attempts SET finished_at = ?2, outcome = ?3, error_type = ?4,
            error_message = ?5, http_status = ?6, duration_ms = ?7 WHERE id = ?1",
    )
    .bind(attempt_id)
    .bind(finished_at)
    .bind(outcome)
    .bind(error_type)
    .bind(error_message)
    .bind(http_status)
    .bind(duration_ms)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即失败，是期望行为
mod tests {
    use super::*;
    use crate::db::test_support::test_pool;

    /// 造 1 批次 + N 任务（默认 q），返回 (batch_id, task_ids)。
    async fn seed(pool: &SqlitePool, n: usize) -> (i64, Vec<i64>) {
        let mut tx = pool.begin().await.unwrap();
        let bid = create_batch(&mut tx, "/out", "{}").await.unwrap();
        sqlx::query("INSERT INTO prompt_groups (id,name,prefix,scene,is_temp,created_at) VALUES (1,'g','GG','',0,0)").execute(&mut *tx).await.unwrap();
        sqlx::query("INSERT INTO prompts (id,group_id,code,text,status,source,created_at,updated_at) VALUES (1,1,'GG-0001','t','active','library',0,0)").execute(&mut *tx).await.unwrap();
        sqlx::query("INSERT INTO ref_images (id,name,file_path,thumb_path,width,height,file_size,created_at) VALUES (1,'r','/a','/t',1,1,1,0)").execute(&mut *tx).await.unwrap();
        let mut ids = Vec::new();
        for _ in 0..n {
            ids.push(insert_task(&mut tx, bid, 1, 1, "t", 1).await.unwrap());
        }
        tx.commit().await.unwrap();
        (bid, ids)
    }

    #[tokio::test]
    async fn delete_task_refuses_running_and_cascades_attempts() {
        let (pool, _d) = test_pool().await;
        let (bid, ids) = seed(&pool, 2).await;
        let t = ids[0];
        // 造一个 Key + 一条 attempt，验证级联删除。
        sqlx::query("INSERT INTO api_keys (id,name,keyring_account,base_url,model,concurrency_limit,enabled,created_at) VALUES (1,'k','acct','http://x/v1','m',2,1,0)")
            .execute(&pool).await.unwrap();
        insert_attempt(&pool, t, 1, 0).await.unwrap();

        // 运行中拒绝删除。
        set_status(&pool, t, "run").await.unwrap();
        assert_eq!(delete_task(&pool, t).await.unwrap(), None, "运行中不应删除");
        // 置回可删终态后删除成功，返回批次 id。
        set_status(&pool, t, "fail").await.unwrap();
        assert_eq!(delete_task(&pool, t).await.unwrap(), Some(bid));
        assert!(get_task(&pool, t).await.unwrap().is_none(), "任务已删除");
        let att: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM task_attempts WHERE task_id = ?1")
            .bind(t)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(att, 0, "attempts 应级联删除");
    }

    #[tokio::test]
    async fn delete_failed_only_removes_failed() {
        let (pool, _d) = test_pool().await;
        let (bid, ids) = seed(&pool, 3).await;
        set_status(&pool, ids[0], "fail").await.unwrap();
        set_status(&pool, ids[1], "fail").await.unwrap();
        // ids[2] 留 q
        let n = delete_failed(&pool, bid).await.unwrap();
        assert_eq!(n, 2);
        let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE batch_id = ?1")
            .bind(bid)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(remaining, 1, "仅失败任务被删，q 保留");
    }
}
