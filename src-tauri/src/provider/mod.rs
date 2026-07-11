//! 生图 Provider 抽象（技术文档 4.4 / 执行计划 2.2）。
//!
//! V1 唯一实现 [`openai::OpenAiCompatible`]（图生图 `POST {base_url}/images/edits`）。
//! V2 接入其它兼容模型 = 新增 trait 实现，不动引擎。

pub mod openai;

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// 生成参数（E16 / 决策 D1）：默认全部空 = 不向 API 传该字段，以提示词与模型默认为准；
/// 显式设置则透传（软件设置优先于提示词内同类描述，由 API 侧覆盖）。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct GenParams {
    /// 图像尺寸，如 "1024x1024" / "1536x1024" / "1024x1536" / "auto"。
    pub size: Option<String>,
    /// 质量，如 "low" / "medium" / "high" / "auto"。
    pub quality: Option<String>,
}

impl GenParams {
    /// 从批次 `params_json` 解析；非法/空对象都退化为「全部空」。
    pub fn from_json(s: &str) -> Self {
        serde_json::from_str(s).unwrap_or_default()
    }
}

/// 生成请求。
#[derive(Debug, Clone)]
pub struct GenRequest {
    pub prompt: String,
    pub image_path: PathBuf,
    pub model: String,
    pub params: GenParams,
}

/// 生成结果：已统一转为 JPEG（q95）字节，供 worker 落盘。
#[derive(Debug)]
pub struct GenImage {
    pub jpeg: Vec<u8>,
}

/// Provider 错误种类（供引擎错误分类器映射到六类）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderErrorKind {
    /// 连接/读取超时
    Timeout,
    /// HTTP 429
    RateLimited,
    /// 4xx + 内容违规
    ContentPolicy,
    /// 401 / 403
    Auth,
    /// 响应体无法解析
    BadResponse,
    /// 网络层错误
    Network,
    /// 其它（含 5xx）
    Other,
}

#[derive(Debug, thiserror::Error)]
#[error("provider[{kind:?}] http={http_status:?}: {message}")]
pub struct ProviderError {
    pub kind: ProviderErrorKind,
    pub http_status: Option<u16>,
    pub message: String,
}

impl ProviderError {
    pub fn new(
        kind: ProviderErrorKind,
        http_status: Option<u16>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            http_status,
            message: message.into(),
        }
    }
}

/// 下载阶段字节进度。
#[derive(Debug, Clone, Copy)]
pub struct DownloadProgress {
    pub received: u64,
    pub total: Option<u64>,
}

/// 下载进度回调。
pub type ProgressFn = Arc<dyn Fn(DownloadProgress) + Send + Sync>;

#[async_trait]
pub trait ImageProvider: Send + Sync {
    /// 生成一张图。`progress` 在 url 下载阶段被调用（b64 返回无字节进度）。
    async fn generate(
        &self,
        req: GenRequest,
        progress: Option<ProgressFn>,
    ) -> Result<GenImage, ProviderError>;
}
