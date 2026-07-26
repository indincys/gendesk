//! 交接目录监听：skill 写完 `rewrite.jsonl` → 2 秒防抖 → 收录 → 条目进「待提交」。
//!
//! 与 `publish::inbox::watcher` 同一套路（notify + 防抖 + 全量 rescan 幂等），
//! 复用它的 `coalesce`：防抖逻辑只该有一份，两处各写一遍必然在某次改动后行为分叉。
//!
//! 之所以是「监听 + 全量重扫」而不是「按事件逐文件处理」：notify 会丢事件也会风暴，
//! 而 `ingest` 本身幂等（收录后移档），全量重扫天然抵抗这两种情况。

use std::path::PathBuf;

use notify::{RecursiveMode, Watcher};
use sqlx::SqlitePool;
use tauri::AppHandle;

use crate::error::{AppError, AppResult};
use crate::publish::inbox::watcher::coalesce;

/// 与收件箱同一个静默窗口：agent 写一批文件时事件是连续的，2 秒足够收敛。
const QUIET: std::time::Duration = std::time::Duration::from_millis(2000);

/// 运行中的交接监听。drop 即停止。
pub struct HandoffWatcher {
    _watcher: notify::RecommendedWatcher,
    _worker: tauri::async_runtime::JoinHandle<()>,
}

/// 在交接根上启动监听。目录不存在时先建（含 `v2v/待改写`、`v2v/已改写`）。
pub fn start(
    pool: SqlitePool,
    root: PathBuf,
    app: AppHandle,
    log: super::activity::Activity,
) -> AppResult<HandoffWatcher> {
    let v2v_dir = root.join(super::handoff::V2V);
    std::fs::create_dir_all(v2v_dir.join(super::handoff::PENDING))?;
    std::fs::create_dir_all(v2v_dir.join(super::handoff::DONE))?;

    let (tx, rx) = tokio::sync::mpsc::channel::<()>(256);
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if res.is_ok() {
            // 满则丢弃：ingest 是全量幂等的，丢事件最坏推迟一轮。
            let _ = tx.try_send(());
        }
    })
    .map_err(|e| AppError::Io(format!("创建交接目录监听失败：{e}")))?;
    // 只监听「已改写」：待改写是我们自己写的，监听它会让每次物化都触发一轮收录，
    // 形成 物化→事件→收录→物化 的自激循环。
    let done = v2v_dir.join(super::handoff::DONE);
    watcher
        .watch(&done, RecursiveMode::Recursive)
        .map_err(|e| AppError::Io(format!("监听交接目录失败：{e}")))?;

    let worker = tauri::async_runtime::spawn(async move {
        let mut rx = rx;
        loop {
            if rx.recv().await.is_none() {
                break;
            }
            if !coalesce(&mut rx, QUIET).await {
                break;
            }
            match super::handoff::ingest(&pool, &root).await {
                Ok(sum) if sum.applied > 0 || sum.stale > 0 || sum.unmatched > 0 => {
                    // 收录是**自动发生**的（skill 写完文件即触发），所以它尤其需要留痕：
                    // 没有这一条，用户只会看到卡片自己从「待改写」跳到「待提交」，
                    // 而「认不出 N 条」这种部分失败连一点声音都没有。
                    log.info(
                        "handoff",
                        None,
                        format!(
                            "收录改写结果：应用 {} · 认不出 {} · 已越过待提交 {}",
                            sum.applied, sum.unmatched, sum.stale
                        ),
                        None,
                    );
                    crate::commands::v2v::refresh_handoff(&pool, &app).await;
                }
                Ok(_) => {}
                Err(e) => log.error("handoff", None, format!("收录改写结果失败：{e}"), None),
            }
        }
    });

    Ok(HandoffWatcher {
        _watcher: watcher,
        _worker: worker,
    })
}
