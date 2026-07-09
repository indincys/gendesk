//! 数据库连接池与迁移（执行计划 §3 / 1.1）。
//!
//! 连接参数：WAL · synchronous=NORMAL · busy_timeout=5s · foreign_keys=ON。
//! 单写者原则在 M2 调度器落实；本层提供池与各域 repo。

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::SqlitePool;

pub mod repo;

/// 打开（或创建）数据库并运行迁移。
pub async fn connect(db_path: &Path) -> Result<SqlitePool, sqlx::Error> {
    let opts = SqliteConnectOptions::new()
        .filename(db_path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(Duration::from_secs(5))
        .foreign_keys(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(opts)
        .await?;

    migrate(&pool).await?;
    Ok(pool)
}

/// 运行内置迁移（forward-only）。
pub async fn migrate(pool: &SqlitePool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!("./migrations").run(pool).await
}

/// 当前 Unix 秒（数据库时间戳统一使用）。
pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
pub mod test_support {
    use super::*;

    // 测试内允许 unwrap/expect：断言失败即测试失败，是期望行为。
    #[allow(clippy::unwrap_used, clippy::expect_used)]
    /// 建立一个基于临时文件的测试库（含迁移），返回池与其临时目录守卫。
    pub async fn test_pool() -> (SqlitePool, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let pool = connect(&dir.path().join("test.db")).await.unwrap();
        (pool, dir)
    }
}
