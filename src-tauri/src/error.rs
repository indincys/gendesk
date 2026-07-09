//! 统一错误类型（执行计划 §1.6 / 技术文档 4.2）。
//!
//! 所有经 IPC 返回前端的错误都收敛到 [`AppError`]，并通过 specta 序列化为
//! 稳定的判别式联合，前端据此做错误分级展示。业务错误分类（Timeout /
//! RateLimited / ContentPolicy / Auth / Interrupted / Other）在 M2 引擎接入
//! 时补充到 provider/engine 层，这里先提供基础通用分类。

use serde::Serialize;
use specta::Type;

/// 应用级错误。`type` 字段作为前端可判别的分类标签。
#[derive(Debug, thiserror::Error, Serialize, Type)]
#[serde(tag = "type", content = "message")]
pub enum AppError {
    /// 数据库 / 持久化失败。
    #[error("数据库错误：{0}")]
    Database(String),

    /// 文件系统 / IO 失败。
    #[error("文件错误：{0}")]
    Io(String),

    /// 输入参数非法（前端应在提交前拦截，此为兜底）。
    #[error("参数错误：{0}")]
    InvalidInput(String),

    /// 系统钥匙串（Keychain / 凭据管理器）访问失败。
    #[error("凭据存储错误：{0}")]
    Keyring(String),

    /// 其它未分类错误。
    #[error("内部错误：{0}")]
    Internal(String),
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError::Io(e.to_string())
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        AppError::Internal(e.to_string())
    }
}

/// 命令返回别名 —— 所有 `#[tauri::command]` 统一返回它。
pub type AppResult<T> = Result<T, AppError>;
