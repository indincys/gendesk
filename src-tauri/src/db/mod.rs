//! 数据库连接池与迁移（执行计划 §3 / 1.1）。
//!
//! 连接参数：WAL · synchronous=NORMAL · busy_timeout=5s · foreign_keys=ON。
//! 单写者原则在 M2 调度器落实；本层提供池与各域 repo。

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::SqlitePool;

use crate::publish::paths;

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

    backup_before_destructive_migration(&pool, db_path).await?;
    migrate(&pool).await?;
    recover_interrupted_exports(&pool).await?;
    Ok(pool)
}

/// single-instance 保证启动恢复时没有另一份本应用仍在导出。token 会跨越数据库记账
/// 与 READY 落盘之间的窗口：READY 已出现代表包可继续执行，只需释放 token；READY
/// 尚未出现且回执为空，代表上次在发布包前崩溃，需要把 used 库存和 sheet 一并回滚。
pub(crate) async fn recover_interrupted_exports(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    let interrupted: Vec<(i64, String, Option<String>, String)> = sqlx::query_as(
        "SELECT id,status,export_dir,export_token FROM task_sheets
         WHERE export_token IS NOT NULL",
    )
    .fetch_all(pool)
    .await?;

    for (sheet_id, status, export_dir, token) in interrupted {
        if status != "exported" {
            sqlx::query("UPDATE task_sheets SET export_token=NULL WHERE id=?1 AND export_token=?2")
                .bind(sheet_id)
                .bind(&token)
                .execute(pool)
                .await?;
            continue;
        }

        let package = export_dir.as_deref().map(Path::new);
        let ready_exists = package.is_some_and(|path| path.join(paths::READY).is_file());
        let receipt_started = package.is_some_and(|path| {
            path.join(paths::RECEIPT_JSONL)
                .metadata()
                .is_ok_and(|metadata| metadata.len() > 0)
        });
        if ready_exists || receipt_started {
            sqlx::query("UPDATE task_sheets SET export_token=NULL WHERE id=?1 AND export_token=?2")
                .bind(sheet_id)
                .bind(&token)
                .execute(pool)
                .await?;
            continue;
        }

        let mut tx = pool.begin().await?;
        let reverted = sqlx::query(
            "UPDATE task_sheets
             SET status='confirmed',export_dir=NULL,exported_at=NULL,export_token=NULL,updated_at=?3
             WHERE id=?1 AND status='exported' AND export_token=?2",
        )
        .bind(sheet_id)
        .bind(&token)
        .bind(now_unix())
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if reverted == 1 {
            sqlx::query(
                "UPDATE image_assets SET state='held',updated_at=?2
                 WHERE state='used' AND post_id IN (SELECT id FROM posts WHERE sheet_id=?1)",
            )
            .bind(sheet_id)
            .bind(now_unix())
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "UPDATE text_items SET state='held'
                 WHERE state='used' AND post_id IN (SELECT id FROM posts WHERE sheet_id=?1)",
            )
            .bind(sheet_id)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
    }
    Ok(())
}

/// 0038 会删除旧发布模型。只要是从 0038 之前的存量库升级，就在同目录先用
/// SQLite `VACUUM INTO` 生成一致性快照；备份失败会阻断迁移，绝不带病 DROP。
async fn backup_before_destructive_migration(
    pool: &SqlitePool,
    db_path: &Path,
) -> Result<Option<PathBuf>, sqlx::Error> {
    let has_migrations: i64 = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='_sqlx_migrations')",
    )
    .fetch_one(pool)
    .await?;
    if has_migrations == 0 {
        return Ok(None);
    }
    let version: i64 =
        sqlx::query_scalar("SELECT COALESCE(MAX(version),0) FROM _sqlx_migrations WHERE success=1")
            .fetch_one(pool)
            .await?;
    if version == 0 || version >= 38 {
        return Ok(None);
    }
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let stem = db_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("gendesk");
    let backup = db_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!("{stem}.pre-0038-{stamp}.db"));
    sqlx::query("VACUUM INTO ?1")
        .bind(backup.to_string_lossy().to_string())
        .execute(pool)
        .await?;
    tracing::info!(path=%backup.display(), from_version=version, "已在破坏性迁移前生成数据库备份");
    Ok(Some(backup))
}

/// 运行内置迁移（forward-only）。
pub async fn migrate(pool: &SqlitePool) -> Result<(), sqlx::migrate::MigrateError> {
    // Forward-only 文件必须连续保留，真实库会拒绝缺少任一已应用版本的二进制。
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

    #[tokio::test]
    async fn destructive_migration_is_preceded_by_consistent_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy.db");
        let opts = SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE _sqlx_migrations(version INTEGER,success INTEGER);
             CREATE TABLE legacy_marker(value TEXT NOT NULL);
             INSERT INTO _sqlx_migrations VALUES(37,1);
             INSERT INTO legacy_marker VALUES('must survive')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let backup = backup_before_destructive_migration(&pool, &path)
            .await
            .unwrap()
            .unwrap();
        assert!(backup.is_file());
        let backup_pool = SqlitePool::connect(&format!("sqlite:{}", backup.display()))
            .await
            .unwrap();
        let marker: String = sqlx::query_scalar("SELECT value FROM legacy_marker")
            .fetch_one(&backup_pool)
            .await
            .unwrap();
        assert_eq!(marker, "must survive");
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

    /// 与迁移无关的错误原样透出，不被这层翻译吞掉。
    #[test]
    fn non_migration_errors_pass_through() {
        let err = sqlx::Error::RowNotFound;
        assert_eq!(explain_connect_error(&err), err.to_string());
    }
}
