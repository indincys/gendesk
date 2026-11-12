//! 数据库连接池与迁移（执行计划 §3 / 1.1）。
//!
//! 连接参数：WAL · synchronous=NORMAL · busy_timeout=5s · foreign_keys=ON。
//! 单写者原则在 M2 调度器落实；本层提供池与各域 repo。

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::SqlitePool;

pub mod repo;
pub mod upgrade;

pub use upgrade::{backup_dir, ConnectError};

/// 打开（或创建）数据库，并在升级保护下运行迁移（见 [`upgrade`]）。
pub async fn connect(db_path: &Path) -> Result<SqlitePool, ConnectError> {
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
        .await
        .map_err(ConnectError::Open)?;

    upgrade::upgrade(&pool, db_path, sqlx::migrate!("./migrations")).await?;
    tracing::info!(embedded = latest_embedded_migration(), "数据库已就绪");
    Ok(pool)
}

/// 直接运行内置迁移，不做版本判断与备份。只给测试重放单条迁移用；启动走 [`connect`]。
#[cfg(test)]
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

    async fn raise_reader_floor(pool: &SqlitePool, floor: i64) {
        sqlx::query("UPDATE schema_meta SET value=? WHERE key='reader_floor'")
            .bind(floor)
            .execute(pool)
            .await
            .unwrap();
    }

    /// 旧二进制打开被**破坏性升级**过的新库必须被拒绝，且拒绝理由要能照着做。
    ///
    /// 这条测试守的是一次真实事故：装好的 v0.18.0（内嵌迁移到 0024）打开被
    /// `pnpm tauri dev` 迁到 0026 的库，sqlx 拒绝 → 干净退出 → 用户只看见 dock 弹一下。
    ///
    /// 0052 起较新的库按 `reader_floor` 放行（见下一条测试），所以这里在伪造未来迁移的
    /// 同时把下限抬到它，重现「旧代码读不懂」的那种库；断言本身未放宽。
    #[tokio::test]
    async fn opening_a_newer_db_is_refused_with_actionable_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");

        let pool = connect(&path).await.unwrap();
        stamp_future_migration(&pool, 9999).await;
        raise_reader_floor(&pool, 9999).await;
        pool.close().await;

        let err = connect(&path)
            .await
            .expect_err("库要求的下限高于本应用时必须拒绝启动，而不是继续跑");
        let msg = err.to_string();

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

    /// 更新版本只做了加法时，旧版本照常打开，之后新版本再打开也不受影响。
    #[tokio::test]
    async fn an_additive_newer_db_opens_in_an_older_app() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");

        let pool = connect(&path).await.unwrap();
        stamp_future_migration(&pool, 9999).await;
        pool.close().await;

        let pool = connect(&path)
            .await
            .expect("下限没有超过本应用时应以兼容模式打开");
        let applied: i64 = sqlx::query_scalar("SELECT MAX(version) FROM _sqlx_migrations")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(applied, 9999, "不认识的迁移记录必须原样保留");
        assert!(
            !backup_dir(&path).exists(),
            "没有待应用的迁移，不该产生备份"
        );
    }

    /// 存量库升级前必须留一份一致性快照；新库不备份。
    #[tokio::test]
    async fn upgrading_an_existing_db_keeps_a_consistent_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gendesk.db");

        let pool = connect(&path).await.unwrap();
        assert!(!backup_dir(&path).exists(), "新库不该备份");
        sqlx::query(
            "CREATE TABLE marker(value TEXT NOT NULL); INSERT INTO marker VALUES('must survive')",
        )
        .execute(&pool)
        .await
        .unwrap();
        // 退回「有一条迁移还没跑」的样子。点名重放 0052（它本身幂等）而不是「最新一条」：
        // 0053 起的迁移是 ADD COLUMN 这类不可重放的加法，重放最新一条会撞重复列，
        // 测的就不再是升级前快照了。断言不变。
        sqlx::query("DELETE FROM _sqlx_migrations WHERE version=?")
            .bind(52_i64)
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;

        let pool = connect(&path).await.unwrap();
        let snapshots: Vec<_> = std::fs::read_dir(backup_dir(&path))
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        assert_eq!(snapshots.len(), 1, "{snapshots:?}");
        let backup = SqlitePool::connect(&format!("sqlite:{}", snapshots[0].display()))
            .await
            .unwrap();
        let marker: String = sqlx::query_scalar("SELECT value FROM marker")
            .fetch_one(&backup)
            .await
            .unwrap();
        assert_eq!(marker, "must survive");
        let applied: i64 = sqlx::query_scalar("SELECT MAX(version) FROM _sqlx_migrations")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(applied, latest_embedded_migration());
    }

    /// 迁移失败要停在失败前的状态，并把备份位置告诉人。
    #[tokio::test]
    async fn a_failing_migration_rolls_back_and_names_the_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gendesk.db");
        let pool = connect(&path).await.unwrap();
        sqlx::query("DELETE FROM _sqlx_migrations WHERE version=?")
            .bind(latest_embedded_migration())
            .execute(&pool)
            .await
            .unwrap();
        // 让最新一条迁移撞上一个占了名字的视图，必然失败。
        sqlx::query(
            "DROP TABLE schema_meta; CREATE VIEW schema_meta AS SELECT 1 AS key, 1 AS value",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool.close().await;

        let err = connect(&path).await.expect_err("迁移必须失败");
        let msg = err.to_string();
        assert!(msg.contains("检查更新"), "{msg}");
        assert!(msg.contains("升级前的完整备份"), "{msg}");
        assert!(matches!(err, ConnectError::Migrate { backup: Some(ref p), .. } if p.is_file()));
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

    #[tokio::test]
    async fn migration_0041_repairs_copy_after_sku_was_assigned_late() {
        let (pool, _dir) = test_support::test_pool().await;
        sqlx::query("DELETE FROM _sqlx_migrations WHERE version=41")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DROP TRIGGER sync_free_copy_product_after_sku_reassign")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO products(id,code,name,created_at,updated_at) VALUES(901,'LEG','旧商品',0,0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO skus(id,code,style_name,product_name,tier,topics_json,status,is_general,
             note,created_at,updated_at,folder_alias,product_id,music_keyword)
             VALUES(901,'LEG-1','旧款','','hot','[]','active',0,'',0,0,'',901,'')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO text_items(sku_id,product_id,kind,text,source,state,created_at)
             VALUES(901,NULL,'title','升级前文案','manual','free',0)",
        )
        .execute(&pool)
        .await
        .unwrap();

        migrate(&pool).await.unwrap();
        let repaired: i64 =
            sqlx::query_scalar("SELECT product_id FROM text_items WHERE sku_id=901")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(repaired, 901);
    }

    /// 运维验收：只对显式提供的数据库副本运行真实迁移，再做 SQLite 完整性检查。
    ///
    /// 运行方式：
    /// `GENDESK_MIGRATION_COPY=/tmp/real-db-copy.db cargo test --lib migrate_real_db_copy -- --ignored`
    #[tokio::test]
    #[ignore = "需要显式提供真实数据库副本路径"]
    async fn migrate_real_db_copy() {
        let path = std::path::PathBuf::from(
            std::env::var("GENDESK_MIGRATION_COPY")
                .expect("请通过 GENDESK_MIGRATION_COPY 提供数据库副本路径"),
        );
        assert!(path.is_file(), "数据库副本不存在：{}", path.display());
        assert!(
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.contains("copy")),
            "为防止误迁移真实库，副本文件名必须包含 copy"
        );

        let pool = connect(&path).await.unwrap();
        let applied: i64 = sqlx::query_scalar("SELECT MAX(version) FROM _sqlx_migrations")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(applied, latest_embedded_migration());

        let foreign_key_violations: Vec<(String, Option<i64>, String, i64)> =
            sqlx::query_as("PRAGMA foreign_key_check")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert!(
            foreign_key_violations.is_empty(),
            "foreign_key_check 发现异常：{foreign_key_violations:?}"
        );
        let integrity: Vec<String> = sqlx::query_scalar("PRAGMA integrity_check")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(integrity, vec!["ok"]);
        pool.close().await;
    }

    /// 与迁移无关的错误原样透出，不被这层翻译吞掉（翻译从 `explain_connect_error`
    /// 挪进了 `ConnectError` 的 Display，打开失败只加一句前缀）。
    #[test]
    fn non_migration_errors_pass_through() {
        let err = sqlx::Error::RowNotFound;
        let text = err.to_string();
        assert!(ConnectError::Open(err).to_string().contains(&text));
    }
}
