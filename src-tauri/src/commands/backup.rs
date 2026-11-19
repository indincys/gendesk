//! 数据备份与数据目录可见性（E19，需求 §4 数据安全）。
//!
//! 让用户看到数据落盘位置 + 一键导出「DB + 资产目录」为 zip：磁盘故障前有备份手段。
//! 导出前保存一致的 SQLite 快照；队列运行中拒绝，以免打包仍在写入的资产。
//!
//! **密钥文件绝不入包**（`secrets.key` / `secrets.enc` 及其 .bak/.tmp 派生物）：
//! 主密钥与密文同目录，一起打进 zip 就等于把 API Key 明文交出去（备份常被外发 /
//! 上网盘）。备份的定位是数据抢救，Key 可在设置页重填，不是抢救对象。

use std::collections::HashSet;
use std::path::{Component, Path};

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;
use tauri_specta::Event;
use walkdir::WalkDir;
use zip::write::SimpleFileOptions;

use crate::db::now_unix;
use crate::error::{AppError, AppResult};
use crate::files;
use crate::state::AppState;

/// `backup://progress`：导出进度（前端进度条）。
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct BackupProgress {
    pub done: u64,
    pub total: u64,
    /// running / done / error
    pub phase: String,
}

/// 数据目录信息（E19：暴露落盘位置）。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DataDirInfo {
    pub root: String,
    pub db_path: String,
}

#[tauri::command]
#[specta::specta]
pub async fn data_dir_info(state: State<'_, AppState>) -> AppResult<DataDirInfo> {
    Ok(DataDirInfo {
        root: state.dirs.root.to_string_lossy().to_string(),
        db_path: state.dirs.db().to_string_lossy().to_string(),
    })
}

/// 在系统文件管理器中打开数据目录。
#[tauri::command]
#[specta::specta]
pub async fn open_data_dir(state: State<'_, AppState>, app: AppHandle) -> AppResult<()> {
    let path = state.dirs.root.to_string_lossy().to_string();
    app.opener()
        .open_path(path, None::<&str>)
        .map_err(|e| AppError::Io(e.to_string()))
}

/// 导出备份：选目标 zip → 保存数据库快照 → 打包数据目录（排除 logs）。
/// 队列运行中（有 run/retry 任务）拒绝，避免边写边打包出不一致备份。
/// 返回所选路径；用户取消返回 None。
#[tauri::command]
#[specta::specta]
pub async fn export_backup(
    state: State<'_, AppState>,
    app: AppHandle,
) -> AppResult<Option<String>> {
    // 队列运行中拒绝（前端亦禁用按钮，双保险）。
    let active: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE status IN ('run','retry')")
            .fetch_one(&state.db)
            .await?;
    if active > 0 {
        return Err(AppError::InvalidInput(
            "队列运行中，请先暂停队列再导出备份".into(),
        ));
    }

    let default_name = format!("gendesk_backup_{}.zip", files::date_yymmdd(now_unix()));
    let picked = app
        .dialog()
        .file()
        .add_filter("Zip 备份", &["zip"])
        .set_file_name(&default_name)
        .blocking_save_file();
    let Some(dest) = picked.and_then(|p| p.into_path().ok()) else {
        return Ok(None); // 取消
    };

    // SQLite 自己生成独立快照，避免直接读取仍可被后台写入的数据库文件。
    let snapshot = tempfile::tempdir()?;
    let db_snapshot = snapshot.path().join("gendesk.db");
    sqlx::query("VACUUM INTO ?")
        .bind(db_snapshot.to_string_lossy().as_ref())
        .execute(&state.db)
        .await?;

    let root = state.dirs.root.clone();
    let logs = state.dirs.logs();
    let handle = app.clone();
    let out = dest.clone();
    // 打包为 CPU/IO 密集同步任务，放到阻塞线程，避免占用异步执行器。
    tokio::task::spawn_blocking(move || {
        let _snapshot = snapshot;
        let parent = out
            .parent()
            .ok_or_else(|| AppError::InvalidInput("请选择备份目录".into()))?;
        let tmp = tempfile::NamedTempFile::new_in(parent)?;
        zip_dir(
            &root,
            &logs,
            tmp.path(),
            Some(&db_snapshot),
            &|done, total, phase| {
                let _ = BackupProgress {
                    done,
                    total,
                    phase: phase.to_string(),
                }
                .emit(&handle);
            },
        )?;
        tmp.persist(out).map_err(|error| error.error)?;
        Ok::<_, AppError>(())
    })
    .await
    .map_err(|e| AppError::Io(format!("备份任务失败: {e}")))??;

    Ok(Some(dest.to_string_lossy().to_string()))
}

/// 递归打包 `root`（跳过 `skip_dir` 子树，通常是 logs）到 `dest` zip，逐文件回报进度。
/// `emit(done, total, phase)` 解耦进度回报，便于测试（无需 Tauri AppHandle）。
fn zip_dir(
    root: &Path,
    skip_dir: &Path,
    dest: &Path,
    db_snapshot: Option<&Path>,
    emit: &dyn Fn(u64, u64, &str),
) -> AppResult<()> {
    // 先枚举文件，得到总数用于进度分母。
    let files: Vec<_> = WalkDir::new(root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .filter(|e| !e.path().starts_with(skip_dir) && e.path() != dest)
        .filter(|e| {
            !matches!(
                e.file_name().to_str(),
                Some("gendesk.db-wal" | "gendesk.db-shm" | "gendesk.db-journal" | ".DS_Store")
            )
        })
        // 密钥文件排除（见模块头）：主密钥 + 密文同在一个 zip = 明文 API Key。
        .filter(|e| {
            !e.file_name()
                .to_str()
                .is_some_and(crate::secrets::is_secret_file)
        })
        .collect();
    let total = files.len() as u64;
    emit(0, total, "running");

    let file = std::fs::File::create(dest)?;
    let mut zip = zip::ZipWriter::new(file);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    for (i, entry) in files.iter().enumerate() {
        let path = entry.path();
        let rel = path.strip_prefix(root).unwrap_or(path);
        let name = rel.to_string_lossy().replace('\\', "/");
        zip.start_file(name, opts)
            .map_err(|e| AppError::Io(e.to_string()))?;
        let source = if rel == Path::new("gendesk.db") {
            db_snapshot.unwrap_or(path)
        } else {
            path
        };
        std::io::copy(&mut std::fs::File::open(source)?, &mut zip)?;
        if (i + 1) % 20 == 0 {
            emit((i + 1) as u64, total, "running");
        }
    }
    zip.finish().map_err(|e| AppError::Io(e.to_string()))?;
    emit(total, total, "done");
    Ok(())
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct BackupCheck {
    pub archive_path: String,
    pub extracted_path: Option<String>,
    pub schema_version: i64,
    pub files: u64,
    pub bytes: u64,
}

/// 校验备份可读性，或将已验证的内容恢复到一个新目录。
#[tauri::command]
#[specta::specta]
pub async fn check_backup(app: AppHandle, extract: bool) -> AppResult<Option<BackupCheck>> {
    let Some(archive) = app
        .dialog()
        .file()
        .add_filter("GenDesk ZIP 备份", &["zip"])
        .blocking_pick_file()
        .and_then(|path| path.into_path().ok())
    else {
        return Ok(None);
    };
    let staging = if extract {
        let Some(parent) = app
            .dialog()
            .file()
            .blocking_pick_folder()
            .and_then(|path| path.into_path().ok())
        else {
            return Ok(None);
        };
        tempfile::Builder::new()
            .prefix("GenDesk 恢复-")
            .tempdir_in(parent)?
    } else {
        tempfile::tempdir()?
    };
    let (staging, files, bytes) = tokio::task::spawn_blocking(move || {
        let (files, bytes) = extract_backup(&archive, staging.path())?;
        Ok::<_, AppError>(((staging, archive), files, bytes))
    })
    .await
    .map_err(|error| AppError::Io(error.to_string()))??;
    let (staging, archive) = staging;
    let schema_version = verify_backup_db(&staging.path().join("gendesk.db")).await?;
    let extracted_path = extract.then(|| staging.keep().to_string_lossy().into_owned());
    Ok(Some(BackupCheck {
        archive_path: archive.to_string_lossy().into_owned(),
        extracted_path,
        schema_version,
        files,
        bytes,
    }))
}

fn extract_backup(archive: &Path, root: &Path) -> AppResult<(u64, u64)> {
    let invalid = |detail: String| AppError::InvalidInput(format!("备份无法校验：{detail}"));
    let mut zip = zip::ZipArchive::new(std::fs::File::open(archive)?)
        .map_err(|error| invalid(error.to_string()))?;
    let mut seen = HashSet::new();
    let mut files = 0;
    let mut bytes = 0_u64;
    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|error| invalid(error.to_string()))?;
        let path = entry
            .enclosed_name()
            .ok_or_else(|| invalid("文件路径无效".into()))?;
        let forbidden = entry.name().contains(['\\', ':'])
            || entry.is_symlink()
            || path.components().any(|part| match part {
                Component::Normal(name) => match name.to_str() {
                    Some(name) => {
                        crate::secrets::is_secret_file(&name.to_ascii_lowercase())
                            || name.ends_with(['.', ' '])
                    }
                    None => true,
                },
                _ => true,
            });
        if forbidden || !seen.insert(path.clone()) {
            return Err(invalid(format!("文件路径重复或不安全：{}", path.display())));
        }
        let output = root.join(path);
        if entry.is_dir() {
            std::fs::create_dir_all(output)?;
        } else {
            if let Some(parent) = output.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut file = std::fs::File::options()
                .write(true)
                .create_new(true)
                .open(output)?;
            bytes = bytes.saturating_add(std::io::copy(&mut entry, &mut file)?);
            files += 1;
        }
    }
    Ok((files, bytes))
}

async fn verify_backup_db(path: &Path) -> AppResult<i64> {
    use sqlx::{sqlite::SqliteConnectOptions, Connection, SqliteConnection};
    let options = SqliteConnectOptions::new().filename(path).read_only(true);
    let mut conn = SqliteConnection::connect_with(&options).await?;
    let integrity: Vec<String> = sqlx::query_scalar("PRAGMA quick_check")
        .fetch_all(&mut conn)
        .await?;
    let schema: Option<i64> =
        sqlx::query_scalar("SELECT MAX(version) FROM _sqlx_migrations WHERE success=1")
            .fetch_one(&mut conn)
            .await?;
    let has_tasks: i64 = sqlx::query_scalar("SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN ('tasks','batches','accepted_works')")
        .fetch_one(&mut conn).await?;
    if integrity != ["ok"] || has_tasks != 3 {
        return Err(AppError::InvalidInput(
            "数据库损坏或并非 GenDesk 备份".into(),
        ));
    }
    schema.ok_or_else(|| AppError::InvalidInput("数据库缺少 GenDesk 版本记录".into()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即失败
mod tests {
    use super::*;
    use std::cell::Cell;

    #[tokio::test]
    async fn backup_round_trip_checks_the_database_and_files() {
        let (pool, fixture) = crate::db::test_support::test_pool().await;
        let root = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        let snapshot = root.path().join("gendesk.db");
        sqlx::query("VACUUM INTO ?")
            .bind(snapshot.to_str())
            .execute(&pool)
            .await
            .unwrap();
        std::fs::write(root.path().join("sample.png"), b"fixture").unwrap();
        let archive = fixture.path().join("backup.zip");
        zip_dir(
            root.path(),
            &root.path().join("logs"),
            &archive,
            None,
            &|_, _, _| {},
        )
        .unwrap();
        let (count, bytes) = extract_backup(&archive, dst.path()).unwrap();
        assert_eq!(count, 2);
        assert!(bytes > 7);
        assert_eq!(
            std::fs::read(dst.path().join("sample.png")).unwrap(),
            b"fixture"
        );
        assert_eq!(
            verify_backup_db(&dst.path().join("gendesk.db"))
                .await
                .unwrap(),
            crate::db::latest_embedded_migration()
        );
    }

    #[test]
    fn backup_reader_refuses_traversal() {
        use std::io::Write;
        let fixture = tempfile::tempdir().unwrap();
        let archive = fixture.path().join("backup.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
        zip.start_file("../escape", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"not valid").unwrap();
        zip.finish().unwrap();
        let root = fixture.path().join("restored");
        std::fs::create_dir(&root).unwrap();
        assert!(extract_backup(&archive, &root).is_err());
        assert!(!fixture.path().join("escape").exists());
    }

    // E19：备份 zip 必须含数据文件、排除 logs 子树，进度终值为总数。
    #[test]
    fn zip_dir_includes_assets_excludes_logs() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("gendesk.db"), b"DBDATA").unwrap();
        std::fs::create_dir_all(root.join("refs")).unwrap();
        std::fs::write(root.join("refs").join("a.jpg"), b"IMG").unwrap();
        let logs = root.join("logs");
        std::fs::create_dir_all(&logs).unwrap();
        std::fs::write(logs.join("app.log"), b"NOISE").unwrap();

        let dest = root.join("backup.zip");
        let last = Cell::new((0u64, 0u64));
        zip_dir(root, &logs, &dest, None, &|done, total, _| {
            last.set((done, total))
        })
        .unwrap();

        // 进度终值：done == total == 2（db + refs/a.jpg，logs 不计）。
        assert_eq!(last.get(), (2, 2));

        let f = std::fs::File::open(&dest).unwrap();
        let mut zip = zip::ZipArchive::new(f).unwrap();
        let names: Vec<String> = (0..zip.len())
            .map(|i| zip.by_index(i).unwrap().name().to_string())
            .collect();
        assert!(names.contains(&"gendesk.db".to_string()), "备份须含数据库");
        assert!(names.contains(&"refs/a.jpg".to_string()), "备份须含资产");
        assert!(
            !names.iter().any(|n| n.starts_with("logs/")),
            "日志目录应被排除，names={names:?}"
        );
    }

    /// 密钥文件（含 .bak/.tmp 派生物）绝不入备份包：主密钥与密文同在一个 zip
    /// 就等于把 API Key 明文交给任何拿到备份的人。
    #[test]
    fn zip_dir_never_includes_secret_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("gendesk.db"), b"DBDATA").unwrap();
        std::fs::write(root.join(crate::secrets::KEY_FILE), b"MASTERKEY").unwrap();
        std::fs::write(root.join(crate::secrets::ENC_FILE), b"CIPHERTEXT").unwrap();
        std::fs::write(root.join("secrets.enc.bak-123"), b"OLDCIPHER").unwrap();
        std::fs::write(root.join("secrets.key.tmp-77"), b"HALFWRITTEN").unwrap();
        let logs = root.join("logs");
        std::fs::create_dir_all(&logs).unwrap();

        let dest = root.join("backup.zip");
        let last = Cell::new((0u64, 0u64));
        zip_dir(root, &logs, &dest, None, &|done, total, _| {
            last.set((done, total))
        })
        .unwrap();

        // 只有 gendesk.db 入包；四个密钥相关文件全被排除（也不进进度分母）。
        assert_eq!(last.get(), (1, 1));

        let f = std::fs::File::open(&dest).unwrap();
        let mut zip = zip::ZipArchive::new(f).unwrap();
        let names: Vec<String> = (0..zip.len())
            .map(|i| zip.by_index(i).unwrap().name().to_string())
            .collect();
        assert_eq!(names, vec!["gendesk.db".to_string()], "names={names:?}");
    }
}
