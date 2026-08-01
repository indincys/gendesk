//! 收件箱 notify 监听，2 秒静默窗口后做幂等全量扫描。

use std::path::{Path, PathBuf};
use std::time::Duration;

use notify::{RecursiveMode, Watcher};
use sqlx::SqlitePool;
use tauri::AppHandle;
use tauri_specta::Event;
use tokio::sync::mpsc;

use crate::error::{AppError, AppResult};
use crate::publish::events::{InboxIngestEvent, PublishBadgesEvent};
use crate::publish::inbox::ingest;
use crate::publish::paths;

const QUIET: Duration = Duration::from_millis(2000);

pub struct PublishWatcher {
    _watcher: notify::RecommendedWatcher,
    _worker: tauri::async_runtime::JoinHandle<()>,
}

pub fn start(pool: SqlitePool, root: PathBuf, app: AppHandle) -> AppResult<PublishWatcher> {
    let inbox = paths::RelPath::new(paths::INBOX).to_local(&root);
    std::fs::create_dir_all(&inbox)?;
    let (tx, rx) = mpsc::channel(256);
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if event.is_ok() {
            let _ = tx.try_send(());
        }
    })
    .map_err(|err| AppError::Io(format!("创建收件箱监听失败：{err}")))?;
    watcher
        .watch(&inbox, RecursiveMode::Recursive)
        .map_err(|err| AppError::Io(format!("监听收件箱失败：{err}")))?;
    let worker = tauri::async_runtime::spawn(run_worker(rx, pool, root, app));
    Ok(PublishWatcher {
        _watcher: watcher,
        _worker: worker,
    })
}

async fn run_worker(mut rx: mpsc::Receiver<()>, pool: SqlitePool, root: PathBuf, app: AppHandle) {
    while rx.recv().await.is_some() {
        if !coalesce(&mut rx, QUIET).await {
            break;
        }
        rescan_and_emit(&pool, &root, &app).await;
    }
}

pub async fn coalesce(rx: &mut mpsc::Receiver<()>, quiet: Duration) -> bool {
    loop {
        tokio::select! {
            _ = tokio::time::sleep(quiet) => return true,
            signal = rx.recv() => if signal.is_none() { return false; },
        }
    }
}

pub async fn rescan_and_emit(pool: &SqlitePool, root: &Path, app: &AppHandle) {
    match ingest::rescan(pool, root).await {
        Ok(results) => {
            for result in results.into_iter().filter(|result| result.changed) {
                let _ = InboxIngestEvent {
                    file_name: result.file_name,
                    state: result.state,
                    product_code: result.product_code,
                    titles: result.titles,
                    bodies: result.bodies,
                    message: result.message,
                }
                .emit(app);
            }
        }
        Err(err) => tracing::warn!(error=%err, "收件箱扫描失败"),
    }
    emit_badges(pool, app).await;
}

pub async fn emit_badges(pool: &SqlitePool, app: &AppHandle) {
    let counts: Result<(i64, i64, i64), sqlx::Error> = sqlx::query_as(
        "SELECT
          (SELECT COUNT(*) FROM inbox_items WHERE state IN ('unclaimed','failed')),
          (SELECT COUNT(*) FROM task_sheets WHERE status IN ('draft','confirmed')),
          (SELECT COUNT(*) FROM task_sheets WHERE status IN ('exported','reconciling'))",
    )
    .fetch_one(pool)
    .await;
    match counts {
        Ok((unclaimed, pending_sheets, pending_reconcile)) => {
            let _ = PublishBadgesEvent {
                unclaimed,
                pending_sheets,
                pending_reconcile,
            }
            .emit(app);
        }
        Err(err) => tracing::warn!(error=%err, "发布徽章计算失败"),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // 测试断言失败即测试失败
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn waits_until_quiet() {
        let (tx, mut rx) = mpsc::channel(8);
        let handle = tokio::spawn(async move { coalesce(&mut rx, QUIET).await });
        tx.send(()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(1000)).await;
        tx.send(()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(2100)).await;
        assert!(handle.await.unwrap());
    }
}
