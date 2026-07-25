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

/// `aspect_ratio` 受控取值（端点文档：gpt-image-2 系列经此参数控制输出比例，
/// **仅保证比例**，实际像素由上游决定）。同 `purpose.rs` / `publish/platform.rs` 的
/// 「受控取值单点」模式：命令边界据此校验，UI 只是它的镜子。
pub const ASPECT_RATIOS: &[&str] = &["1:1", "16:9", "9:16", "4:3", "3:4", "3:2", "2:3", "21:9"];

/// 该端点对显式 `size` 的硬性要求：边长须为 16 的倍数
/// （踩过的坑：`1080x1920` 看着是标准 9:16，1080 却不是 16 的倍数 → 400）。
const SIZE_EDGE_MULTIPLE: u32 = 16;

/// 输出格式受控取值。端点文档里还有 webp，但落盘/缩略图/发布链一路只按 PNG|JPG 走，
/// 故只开这两个——多开一个取值就要多守一条链路。
pub const OUTPUT_FORMATS: &[&str] = &["png", "jpeg"];

/// 生成参数（E16 / 决策 D1）：默认全部空 = 不向 API 传该字段，以提示词与模型默认为准；
/// 显式设置则透传（软件设置优先于提示词内同类描述，由 API 侧覆盖）。
///
/// **只保留真的会用到的三项**（比例/尺寸/输出格式）。端点文档里还有
/// quality / response_format / background / output_compression / extra_fields 等，
/// 一律不做——参数摆在界面上却没人用，只会让「到底哪个在起作用」更难回答。
/// `n` 恒为 1 由请求本身给定：抽卡 k 次在引擎侧展开成 k 个任务（各自独立重试与验收），
/// 而不是发 `n=k`——那样一次响应里 k 张图只有一张能落进当前任务。
///
/// **比例走 `aspect_ratio` 而不是 `size`**：gpt-image-2 系列就是这么控制画幅的，
/// 而提示词里写「9:16」对模型不构成约束——不显式给参数，多数情况回来的是 1:1。
///
/// 批次的 `params_json` 是「本批配置快照」，比本结构宽：还带 `draws`、`watermark`
/// （本地去水印档位）等纯 UI 键供「按此配置再来一批」还原。此处 **不** 加
/// `deny_unknown_fields`，那些键在这里被静默忽略——它们本就不该出现在发给远端的请求里。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct GenParams {
    /// 宽高比，如 "9:16"（受控取值见 [`ASPECT_RATIOS`]）。**控制画幅的首选参数**。
    pub aspect_ratio: Option<String>,
    /// 精确尺寸，如 "1024x1024" / "auto"。仅部分模型认；边长须为 16 的倍数。
    pub size: Option<String>,
    /// 输出格式："png" / "jpeg"（受控取值见 [`OUTPUT_FORMATS`]）。
    /// 它同时决定**本地交付格式**：选了 PNG 就必须拿到 .png，见 `openai::deliver`。
    pub output_format: Option<String>,
    /// 输出处理（任务1）：清除 AI 元数据（EXIF/XMP/PNG 文本/IPTC）。缺省视为开启。
    pub clear_ai_metadata: Option<bool>,
    /// 输出处理（任务1）：去除 C2PA 内容凭据（JUMBF/`caBX`）。缺省视为开启。
    pub remove_c2pa: Option<bool>,
}

impl GenParams {
    /// 从批次 `params_json` 解析；非法/空对象都退化为「全部空」。
    /// 调度器侧（已落库的批次）用它：宁可少带字段也不能让批次跑不起来。
    pub fn from_json(s: &str) -> Self {
        serde_json::from_str(s).unwrap_or_default()
    }

    /// 入口侧（创建批次）用的严格解析：键的**类型**不对就报错，而不是静默退化成
    /// 「全部空」——那正是「我明明选了 9:16，请求里却一个字段都没有」的成因。
    pub fn parse_checked(s: &str) -> Result<Self, String> {
        let v: serde_json::Value =
            serde_json::from_str(s).map_err(|e| format!("生成参数快照不是合法 JSON：{e}"))?;
        if !v.is_object() {
            return Err("生成参数快照必须是 JSON 对象".to_string());
        }
        let p: Self =
            serde_json::from_value(v).map_err(|e| format!("生成参数快照字段类型不对：{e}"))?;
        p.validate()?;
        Ok(p)
    }

    /// 花钱之前的本地预检：比例/输出格式的受控取值 + 尺寸边长。
    /// （端点的拒绝发生在计费之后，批量 20 条会连报 20 次同一个错。）
    pub fn validate(&self) -> Result<(), String> {
        if let Some(ar) = &self.aspect_ratio {
            if !ASPECT_RATIOS.contains(&ar.as_str()) {
                return Err(format!(
                    "不支持的宽高比「{ar}」，可用：{}",
                    ASPECT_RATIOS.join(" / ")
                ));
            }
        }
        if let Some(size) = &self.size {
            validate_size(size)?;
        }
        if let Some(f) = &self.output_format {
            if !OUTPUT_FORMATS.contains(&f.as_str()) {
                return Err(format!(
                    "不支持的输出格式「{f}」，可用：{}",
                    OUTPUT_FORMATS.join(" / ")
                ));
            }
        }
        Ok(())
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

/// 校验显式尺寸：`auto` 放行；否则须为 `宽x高`，且两边均为 16 的倍数。
fn validate_size(size: &str) -> Result<(), String> {
    let s = size.trim();
    if s.eq_ignore_ascii_case("auto") {
        return Ok(());
    }
    let (w, h) = s
        .split_once(['x', 'X', '*', '×'])
        .and_then(|(a, b)| Some((a.trim().parse::<u32>().ok()?, b.trim().parse::<u32>().ok()?)))
        .filter(|(w, h)| *w > 0 && *h > 0)
        .ok_or_else(|| format!("尺寸「{size}」格式不对，应形如 1024x1024 或 auto"))?;
    if w % SIZE_EDGE_MULTIPLE != 0 || h % SIZE_EDGE_MULTIPLE != 0 {
        let rw = round_to_multiple(w);
        let rh = round_to_multiple(h);
        return Err(format!(
            "尺寸「{size}」边长须为 {SIZE_EDGE_MULTIPLE} 的倍数（端点限制），\
             可改为 {rw}x{rh}，或改用「比例」参数（9:16 等）由上游定像素"
        ));
    }
    Ok(())
}

/// 取最近的 16 倍数（至少一个整倍）。
fn round_to_multiple(v: u32) -> u32 {
    let m = SIZE_EDGE_MULTIPLE;
    ((v + m / 2) / m).max(1) * m
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
    // 明明选了 9:16，请求里却一个字段都没有。
    #[test]
    fn ui_only_keys_are_ignored_without_dropping_wire_params() {
        let p = GenParams::from_json(
            r#"{"aspectRatio":"9:16","outputFormat":"png","draws":3,"watermark":"none",
                "clearAiMetadata":false,"removeC2pa":true}"#,
        );
        assert_eq!(p.aspect_ratio.as_deref(), Some("9:16"));
        assert_eq!(p.output_format.as_deref(), Some("png"));
        assert!(!p.clear_meta());
        assert!(p.remove_c2pa());
    }

    // 快照里的 `watermark` 是**本地**去水印档位（字符串），不是远端参数。它与
    // GenParams 的字段不同名，故不会把整份参数撞成「全部空」。
    #[test]
    fn local_dewatermark_tier_is_not_a_wire_param() {
        let p = GenParams::parse_checked(r#"{"watermark":"none","aspectRatio":"9:16"}"#).unwrap();
        assert_eq!(p.aspect_ratio.as_deref(), Some("9:16"));
    }

    // 未设置 = 键不出现 → 不带该字段（D1）。缺省的输出处理视为开启。
    #[test]
    fn absent_keys_mean_follow_prompt_defaults() {
        let p = GenParams::from_json("{}");
        assert!(p.aspect_ratio.is_none() && p.size.is_none() && p.output_format.is_none());
        assert!(p.clear_meta() && p.remove_c2pa());
    }

    // 只保留会用到的三项；文档里其余参数**不做**，出现在快照里也只当作未知键忽略。
    #[test]
    fn unused_documented_params_stay_out_of_the_wire_struct() {
        let p = GenParams::parse_checked(
            r#"{"aspectRatio":"9:16","quality":"standard","responseFormat":"b64_json",
                "background":"transparent","outputCompression":90,"extraFields":{"seed":1234}}"#,
        )
        .unwrap();
        assert_eq!(p.aspect_ratio.as_deref(), Some("9:16"));
        let json = serde_json::to_string(&p).unwrap();
        for k in [
            "quality",
            "responseFormat",
            "background",
            "outputCompression",
        ] {
            assert!(!json.contains(k), "{k} 不该进 GenParams：{json}");
        }
    }

    #[test]
    fn rejects_unsupported_output_format() {
        assert!(GenParams::parse_checked(r#"{"outputFormat":"png"}"#).is_ok());
        assert!(GenParams::parse_checked(r#"{"outputFormat":"jpeg"}"#).is_ok());
        // jpg / webp 不是端点的取值，早报比让远端 400 强。
        assert!(GenParams::parse_checked(r#"{"outputFormat":"jpg"}"#).is_err());
        assert!(GenParams::parse_checked(r#"{"outputFormat":"webp"}"#).is_err());
    }

    // 用户实际踩的坑：1080 不是 16 的倍数 → 端点 400。预检须在花钱之前拦下，
    // 且把可用值直接给出来（1080 → 1088；9:16 的正解是改用比例参数）。
    #[test]
    fn rejects_size_with_non_multiple_of_16_edges() {
        let e = GenParams::parse_checked(r#"{"size":"1080x1920"}"#).unwrap_err();
        assert!(e.contains("16 的倍数"), "{e}");
        assert!(e.contains("1088x1920"), "应给出可用尺寸：{e}");
        // 合法尺寸与 auto 放行。
        assert!(GenParams::parse_checked(r#"{"size":"1152x2048"}"#).is_ok());
        assert!(GenParams::parse_checked(r#"{"size":"auto"}"#).is_ok());
        assert!(GenParams::parse_checked(r#"{"size":"竖屏"}"#).is_err());
    }

    #[test]
    fn rejects_unsupported_aspect_ratio_and_bad_shapes() {
        assert!(GenParams::parse_checked(r#"{"aspectRatio":"9x16"}"#).is_err());
        assert!(GenParams::parse_checked(r#"{"aspectRatio":"21:9"}"#).is_ok());
        assert!(GenParams::parse_checked("[]").is_err());
    }

    // 严格解析的意义：键的类型不对要**报错**，不能像 from_json 那样静默退化成
    // 「全部空」——后者的表现是「设置了却一个字段都没发出去」，最难查。
    #[test]
    fn parse_checked_errors_where_from_json_would_silently_empty() {
        let bad = r#"{"aspectRatio":"9:16","size":123}"#;
        assert!(GenParams::parse_checked(bad).is_err());
        assert!(GenParams::from_json(bad).aspect_ratio.is_none());
    }
}
