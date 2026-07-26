//! 收件目录监听：skill 写完 READY.txt → 2 秒防抖 → 全量扫描 → 自动建批。
//!
//! 与 `publish::inbox::watcher` / `v2v::watcher` 同一套路（notify + 防抖 + 全量重扫），
//! 并复用它们的 `coalesce`：防抖逻辑只该有一份，三处各写一遍必然在某次改动后行为分叉。
//!
//! 全量重扫而非按事件逐文件处理：notify 会丢事件也会风暴，而 `scan` 幂等
//! （去重表 + 移档），全量重扫天然抵抗这两种情况。

use std::path::PathBuf;

use notify::{RecursiveMode, Watcher};
use tauri::AppHandle;

use super::ingest::Ctx;
use crate::error::{AppError, AppResult};
use crate::publish::inbox::watcher::coalesce;

/// 与收件箱同一个静默窗口：skill 写一批文件时事件是连续的，2 秒足够收敛。
const QUIET: std::time::Duration = std::time::Duration::from_millis(2000);

/// 运行中的收件监听。drop 即停止。
pub struct IntakeWatcher {
    _watcher: notify::RecommendedWatcher,
    _worker: tauri::async_runtime::JoinHandle<()>,
}

/// 在交接根上启动监听。目录不存在时先建。
pub fn start(ctx: Ctx, root: PathBuf, app: AppHandle) -> AppResult<IntakeWatcher> {
    let pending = super::pending_dir(&root);
    std::fs::create_dir_all(&pending)?;

    let (tx, rx) = tokio::sync::mpsc::channel::<()>(256);
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if res.is_ok() {
            // 满则丢弃：scan 是全量幂等的，丢事件最坏推迟一轮。
            let _ = tx.try_send(());
        }
    })
    .map_err(|e| AppError::Io(format!("创建收件目录监听失败：{e}")))?;
    watcher
        .watch(&pending, RecursiveMode::Recursive)
        .map_err(|e| AppError::Io(format!("监听收件目录失败：{e}")))?;

    let worker = tauri::async_runtime::spawn(async move {
        let mut rx = rx;
        loop {
            if rx.recv().await.is_none() {
                break;
            }
            if !coalesce(&mut rx, QUIET).await {
                break;
            }
            crate::commands::intake::scan_and_emit(&ctx, &root, &app).await;
        }
    });

    Ok(IntakeWatcher {
        _watcher: watcher,
        _worker: worker,
    })
}
