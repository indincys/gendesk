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

/// 本二进制**编译期内嵌**的最新迁移版本号。
///
/// `sqlx::migrate!` 在编译时把 `migrations/` 整个塞进二进制，故这个数字是「这份可执行
/// 文件认识到第几版 schema」的准确答案 —— 它与 `CARGO_PKG_VERSION` 是两件事，
/// 排查「旧包对新库」时要看的正是它。
pub fn latest_embedded_migration() -> i64 {
    sqlx::migrate!("./migrations")
        .iter()
        .map(|m| m.version)
        .max()
        .unwrap_or(0)
}

/// 把 [`connect`] 的失败翻译成一句能照着做的话。
///
/// 迁移是 forward-only 且编译期内嵌，故**旧版本的二进制打开被新版本迁移过的库**时，
/// sqlx 会报 `VersionMissing` 并拒绝继续。这是保护而非故障：旧代码不认识新 schema，
/// 让它跑下去才会真损坏数据。
///
/// 但 sqlx 的原文（"migration N was previously applied but is missing in the resolved
/// migrations"）不指向任何可执行的动作，而这条错误恰恰只出现在最难自查的场景里 ——
/// 用户看到的全部现象是 **dock 图标弹一下就没了**。翻译在这里做，是因为只有这一层
/// 同时知道「库里到第几版」和「我内嵌到第几版」。
pub fn explain_connect_error(err: &sqlx::Error) -> String {
    let sqlx::Error::Migrate(migrate_err) = err else {
        return err.to_string();
    };
    match migrate_err.as_ref() {
        sqlx::migrate::MigrateError::VersionMissing(applied) => format!(
            "数据库是更新版本的 GenDesk 建立的，当前这个版本读不了它。\n\n\
             库里已应用到迁移 {applied}，而本应用（v{}）内置的迁移只到 {}。\n\n\
             请把 GenDesk 更新到最新版本再打开。用旧版本继续运行会损坏数据，因此已停止启动。",
            env!("CARGO_PKG_VERSION"),
            latest_embedded_migration(),
        ),
        sqlx::migrate::MigrateError::VersionMismatch(version) => format!(
            "数据库里迁移 {version} 的内容与本应用内置的对不上（校验和不一致）。\n\n\
             通常是同一版本号的迁移文件被改过 —— 迁移发布后不可修改。\n\
             请确认使用的是正式发布版本；若这是开发构建，需要重置数据库。"
        ),
        other => other.to_string(),
    }
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

#[cfg(test)]
// 测试内允许 unwrap/expect：断言失败即测试失败，是期望行为。
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// 伪造一条「更新版本的应用留下的」迁移记录，重现「旧包对新库」。
    async fn stamp_future_migration(pool: &SqlitePool, version: i64) {
        sqlx::query(
            "INSERT INTO _sqlx_migrations
             (version, description, installed_on, success, checksum, execution_time)
             VALUES (?, 'from the future', CURRENT_TIMESTAMP, 1, X'00', 0)",
        )
        .bind(version)
        .execute(pool)
        .await
        .unwrap();
    }

    /// 旧二进制打开新库必须**被拒绝**，且拒绝理由要能照着做。
    ///
    /// 这条测试守的是一次真实事故：装好的 v0.18.0（内嵌迁移到 0024）打开被
    /// `pnpm tauri dev` 迁到 0026 的库，sqlx 拒绝 → 干净退出 → 用户只看见 dock 弹一下。
    #[tokio::test]
    async fn opening_a_newer_db_is_refused_with_actionable_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");

        let pool = connect(&path).await.unwrap();
        stamp_future_migration(&pool, 9999).await;
        pool.close().await;

        let err = connect(&path)
            .await
            .expect_err("库里有本应用不认识的迁移时必须拒绝启动，而不是继续跑");
        let msg = explain_connect_error(&err);

        // 两个数字都要在：库到了哪一版、我只认到哪一版。
        assert!(msg.contains("9999"), "未指出库里的版本：{msg}");
        assert!(
            msg.contains(&latest_embedded_migration().to_string()),
            "未指出本应用内嵌的版本：{msg}"
        );
        // 必须给出动作，而不是复述 sqlx 的英文。
        assert!(msg.contains("更新到最新版本"), "未给出可执行的动作：{msg}");
        assert!(
            !msg.contains("resolved migrations"),
            "泄漏了 sqlx 原文：{msg}"
        );
    }

    /// 内嵌版本号必须跟着 `migrations/` 走，而不是某个写死的数字。
    #[tokio::test]
    async fn latest_embedded_migration_tracks_the_migrations_dir() {
        let (pool, _dir) = test_support::test_pool().await;
        let applied: i64 = sqlx::query_scalar("SELECT MAX(version) FROM _sqlx_migrations")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(applied, latest_embedded_migration());
    }

    /// 与迁移无关的错误原样透出，不被这层翻译吞掉。
    #[test]
    fn non_migration_errors_pass_through() {
        let err = sqlx::Error::RowNotFound;
        assert_eq!(explain_connect_error(&err), err.to_string());
    }
}
