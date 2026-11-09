//! 启动时的数据库升级保护。
//!
//! 应用靠应用内更新分发到多台电脑，升级脚本出问题的代价是「那台电脑打不开」。底线：
//!
//! 1. **先备份再动 schema**：有待应用的迁移时，先 `VACUUM INTO` 一份一致性快照到
//!    `database-backups/`（滚动保留最近 [`BACKUPS_KEPT`] 份）。备份失败就不升级。
//! 2. **失败不留半截**：每条迁移在自己的事务里（`-- no-transaction` 的除外），失败的那条
//!    整体撤销，已成功的保留；下一个修好的版本从断点继续。
//! 3. **失败给出路**：错误翻译成能照着做的话；启动壳（`lib.rs`）据此弹出可「检查更新」
//!    的对话框，装上修复版即可，不必手动下载。
//! 4. **旧版本打开新库**：库里有本应用不认识的迁移时，看 `schema_meta.reader_floor`
//!    （0052）。不高于本应用内嵌的最新迁移就忽略那些迁移照常打开；否则拒绝，
//!    因为旧代码读写破坏性改过的 schema 会损坏数据。

use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};

use sqlx::migrate::{MigrateError, Migrator};
use sqlx::SqlitePool;

/// 升级前快照所在目录（数据根下）。离线整理命令的备份也放这里。
pub const BACKUP_DIR: &str = "database-backups";
/// 升级前快照滚动保留的份数。只清本模块写的快照，不碰其它备份。
pub const BACKUPS_KEPT: usize = 5;
/// 升级前快照的文件名标记：`YYYY-MM-DD_HHMMSS_升级前_迁移{from}到{to}.db`。
const BACKUP_TAG: &str = "_升级前_";
/// 兼容下限从这一号迁移起生效；之前的迁移早已发布，不受「破坏性改动须抬下限」检查。
#[cfg(test)]
const COMPAT_SINCE: i64 = 52;

/// 数据库与 `db_path` 同在数据根下时，备份目录的位置。
pub fn backup_dir(db_path: &Path) -> PathBuf {
    db_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(BACKUP_DIR)
}

/// 打开数据库失败的原因。`Display` 是给人看的完整说明（启动对话框直接用）。
#[derive(Debug)]
pub enum ConnectError {
    /// 连不上库文件（权限、损坏、磁盘）。
    Open(sqlx::Error),
    /// 库被更新版本做过破坏性升级，本应用读不了。
    TooNew {
        db_version: i64,
        floor: Option<i64>,
        embedded: i64,
    },
    /// 升级前备份失败，没有动 schema。
    Backup { dir: PathBuf, reason: String },
    /// 迁移本身失败。
    Migrate {
        error: MigrateError,
        backup: Option<PathBuf>,
    },
}

impl std::error::Error for ConnectError {}

impl fmt::Display for ConnectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let app = env!("CARGO_PKG_VERSION");
        match self {
            ConnectError::Open(e) => write!(f, "无法打开数据库：{e}"),
            ConnectError::TooNew {
                db_version,
                floor,
                embedded,
            } => {
                let need = match floor {
                    Some(floor) => format!("这份库要求应用至少认识到迁移 {floor}"),
                    None => "这份库没有兼容标记".to_string(),
                };
                write!(
                    f,
                    "数据库是更新版本的 GenDesk 升级过的，当前这个版本（v{app}）读不了它。\n\n\
                     库里已应用到迁移 {db_version}，{need}，而本应用内置的迁移只到 {embedded}。\n\n\
                     请把 GenDesk 更新到最新版本再打开。用旧版本继续运行会损坏数据，因此已停止启动。"
                )
            }
            ConnectError::Backup { dir, reason } => write!(
                f,
                "升级数据库前要先做一份备份，但备份没有成功：{reason}\n\n\
                 为了不在没有退路的情况下改动数据，这次没有升级，数据保持原样。\n\
                 请检查磁盘空间后重新打开 GenDesk。备份目录：{}",
                dir.display()
            ),
            ConnectError::Migrate { error, backup } => {
                let backup = match backup {
                    Some(path) => format!("升级前的完整备份：{}", path.display()),
                    None => "这是一份新库，没有需要备份的数据。".to_string(),
                };
                match error {
                    MigrateError::VersionMismatch(version) => write!(
                        f,
                        "数据库里迁移 {version} 的内容与本应用（v{app}）内置的对不上（校验和不一致）。\n\n\
                         通常是同一版本号的迁移文件被改过 —— 迁移发布后不可修改。\n\
                         请确认使用的是正式发布版本；若这是开发构建，需要重置数据库。\n\n{backup}"
                    ),
                    MigrateError::Dirty(version) => write!(
                        f,
                        "数据库迁移 {version} 上次执行到一半就中断了，库处于不完整状态。\n\n\
                         请点「检查更新」安装修复版本；如果仍打不开，可以退出后用备份替换 gendesk.db。\n\n{backup}"
                    ),
                    MigrateError::ExecuteMigration(source, version) => write!(
                        f,
                        "数据库升级失败：迁移 {version} 没能执行（{source}）。\n\n\
                         失败的那一步已整体撤销，之前的数据没有丢失，也没有写入一半的内容。\n\
                         这是这个版本（v{app}）的升级脚本有问题，请点「检查更新」安装修复版本，\
                         或先装回上一个版本继续使用。\n\n{backup}"
                    ),
                    other => write!(
                        f,
                        "数据库升级失败：{other}\n\n请点「检查更新」安装修复版本。\n\n{backup}"
                    ),
                }
            }
        }
    }
}

impl From<ConnectError> for crate::error::AppError {
    fn from(e: ConnectError) -> Self {
        crate::error::AppError::Database(e.to_string())
    }
}

/// 库里已成功应用的迁移号；新库为空。
async fn applied_versions(pool: &SqlitePool) -> Result<BTreeSet<i64>, sqlx::Error> {
    if !table_exists(pool, "_sqlx_migrations").await? {
        return Ok(BTreeSet::new());
    }
    let rows: Vec<i64> = sqlx::query_scalar("SELECT version FROM _sqlx_migrations WHERE success=1")
        .fetch_all(pool)
        .await?;
    Ok(rows.into_iter().collect())
}

/// 库的兼容下限；0052 之前的库没有这张表。
async fn reader_floor(pool: &SqlitePool) -> Result<Option<i64>, sqlx::Error> {
    if !table_exists(pool, "schema_meta").await? {
        return Ok(None);
    }
    sqlx::query_scalar("SELECT value FROM schema_meta WHERE key='reader_floor'")
        .fetch_optional(pool)
        .await
}

async fn table_exists(pool: &SqlitePool, name: &str) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?)")
        .bind(name)
        .fetch_one(pool)
        .await
}

/// 启动时要做什么。纯函数，便于覆盖各种版本组合。
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Decision {
    /// 有本应用不认识的迁移（库比应用新），需要按兼容模式打开。
    pub newer: bool,
    /// 待应用的迁移号（升序）。
    pub pending: Vec<i64>,
    /// 库里已有数据（非新库），升级前需要备份。
    pub has_data: bool,
}

pub(crate) fn decide(
    applied: &BTreeSet<i64>,
    embedded: &BTreeSet<i64>,
    floor: Option<i64>,
) -> Result<Decision, ConnectError> {
    let latest = embedded.iter().copied().max().unwrap_or(0);
    let newest_unknown = applied.difference(embedded).copied().max();
    if let Some(db_version) = newest_unknown {
        // 没有兼容标记的较新库（来自 0052 之前的分叉或被改过的库）按不兼容处理。
        if !matches!(floor, Some(floor) if floor <= latest) {
            return Err(ConnectError::TooNew {
                db_version,
                floor,
                embedded: latest,
            });
        }
    }
    Ok(Decision {
        newer: newest_unknown.is_some(),
        pending: embedded.difference(applied).copied().collect(),
        has_data: !applied.is_empty(),
    })
}

/// 检查版本 → 必要时备份 → 迁移。`connect` 拿到池后调用。
pub async fn upgrade(
    pool: &SqlitePool,
    db_path: &Path,
    mut migrator: Migrator,
) -> Result<(), ConnectError> {
    let applied = applied_versions(pool).await.map_err(ConnectError::Open)?;
    let floor = reader_floor(pool).await.map_err(ConnectError::Open)?;
    let embedded: BTreeSet<i64> = migrator.iter().map(|m| m.version).collect();
    let decision = decide(&applied, &embedded, floor)?;

    if decision.newer {
        tracing::warn!(
            floor = ?floor,
            embedded = ?embedded.last(),
            "数据库由更新版本升级过，按兼容下限放行，忽略本应用不认识的迁移"
        );
    }
    let backup = match (decision.pending.first(), decision.pending.last()) {
        (Some(_), Some(&to)) if decision.has_data => {
            let from = applied.last().copied().unwrap_or(0);
            Some(snapshot(pool, db_path, from, to).await?)
        }
        _ => None,
    };

    migrator.set_ignore_missing(decision.newer);
    if let Err(error) = migrator.run(pool).await {
        tracing::error!(%error, backup = ?backup, "数据库迁移失败");
        return Err(ConnectError::Migrate { error, backup });
    }
    if let Some(path) = &backup {
        tracing::info!(path = %path.display(), pending = ?decision.pending, "数据库已升级，升级前快照已保留");
    }
    Ok(())
}

/// 升级前快照，然后清掉超出保留份数的旧快照。
async fn snapshot(
    pool: &SqlitePool,
    db_path: &Path,
    from: i64,
    to: i64,
) -> Result<PathBuf, ConnectError> {
    let dir = backup_dir(db_path);
    let fail = |reason: String| ConnectError::Backup {
        dir: dir.clone(),
        reason,
    };
    std::fs::create_dir_all(&dir).map_err(|e| fail(e.to_string()))?;
    let stamp = chrono::Local::now().format("%Y-%m-%d_%H%M%S");
    let base = format!("{stamp}{BACKUP_TAG}迁移{from}到{to}");
    let mut path = dir.join(format!("{base}.db"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{base}_{n}.db"));
        n += 1;
    }
    sqlx::query("VACUUM INTO ?1")
        .bind(path.to_string_lossy().to_string())
        .execute(pool)
        .await
        .map_err(|e| fail(e.to_string()))?;
    prune(&dir);
    Ok(path)
}

/// 只保留最近 [`BACKUPS_KEPT`] 份升级前快照。清理失败不影响启动。
fn prune(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut snapshots: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension().is_some_and(|ext| ext == "db")
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.contains(BACKUP_TAG))
        })
        .collect();
    // 文件名以日期时间开头，字典序即时间序。
    snapshots.sort();
    let excess = snapshots.len().saturating_sub(BACKUPS_KEPT);
    for old in snapshots.into_iter().take(excess) {
        if let Err(error) = std::fs::remove_file(&old) {
            tracing::warn!(%error, path = %old.display(), "清理旧的升级前快照失败");
        }
    }
}

/// 一条迁移是否会让旧代码读写出错（看去掉注释后的 SQL）。
///
/// 只认得出结构性的破坏：删表、删列、改名（含重建表的最后一步）。改变已有列的含义
/// 这类语义变化认不出来，写迁移时仍要自己判断是否抬高下限。
#[cfg(test)]
fn looks_breaking(sql: &str) -> bool {
    let code: String = sql
        .lines()
        .map(|line| line.split("--").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join(" ")
        .to_uppercase();
    let code = code.split_whitespace().collect::<Vec<_>>().join(" ");
    ["DROP TABLE", "DROP COLUMN", "RENAME TO", "RENAME COLUMN"]
        .iter()
        .any(|pattern| code.contains(pattern))
}

#[cfg(test)]
// 测试内允许 unwrap/expect：断言失败即测试失败，是期望行为。
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn set(v: &[i64]) -> BTreeSet<i64> {
        v.iter().copied().collect()
    }

    #[test]
    fn decide_covers_fresh_upgrade_current_and_newer_databases() {
        let embedded = set(&[1, 2, 52, 53]);

        let fresh = decide(&set(&[]), &embedded, None).unwrap();
        assert_eq!(fresh.pending, vec![1, 2, 52, 53]);
        assert!(!fresh.has_data && !fresh.newer);

        let upgrade = decide(&set(&[1, 2]), &embedded, None).unwrap();
        assert_eq!(upgrade.pending, vec![52, 53]);
        assert!(upgrade.has_data && !upgrade.newer);

        let current = decide(&embedded, &embedded, Some(52)).unwrap();
        assert!(current.pending.is_empty() && !current.newer);

        // 更新版本只做了加法：下限仍是 52，旧应用照常打开。
        let newer = decide(&set(&[1, 2, 52, 53, 54, 55]), &embedded, Some(52)).unwrap();
        assert!(newer.newer && newer.pending.is_empty());

        // 更新版本在 55 做了破坏性改动：拒绝。
        let broken = decide(&set(&[1, 2, 52, 53, 54, 55]), &embedded, Some(55)).unwrap_err();
        assert!(matches!(
            broken,
            ConnectError::TooNew {
                db_version: 55,
                floor: Some(55),
                embedded: 53
            }
        ));

        // 没有兼容标记的较新库一律拒绝。
        assert!(decide(&set(&[1, 2, 60]), &embedded, None).is_err());
    }

    #[test]
    fn breaking_statements_are_recognised_outside_comments() {
        assert!(looks_breaking("DROP TABLE old_things;"));
        assert!(looks_breaking("alter table a\n  rename   to b;"));
        assert!(looks_breaking("ALTER TABLE a DROP COLUMN x;"));
        assert!(!looks_breaking(
            "-- 以后再 DROP TABLE 旧表\nCREATE TABLE x(id INTEGER);"
        ));
        assert!(!looks_breaking(
            "ALTER TABLE a ADD COLUMN b TEXT NOT NULL DEFAULT '';"
        ));
    }

    /// 机器检查：0052 起每条破坏性迁移都必须在同一文件里处理 reader_floor，
    /// 否则旧版本会以兼容模式打开一份它读不懂的库。
    #[test]
    fn breaking_migrations_raise_the_reader_floor() {
        let offenders: Vec<i64> = sqlx::migrate!("./migrations")
            .iter()
            .filter(|m| m.version >= COMPAT_SINCE)
            .filter(|m| looks_breaking(&m.sql) && !m.sql.contains("reader_floor"))
            .map(|m| m.version)
            .collect();
        assert!(
            offenders.is_empty(),
            "这些迁移删表/删列/改名却没有抬高 schema_meta.reader_floor：{offenders:?}"
        );
    }

    #[test]
    fn prune_keeps_only_the_newest_upgrade_snapshots() {
        let dir = tempfile::tempdir().unwrap();
        for day in 1..=BACKUPS_KEPT + 2 {
            std::fs::write(
                dir.path()
                    .join(format!("2026-11-{day:02}_000000{BACKUP_TAG}迁移1到2.db")),
                b"",
            )
            .unwrap();
        }
        std::fs::write(dir.path().join("2026-01-01_000000_内容整理.db"), b"").unwrap();
        prune(dir.path());
        let mut left: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        left.sort();
        assert_eq!(left.len(), BACKUPS_KEPT + 1);
        assert!(left.contains(&"2026-01-01_000000_内容整理.db".to_string()));
        assert!(!left.iter().any(|n| n.starts_with("2026-11-01")));
        assert!(!left.iter().any(|n| n.starts_with("2026-11-02")));
    }
}
