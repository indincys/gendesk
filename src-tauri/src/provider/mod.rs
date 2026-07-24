//! 生图 Provider 抽象（技术文档 4.4 / 执行计划 2.2）。
//!
//! V1 唯一实现 [`openai::OpenAiCompatible`]（图生图 `POST {base_url}/images/edits`）。
//! V2 接入其它兼容模型 = 新增 trait 实现，不动引擎。

pub mod openai;
pub mod sanitize;

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// 生成参数（E16 / 决策 D1）：默认全部空 = 不向 API 传该字段，以提示词与模型默认为准；
/// 显式设置则透传（软件设置优先于提示词内同类描述，由 API 侧覆盖）。
///
/// 批次的 `params_json` 是「本批配置快照」，比本结构宽：还带 `draws`、`watermark`
/// 等纯 UI 键供「按此配置再来一批」还原。此处 **不** 加 `deny_unknown_fields`，
/// 那些键在这里被静默忽略——它们本就不该出现在发给远端的请求里。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct GenParams {
    /// 图像尺寸，如 "1024x1024" / "1536x1024" / "1024x1536" / "auto"。
    pub size: Option<String>,
    /// 质量，如 "low" / "medium" / "high" / "auto"。
    pub quality: Option<String>,
    /// 输出处理（任务1）：清除 AI 元数据（EXIF/XMP/PNG 文本/IPTC）。缺省视为开启。
    pub clear_ai_metadata: Option<bool>,
    /// 输出处理（任务1）：去除 C2PA 内容凭据（JUMBF/`caBX`）。缺省视为开启。
    pub remove_c2pa: Option<bool>,
}

impl GenParams {
    /// 从批次 `params_json` 解析；非法/空对象都退化为「全部空」。
    pub fn from_json(s: &str) -> Self {
        serde_json::from_str(s).unwrap_or_default()
    }

    /// 是否清除 AI 元数据（缺省=开启，与生成页默认一致）。
    pub fn clear_meta(&self) -> bool {
        self.clear_ai_metadata.unwrap_or(true)
    }

    /// 是否去除 C2PA 内容凭据（缺省=开启）。
    pub fn remove_c2pa(&self) -> bool {
        self.remove_c2pa.unwrap_or(true)
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

/// 生成结果：处理后的图片字节 + 扩展名（不含点，如 "jpg"/"png"），供 worker 落盘。
/// 默认（清元数据+去 C2PA）走统一重编码 JPEG；用户保留原格式时携带真实扩展名。
#[derive(Debug)]
pub struct GenImage {
    pub bytes: Vec<u8>,
    pub ext: String,
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

#[cfg(test)]
#[allow(clippy::unwrap_used)] // 测试断言失败即失败
mod tests {
    use super::GenParams;

    // 批次快照里混着纯 UI 键（抽卡次数、去水印档位、输出处理开关）。它们**不得**
    // 影响发往远端的字段，也不得让整份快照解析失败退化成「全部空」——那会让用户
    // 明明选了 9:16，请求里却一个 size 都没有。
    #[test]
    fn ui_only_keys_are_ignored_without_dropping_wire_params() {
        let p = GenParams::from_json(
            r#"{"size":"1080x1920","quality":"high","draws":3,"watermark":"none",
                "clearAiMetadata":false,"removeC2pa":true}"#,
        );
        assert_eq!(p.size.as_deref(), Some("1080x1920"));
        assert_eq!(p.quality.as_deref(), Some("high"));
        assert!(!p.clear_meta());
        assert!(p.remove_c2pa());
    }

    // 未设置 = 键不出现 → 不带该字段（D1）。缺省的输出处理视为开启。
    #[test]
    fn absent_keys_mean_follow_prompt_defaults() {
        let p = GenParams::from_json("{}");
        assert!(p.size.is_none() && p.quality.is_none());
        assert!(p.clear_meta() && p.remove_c2pa());
    }
}
