//! 中断恢复（执行计划 2.7 / 技术文档 4.3）。
//!
//! 启动时：run/retry → fail(Interrupted)（保留现场，可手动重试）；q 原样保留续跑。

use sqlx::SqlitePool;

use super::events::SharedSink;
use crate::db::repo::tasks as task_repo;
use crate::error::AppResult;

/// 执行恢复，返回被标记为中断的任务 id 列表（供前端 banner 计数）。
pub async fn recover(pool: &SqlitePool, _sink: &SharedSink) -> AppResult<Vec<i64>> {
    let ids = task_repo::recover_interrupted(pool).await?;
    if !ids.is_empty() {
        tracing::warn!(
            count = ids.len(),
            "启动恢复：将中断任务标记为 fail(Interrupted)"
        );
    }
    Ok(ids)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即失败，是期望行为
mod tests {
    use super::*;
    use crate::db::test_support::test_pool;
    use crate::engine::events::{test_sink::CollectingSink, SharedSink};

    #[tokio::test]
    async fn recovers_run_and_retry_to_fail_interrupted() {
        let (pool, _d) = test_pool().await;
        // 造 batch + 一个 run、一个 retry、一个 q
        let mut tx = pool.begin().await.unwrap();
        let b = task_repo::create_batch(&mut tx, "/out", "{}")
            .await
            .unwrap();
        // 需要外键：ref_images / prompts。插入最小依赖。
        sqlx::query("INSERT INTO prompt_groups (id,name,prefix,scene,is_temp,created_at) VALUES (1,'g','GG','',0,0)").execute(&mut *tx).await.unwrap();
        sqlx::query("INSERT INTO prompts (id,group_id,code,text,status,source,created_at,updated_at) VALUES (1,1,'GG-0001','t','active','library',0,0)").execute(&mut *tx).await.unwrap();
        sqlx::query("INSERT INTO ref_images (id,name,file_path,thumb_path,width,height,file_size,created_at) VALUES (1,'r','/a','/t',1,1,1,0)").execute(&mut *tx).await.unwrap();
        let t_run = task_repo::insert_task(&mut tx, b, 1, 1, "t", 1).await.unwrap();
        let t_retry = task_repo::insert_task(&mut tx, b, 1, 1, "t", 1).await.unwrap();
        let t_q = task_repo::insert_task(&mut tx, b, 1, 1, "t", 1).await.unwrap();
        tx.commit().await.unwrap();
        task_repo::set_status(&pool, t_run, "run").await.unwrap();
        task_repo::set_status(&pool, t_retry, "retry")
            .await
            .unwrap();

        let sink: SharedSink = CollectingSink::shared();
        let ids = recover(&pool, &sink).await.unwrap();
        assert_eq!(ids.len(), 2);

        assert_eq!(
            task_repo::get_task(&pool, t_run)
                .await
                .unwrap()
                .unwrap()
                .status,
            "fail"
        );
        assert_eq!(
            task_repo::get_task(&pool, t_retry)
                .await
                .unwrap()
                .unwrap()
                .status,
            "fail"
        );
        // q 保留
        assert_eq!(
            task_repo::get_task(&pool, t_q)
                .await
                .unwrap()
                .unwrap()
                .status,
            "q"
        );
    }
}
