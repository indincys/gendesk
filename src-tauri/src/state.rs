//! 应用运行时状态（Tauri 托管）。业务真相的持有者：DB 池、密钥存储、数据目录、引擎。

use std::sync::Arc;

use sqlx::SqlitePool;

use crate::engine::Engine;
use crate::files::DataDirs;
use crate::secrets::SecretStore;
use crate::v2v::activity::Activity;

pub struct AppState {
    pub db: SqlitePool,
    pub secrets: Arc<dyn SecretStore>,
    pub dirs: Arc<DataDirs>,
    pub engine: Arc<Engine>,
    /// 视频流水线执行日志。轮询器、提交、交接目录监听共用同一份缓冲，
    /// 命令层据此回答「刚才这台机器替我做了什么、有没有报错」。
    pub v2v_log: Activity,
}

impl AppState {
    pub fn new(
        db: SqlitePool,
        secrets: Arc<dyn SecretStore>,
        dirs: Arc<DataDirs>,
        engine: Arc<Engine>,
        v2v_log: Activity,
    ) -> Self {
        Self {
            db,
            secrets,
            dirs,
            engine,
            v2v_log,
        }
    }
}
