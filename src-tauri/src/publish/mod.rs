//! 发布与资产管理模块（发布模块执行计划 §5.1）。
//!
//! 业务真相只在 Rust：收件箱收录、套装编排、任务包导出、回执对账、看板日报。
//! 与现有引擎调度器互不干扰——各异步链路汇入各自的串行工作者（单写者纪律的发布版）。

pub mod events;
pub mod exporter;
pub mod inbox;
pub mod paths;
pub mod planner;
pub mod platform;
pub mod reconcile;
pub mod restock;
pub mod sku_mapping;
pub mod ticker;
pub mod xlsx;

use std::path::PathBuf;
use std::sync::Mutex;

use sqlx::SqlitePool;
use tauri::AppHandle;

use crate::error::AppResult;

/// 发布模块运行时状态：持有收件箱 + 任务包 watcher 句柄，支持根目录变更时热重启。
pub struct PublishState {
    pool: SqlitePool,
    app: AppHandle,
    watchers: Mutex<Vec<inbox::watcher::PublishWatcher>>,
}

impl PublishState {
    pub fn new(pool: SqlitePool, app: AppHandle) -> Self {
        Self {
            pool,
            app,
            watchers: Mutex::new(Vec::new()),
        }
    }

    /// 在给定本机根目录上（重）启动收件箱 + 任务包监听；旧监听随替换而 drop 停止。
    pub fn restart(&self, root: PathBuf) -> AppResult<()> {
        let inbox_w = inbox::watcher::start(self.pool.clone(), root.clone(), self.app.clone())?;
        let pkg_w = inbox::watcher::start_pkg(self.pool.clone(), root, self.app.clone())?;
        if let Ok(mut guard) = self.watchers.lock() {
            *guard = vec![inbox_w, pkg_w];
        }
        Ok(())
    }

    /// 停止监听（清空句柄）。
    pub fn stop(&self) {
        if let Ok(mut guard) = self.watchers.lock() {
            guard.clear();
        }
    }
}
