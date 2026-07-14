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

    // 变体名保留 `Keyring`（非 doc 注释：doc 会被 specta 带进 bindings.ts）——
    // 生产路径已是 `secrets::FileStore` 本地加密文件，系统钥匙串仅剩迁移读取。
    /// 密钥存储访问失败（本地加密文件 / 迁移期的系统钥匙串）。
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

impl From<sqlx::Error> for AppError {
    fn from(e: sqlx::Error) -> Self {
        AppError::Database(e.to_string())
    }
}

impl From<sqlx::migrate::MigrateError> for AppError {
    fn from(e: sqlx::migrate::MigrateError) -> Self {
        AppError::Database(e.to_string())
    }
}

impl From<keyring::Error> for AppError {
    fn from(e: keyring::Error) -> Self {
        AppError::Keyring(e.to_string())
    }
}

impl From<image::ImageError> for AppError {
    fn from(e: image::ImageError) -> Self {
        AppError::Io(e.to_string())
    }
}

/// 命令返回别名 —— 所有 `#[tauri::command]` 统一返回它。
pub type AppResult<T> = Result<T, AppError>;

#[cfg(test)]
mod tests {
    use super::*;

    /// 五个变体的 Display（`#[error]`）均非空，且各带自身中文前缀，
    /// 保证前端错误分级展示不落到空串。
    #[test]
    fn display_covers_every_variant() {
        let cases = [
            (AppError::Database("x".into()), "数据库错误"),
            (AppError::Io("x".into()), "文件错误"),
            (AppError::InvalidInput("x".into()), "参数错误"),
            (AppError::Keyring("x".into()), "凭据存储错误"),
            (AppError::Internal("x".into()), "内部错误"),
        ];
        for (err, prefix) in cases {
            let text = err.to_string();
            assert!(text.starts_with(prefix), "{text} 应以 {prefix} 开头");
        }
    }

    /// 每个 `From` 转换都落到预期分类，覆盖 IPC 错误收敛的全部入口。
    // 测试内允许 unwrap_err：构造样本错误，转换失败即测试失败，是期望行为。
    #[allow(clippy::unwrap_used, clippy::expect_used)]
    #[test]
    fn from_conversions_map_to_expected_categories() {
        let io: AppError = std::io::Error::other("boom").into();
        assert!(matches!(io, AppError::Io(_)));

        let json: AppError = serde_json::from_str::<i32>("[").unwrap_err().into();
        assert!(matches!(json, AppError::Internal(_)));

        let db: AppError = sqlx::Error::RowNotFound.into();
        assert!(matches!(db, AppError::Database(_)));

        let migrate: AppError = sqlx::migrate::MigrateError::VersionMissing(2).into();
        assert!(matches!(migrate, AppError::Database(_)));

        let img: AppError = image::load_from_memory(&[0, 1, 2, 3]).unwrap_err().into();
        assert!(matches!(img, AppError::Io(_)));

        let key: AppError = keyring::Error::NoEntry.into();
        assert!(matches!(key, AppError::Keyring(_)));
    }
}
