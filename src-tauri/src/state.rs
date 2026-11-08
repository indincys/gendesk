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
    /// 视频流水线执行日志。轮询器、提交、CLI 改写交回共用同一份缓冲，
    /// 命令层据此回答「刚才这台机器替我做了什么、有没有报错」。
    pub v2v_log: Activity,
    /// 工单收录的互斥锁。CLI 每次请求都新建一个 `intake::ingest::Ctx`，
    /// 必须有个地方存放「大家共用的那一把」。
    pub intake_lock: Arc<tokio::sync::Mutex<()>>,
    /// 串行验收，避免不同入口同时通过同一任务；Poster 队列另用独立锁。
    pub review_lock: tokio::sync::Mutex<()>,
    pub poster_lock: tokio::sync::Mutex<()>,
    pub live_photo_wake: tokio::sync::Notify,
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
            intake_lock: Arc::new(tokio::sync::Mutex::new(())),
            review_lock: tokio::sync::Mutex::new(()),
            poster_lock: tokio::sync::Mutex::new(()),
            live_photo_wake: tokio::sync::Notify::new(),
        }
    }
}
