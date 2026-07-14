//! 收件箱文件监听 + 大小稳定防抖（发布模块执行计划 §5.1 inbox/watcher）。
//!
//! notify 监听 收件箱/ → 事件汇入 tokio 通道 → 防抖收敛（连续 2 秒无新事件）→ 全量
//! rescan（幂等，天然抵抗 notify 事件风暴/丢事件）→ 逐条 InboxIngestEvent + PublishBadgesEvent。
//! 防抖窗口的 coalesce 逻辑抽为纯 async 函数，虚拟时钟可测。

use std::path::{Path, PathBuf};
use std::time::Duration;

use notify::{RecursiveMode, Watcher};
use sqlx::SqlitePool;
use tauri::AppHandle;
use tauri_specta::Event;
use tokio::sync::mpsc;

use crate::commands::publish_skus::badge_counts;
use crate::error::{AppError, AppResult};
use crate::publish::events::{InboxIngestEvent, PublishBadgesEvent};
use crate::publish::inbox::ingest;
use crate::publish::paths;

/// 防抖静默窗口：文件大小/事件连续 2 秒稳定才开始收录（规范 §4）。
const QUIET: Duration = Duration::from_millis(2000);

/// 运行中的收件箱监听。drop 即停止（watcher 析构 + 通道关闭令 worker 退出）。
pub struct PublishWatcher {
    _watcher: notify::RecommendedWatcher,
    _worker: tokio::task::JoinHandle<()>,
}

/// 在指定本机根目录上启动监听。root 下不存在 收件箱/ 时先建。
pub fn start(pool: SqlitePool, root: PathBuf, app: AppHandle) -> AppResult<PublishWatcher> {
    let inbox_dir = paths::RelPath::from_parts([paths::INBOX]).to_local(&root);
    std::fs::create_dir_all(&inbox_dir)?;

    let (tx, rx) = mpsc::channel::<()>(256);
    // notify 回调在独立线程；用 blocking_send 汇入通道（满则丢弃，rescan 幂等兜底）。
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if res.is_ok() {
            let _ = tx.try_send(());
        }
    })
    .map_err(|e| AppError::Io(format!("创建收件箱监听失败：{e}")))?;
    watcher
        .watch(&inbox_dir, RecursiveMode::Recursive)
        .map_err(|e| AppError::Io(format!("监听收件箱目录失败：{e}")))?;

    let worker = tokio::spawn(async move {
        run_worker(rx, pool, root, app).await;
    });

    Ok(PublishWatcher {
        _watcher: watcher,
        _worker: worker,
    })
}

/// worker 主循环：等首个事件 → 防抖收敛 → rescan + 发事件。
async fn run_worker(mut rx: mpsc::Receiver<()>, pool: SqlitePool, root: PathBuf, app: AppHandle) {
    loop {
        // 阻塞等第一个事件；通道关闭 → 退出。
        if rx.recv().await.is_none() {
            break;
        }
        // 防抖：吸收后续事件直到静默窗口。
        if !coalesce(&mut rx, QUIET).await {
            break; // 通道关闭
        }
        rescan_and_emit(&pool, &root, &app).await;
    }
}

/// 防抖收敛：持续吸收事件，直到 `quiet` 时长内无新事件返回 true；通道关闭返回 false。
/// 抽为纯 async 函数便于虚拟时钟测试（tokio::time::pause）。
pub async fn coalesce(rx: &mut mpsc::Receiver<()>, quiet: Duration) -> bool {
    loop {
        tokio::select! {
            _ = tokio::time::sleep(quiet) => return true,
            sig = rx.recv() => {
                if sig.is_none() {
                    return false;
                }
                // 收到新事件 → 重置静默计时（继续循环）。
            }
        }
    }
}

/// 全量收录并推事件（错误落日志，不 panic）。
async fn rescan_and_emit(pool: &SqlitePool, root: &Path, app: &AppHandle) {
    match ingest::rescan(pool, root).await {
        Ok(outcomes) => {
            for o in outcomes {
                // 仅对有意义结果推 toast（成功/待认领/失败）。
                let file_name = match &o {
                    ingest::IngestOutcome::Ingested { sku_code, .. } => sku_code.clone(),
                    _ => String::new(),
                };
                let _ = InboxIngestEvent {
                    file_name,
                    outcome: o,
                }
                .emit(app);
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "收件箱 rescan 失败");
        }
    }
    emit_badges(pool, app).await;
}

/// 计算并推送发布徽章。
pub async fn emit_badges(pool: &SqlitePool, app: &AppHandle) {
    match badge_counts(pool).await {
        Ok(b) => {
            let _ = PublishBadgesEvent {
                unclaimed: b.unclaimed,
                warn: b.warn,
                pending_sheets: b.pending_sheets,
                pending_reconcile: b.pending_reconcile,
            }
            .emit(app);
        }
        Err(e) => tracing::warn!(error = %e, "计算发布徽章失败"),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    // 防抖：静默窗口内不断有事件 → 不返回；停止后经过 quiet → 返回 true。
    #[tokio::test(start_paused = true)]
    async fn coalesce_waits_for_quiet_window() {
        let (tx, mut rx) = mpsc::channel::<()>(16);
        let handle = tokio::spawn(async move { coalesce(&mut rx, QUIET).await });

        // 每 500ms 发一次事件，持续 3 次（1.5s，均在 2s 窗口内重置）。
        for _ in 0..3 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            tx.send(()).await.unwrap();
        }
        // 此时还未静默满 2s，任务应仍在等待。
        assert!(!handle.is_finished(), "静默窗口未满不应返回");

        // 停止发事件，推进 2s → 应返回 true。
        tokio::time::sleep(Duration::from_millis(2100)).await;
        assert!(handle.await.unwrap(), "静默满窗口应返回 true");
    }

    // 通道关闭（发送端 drop）→ coalesce 返回 false。
    #[tokio::test(start_paused = true)]
    async fn coalesce_returns_false_on_close() {
        let (tx, mut rx) = mpsc::channel::<()>(16);
        drop(tx);
        assert!(!coalesce(&mut rx, QUIET).await);
    }
}
