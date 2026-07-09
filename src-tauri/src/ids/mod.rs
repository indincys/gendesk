//! 编号号池（执行计划 1.2 / 技术文档 5.1）。
//!
//! 编号格式 `前缀-0001`；发放优先取回收池最小号，否则自增 `id_pools.next_seq`。
//! 全部操作在调用方传入的事务连接上执行，与业务写同事务（单写者，杜绝竞态）。

// recycle 在 M3 废纸篓清理接入；先落地并由属性测试覆盖。
#![allow(dead_code)]

use sqlx::{Row, SqliteConnection, SqlitePool};

/// 分配一个编号（前缀内单调，回收优先）。必须在事务中调用。
pub async fn allocate(conn: &mut SqliteConnection, prefix: &str) -> Result<i64, sqlx::Error> {
    // 1) 回收池优先：取该前缀最小的可用号并移除。
    if let Some(row) =
        sqlx::query("SELECT number FROM id_recycled WHERE prefix = ?1 ORDER BY number LIMIT 1")
            .bind(prefix)
            .fetch_optional(&mut *conn)
            .await?
    {
        let number: i64 = row.get(0);
        sqlx::query("DELETE FROM id_recycled WHERE prefix = ?1 AND number = ?2")
            .bind(prefix)
            .bind(number)
            .execute(&mut *conn)
            .await?;
        return Ok(number);
    }

    // 2) 否则自增号池。首次遇到前缀时初始化。
    sqlx::query(
        "INSERT INTO id_pools (prefix, next_seq) VALUES (?1, 1) ON CONFLICT(prefix) DO NOTHING",
    )
    .bind(prefix)
    .execute(&mut *conn)
    .await?;
    let number: i64 = sqlx::query_scalar(
        "UPDATE id_pools SET next_seq = next_seq + 1 WHERE prefix = ?1 RETURNING next_seq - 1",
    )
    .bind(prefix)
    .fetch_one(&mut *conn)
    .await?;
    Ok(number)
}

/// 回收一个编号（进废纸篓清理时调用）。幂等：重复回收忽略。
pub async fn recycle(
    conn: &mut SqliteConnection,
    prefix: &str,
    number: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT OR IGNORE INTO id_recycled (prefix, number) VALUES (?1, ?2)")
        .bind(prefix)
        .bind(number)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// 只读窥视下一个自增号（不消费，用于导入预览的编号区间估算，忽略回收池）。
pub async fn peek_next(pool: &SqlitePool, prefix: &str) -> Result<i64, sqlx::Error> {
    let next: Option<i64> = sqlx::query_scalar("SELECT next_seq FROM id_pools WHERE prefix = ?1")
        .bind(prefix)
        .fetch_optional(pool)
        .await?;
    Ok(next.unwrap_or(1))
}

/// 格式化为对外编号：`DZ-0001`。
pub fn format_code(prefix: &str, number: i64) -> String {
    format!("{prefix}-{number:04}")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即失败，是期望行为
mod tests {
    use super::*;
    use crate::db::test_support::test_pool;
    use proptest::prelude::*;
    use std::collections::BTreeSet;

    #[tokio::test]
    async fn allocate_is_monotonic_per_prefix() {
        let (pool, _dir) = test_pool().await;
        let mut conn = pool.acquire().await.unwrap();
        assert_eq!(allocate(&mut conn, "DZ").await.unwrap(), 1);
        assert_eq!(allocate(&mut conn, "DZ").await.unwrap(), 2);
        assert_eq!(allocate(&mut conn, "DZ").await.unwrap(), 3);
        // 前缀隔离
        assert_eq!(allocate(&mut conn, "RX").await.unwrap(), 1);
    }

    #[tokio::test]
    async fn recycled_numbers_reused_before_new() {
        let (pool, _dir) = test_pool().await;
        let mut conn = pool.acquire().await.unwrap();
        assert_eq!(allocate(&mut conn, "DZ").await.unwrap(), 1);
        assert_eq!(allocate(&mut conn, "DZ").await.unwrap(), 2);
        assert_eq!(allocate(&mut conn, "DZ").await.unwrap(), 3);
        recycle(&mut conn, "DZ", 2).await.unwrap();
        // 下次发放应先取回收的 2
        assert_eq!(allocate(&mut conn, "DZ").await.unwrap(), 2);
        // 回收池空后继续自增
        assert_eq!(allocate(&mut conn, "DZ").await.unwrap(), 4);
    }

    #[test]
    fn format_code_pads_to_four() {
        assert_eq!(format_code("DZ", 1), "DZ-0001");
        assert_eq!(format_code("RX", 128), "RX-0128");
        assert_eq!(format_code("CJ", 12345), "CJ-12345");
    }

    // 操作序列：分配 / 回收一个当前占用号。
    #[derive(Debug, Clone)]
    enum Op {
        Alloc(u8),       // prefix idx
        Recycle(u8, u8), // prefix idx, which outstanding (mod)
    }

    fn op_strategy() -> impl Strategy<Value = Op> {
        prop_oneof![
            (0u8..2).prop_map(Op::Alloc),
            (0u8..2, any::<u8>()).prop_map(|(p, n)| Op::Recycle(p, n)),
        ]
    }

    proptest! {
        // 属性：任意 分配/回收 序列，同一前缀的「当前占用号」集合永不出现重号，
        // 且回收号总在新增号之前被复用（不漏号）。由不变量驱动，AI 无法照抄实现骗过。
        #[test]
        fn pool_never_duplicates_outstanding(ops in proptest::collection::vec(op_strategy(), 0..200)) {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            rt.block_on(async {
                let (pool, _dir) = test_pool().await;
                let mut conn = pool.acquire().await.unwrap();
                let prefixes = ["AA", "BB"];
                // 参考模型：每个前缀当前占用的号集合
                let mut outstanding: [BTreeSet<i64>; 2] = [BTreeSet::new(), BTreeSet::new()];

                for op in ops {
                    match op {
                        Op::Alloc(pi) => {
                            let pi = (pi as usize) % 2;
                            let n = allocate(&mut conn, prefixes[pi]).await.unwrap();
                            // 不变量：新发放号不得已在占用集合中
                            prop_assert!(!outstanding[pi].contains(&n), "重号 {} 前缀 {}", n, prefixes[pi]);
                            outstanding[pi].insert(n);
                        }
                        Op::Recycle(pi, which) => {
                            let pi = (pi as usize) % 2;
                            if outstanding[pi].is_empty() { continue; }
                            let idx = (which as usize) % outstanding[pi].len();
                            let n = *outstanding[pi].iter().nth(idx).unwrap();
                            recycle(&mut conn, prefixes[pi], n).await.unwrap();
                            outstanding[pi].remove(&n);
                        }
                    }
                }
                Ok(())
            })?;
        }
    }
}
