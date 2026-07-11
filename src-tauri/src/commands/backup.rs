//! 数据备份与数据目录可见性（E19，需求 §4 数据安全）。
//!
//! 让用户看到数据落盘位置 + 一键导出「DB + 资产目录」为 zip：磁盘故障前有备份手段。
//! 导出前 WAL 检查点保证一致；队列运行中拒绝（避免边写边打包出半成品）。

use std::io::Write;
use std::path::Path;

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

/// 导出备份：选目标 zip → WAL 检查点 → 打包数据目录（排除 logs）。
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

    // WAL 检查点：把 -wal 落回主库，保证打包到的 gendesk.db 是完整快照。
    sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
        .execute(&state.db)
        .await?;

    let root = state.dirs.root.clone();
    let logs = state.dirs.logs();
    let handle = app.clone();
    let out = dest.clone();
    // 打包为 CPU/IO 密集同步任务，放到阻塞线程，避免占用异步执行器。
    tokio::task::spawn_blocking(move || {
        zip_dir(&root, &logs, &out, &|done, total, phase| {
            let _ = BackupProgress {
                done,
                total,
                phase: phase.to_string(),
            }
            .emit(&handle);
        })
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
    emit: &dyn Fn(u64, u64, &str),
) -> AppResult<()> {
    // 先枚举文件，得到总数用于进度分母。
    let files: Vec<_> = WalkDir::new(root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .filter(|e| !e.path().starts_with(skip_dir))
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
        // 读整文件再写：资产多为图片，单文件可控。
        let bytes = std::fs::read(path)?;
        zip.write_all(&bytes)?;
        if (i + 1) % 20 == 0 {
            emit((i + 1) as u64, total, "running");
        }
    }
    zip.finish().map_err(|e| AppError::Io(e.to_string()))?;
    emit(total, total, "done");
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即失败
mod tests {
    use super::*;
    use std::cell::Cell;

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
        zip_dir(root, &logs, &dest, &|done, total, _| {
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
}
