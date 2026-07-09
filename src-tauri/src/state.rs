//! 应用运行时状态（Tauri 托管）。业务真相的持有者：DB 池、密钥存储、数据目录、引擎。

use std::sync::Arc;

use sqlx::SqlitePool;

use crate::engine::Engine;
use crate::files::DataDirs;
use crate::secrets::SecretStore;

pub struct AppState {
    pub db: SqlitePool,
    pub secrets: Arc<dyn SecretStore>,
    pub dirs: Arc<DataDirs>,
    pub engine: Arc<Engine>,
}

impl AppState {
    pub fn new(
        db: SqlitePool,
        secrets: Arc<dyn SecretStore>,
        dirs: Arc<DataDirs>,
        engine: Arc<Engine>,
    ) -> Self {
        Self {
            db,
            secrets,
            dirs,
            engine,
        }
    }
}
