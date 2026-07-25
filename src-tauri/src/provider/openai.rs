//! OpenAI 兼容生图 Provider（技术文档 4.4）。
//!
//! `POST {base_url}/images/edits`（multipart：参考图 + prompt + model）。
//! base_url 约定已含 /v1（R6）。兼容 `b64_json` 与 `url` 返回；结果统一转 JPG q95。

use std::io::Cursor;
use std::time::Duration;

use base64::Engine as _;
use futures_util::StreamExt;
use image::codecs::jpeg::JpegEncoder;
use serde::Deserialize;

use super::{
    DownloadProgress, GenImage, GenRequest, ImageProvider, ProgressFn, ProviderError,
    ProviderErrorKind,
};

const JPEG_QUALITY: u8 = 95;

pub struct OpenAiCompatible {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    request_timeout: Duration,
}

#[derive(Deserialize)]
struct EditResponse {
    data: Vec<ImageDatum>,
}
#[derive(Deserialize)]
struct ImageDatum {
    b64_json: Option<String>,
    url: Option<String>,
}
#[derive(Deserialize)]
struct ApiErrorEnvelope {
    error: Option<ApiError>,
}
#[derive(Deserialize)]
struct ApiError {
    message: Option<String>,
    code: Option<String>,
    #[serde(rename = "type")]
    type_: Option<String>,
}

impl OpenAiCompatible {
    /// `connect_timeout` 连接超时（默认 10s），`request_timeout` 整请求超时（默认 180s）。
    /// 生产走 [`Self::with_client`]（工厂复用 Client）；本构造主要供独立测试使用。
    #[allow(dead_code)]
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        connect_timeout: Duration,
        request_timeout: Duration,
    ) -> Result<Self, ProviderError> {
        let client = reqwest::Client::builder()
            .connect_timeout(connect_timeout)
            .build()
            .map_err(|e| ProviderError::new(ProviderErrorKind::Network, None, e.to_string()))?;
        Ok(Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
            request_timeout,
            client,
        })
    }

    /// 用共享 reqwest::Client 构造（工厂按 Key 复用同一 Client）。
    pub fn with_client(
        client: reqwest::Client,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        request_timeout: Duration,
    ) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
            request_timeout,
            client,
        }
    }

    fn endpoint(&self) -> String {
        format!("{}/images/edits", self.base_url)
    }
}

/// 违规判定关键词（内容策略）。
fn looks_like_content_policy(code: Option<&str>, type_: Option<&str>, msg: &str) -> bool {
    let hay = format!(
        "{} {} {}",
        code.unwrap_or(""),
        type_.unwrap_or(""),
        msg.to_lowercase()
    )
    .to_lowercase();
    [
        "content_policy",
        "content policy",
        "safety",
        "violat",
        "moderation",
        "违规",
        "内容策略",
    ]
    .iter()
    .any(|k| hay.contains(k))
}

fn map_reqwest_err(e: reqwest::Error) -> ProviderError {
    let kind = if e.is_timeout() {
        ProviderErrorKind::Timeout
    } else if e.is_connect() {
        ProviderErrorKind::Network
    } else {
        ProviderErrorKind::Other
    };
    ProviderError::new(kind, e.status().map(|s| s.as_u16()), e.to_string())
}

/// 将任意图片字节重编码为 JPEG q95。
fn to_jpeg(bytes: &[u8]) -> Result<Vec<u8>, ProviderError> {
    let img = image::load_from_memory(bytes)
        .map_err(|e| ProviderError::new(ProviderErrorKind::BadResponse, None, e.to_string()))?;
    let rgb = img.to_rgb8();
    let mut out = Cursor::new(Vec::new());
    JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY)
        .encode(
            &rgb,
            rgb.width(),
            rgb.height(),
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|e| ProviderError::new(ProviderErrorKind::Other, None, e.to_string()))?;
    Ok(out.into_inner())
}

/// 重编码为 PNG（用户选了 PNG 而远端给的不是 PNG 时）。重编码本身也抹掉附属段。
fn to_png(bytes: &[u8]) -> Result<Vec<u8>, ProviderError> {
    let img = image::load_from_memory(bytes)
        .map_err(|e| ProviderError::new(ProviderErrorKind::BadResponse, None, e.to_string()))?;
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png)
        .map_err(|e| ProviderError::new(ProviderErrorKind::Other, None, e.to_string()))?;
    Ok(out.into_inner())
}

#[async_trait::async_trait]
impl ImageProvider for OpenAiCompatible {
    async fn generate(
        &self,
        req: GenRequest,
        progress: Option<ProgressFn>,
    ) -> Result<GenImage, ProviderError> {
        // 读参考图 → multipart
        let image_bytes = tokio::fs::read(&req.image_path).await.map_err(|e| {
            ProviderError::new(ProviderErrorKind::Other, None, format!("读参考图失败：{e}"))
        })?;
        let file_name = req
            .image_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("image.png")
            .to_string();

        // 输出处理开关须在下方消费 size/quality 前读取（避免 params 部分移动后再借用）。
        let clear_meta = req.params.clear_meta();
        let remove_c2pa = req.params.remove_c2pa();

        let part = reqwest::multipart::Part::bytes(image_bytes).file_name(file_name);
        let mut form = reqwest::multipart::Form::new()
            .part("image", part)
            .text("prompt", req.prompt)
            .text("model", req.model)
            // n 恒为 1：一个任务 = 一张图（抽卡 k 次在引擎侧展开成 k 个任务）。
            .text("n", "1");
        // E16 / D1：仅透传显式设置的参数；未设置不带该字段（跟随提示词/模型默认）。
        // 字段名与端点文档的参数表一一对应；比例首选 aspect_ratio（size 只有部分模型认）。
        let p = req.params;
        let want_format = p.output_format.clone();
        for (name, value) in [
            ("aspect_ratio", p.aspect_ratio),
            ("size", p.size),
            ("output_format", p.output_format),
        ] {
            if let Some(v) = value {
                form = form.text(name, v);
            }
        }

        let resp = self
            .client
            .post(self.endpoint())
            .bearer_auth(&self.api_key)
            .timeout(self.request_timeout)
            .multipart(form)
            .send()
            .await
            .map_err(map_reqwest_err)?;

        let status = resp.status();
        if !status.is_success() {
            let code = status.as_u16();
            let body = resp.text().await.unwrap_or_default();
            let parsed: Option<ApiErrorEnvelope> = serde_json::from_str(&body).ok();
            let (msg, ecode, etype) = parsed
                .and_then(|e| e.error)
                .map(|e| (e.message.unwrap_or_default(), e.code, e.type_))
                .unwrap_or_else(|| (body.clone(), None, None));

            let kind = match code {
                401 | 403 => ProviderErrorKind::Auth,
                429 => ProviderErrorKind::RateLimited,
                400..=499
                    if looks_like_content_policy(ecode.as_deref(), etype.as_deref(), &msg) =>
                {
                    ProviderErrorKind::ContentPolicy
                }
                _ => ProviderErrorKind::Other,
            };
            return Err(ProviderError::new(kind, Some(code), truncate(&msg, 300)));
        }

        // 解析成功响应
        let body = resp
            .text()
            .await
            .map_err(|e| ProviderError::new(ProviderErrorKind::BadResponse, None, e.to_string()))?;
        let parsed: EditResponse = serde_json::from_str(&body).map_err(|e| {
            ProviderError::new(
                ProviderErrorKind::BadResponse,
                None,
                format!("响应解析失败：{e}"),
            )
        })?;
        let datum = parsed.data.into_iter().next().ok_or_else(|| {
            ProviderError::new(ProviderErrorKind::BadResponse, None, "响应无图像")
        })?;

        let raw = if let Some(b64) = datum.b64_json {
            base64::engine::general_purpose::STANDARD
                .decode(b64.trim())
                .map_err(|e| {
                    ProviderError::new(
                        ProviderErrorKind::BadResponse,
                        None,
                        format!("base64 解码失败：{e}"),
                    )
                })?
        } else if let Some(url) = datum.url {
            self.download(&url, progress).await?
        } else {
            return Err(ProviderError::new(
                ProviderErrorKind::BadResponse,
                None,
                "既无 b64 也无 url",
            ));
        };

        deliver(&raw, want_format.as_deref(), clear_meta, remove_c2pa)
    }
}

/// 交付本地文件：**用户选的输出格式说了算**。
///
/// 默认（未选格式）沿用既有规则：「清元数据 + 去 C2PA」全开 → 统一重编码 JPEG
/// （本身抹除全部附属段）；任一开关关闭 → 保留原容器做定向剥离。但用户显式选了
/// PNG 时那条规则会把 PNG 悄悄变成 JPG——「选了 PNG 拿到 JPG」是纯粹的失信，
/// 故显式选择优先：PNG 走容器级剥离（抹 tEXt/zTXt/iTXt/eXIf 与 caBX），
/// 拿到的不是 PNG 就重编码成 PNG。
fn deliver(
    raw: &[u8],
    want_format: Option<&str>,
    clear_meta: bool,
    remove_c2pa: bool,
) -> Result<GenImage, ProviderError> {
    let jpeg = |raw: &[u8]| -> Result<GenImage, ProviderError> {
        Ok(GenImage {
            bytes: to_jpeg(raw)?,
            ext: "jpg".to_string(),
        })
    };
    let stripped = super::sanitize::strip_preserve(raw, clear_meta, remove_c2pa);
    match want_format {
        Some("png") => match stripped {
            Some((bytes, "png")) => Ok(GenImage {
                bytes,
                ext: "png".to_string(),
            }),
            // 远端没给 PNG（或容器不认识）：按用户所选重编码为 PNG。
            _ => Ok(GenImage {
                bytes: to_png(raw)?,
                ext: "png".to_string(),
            }),
        },
        // 选了 JPEG：全清时重编码（更彻底），否则原样保留其想留的元数据。
        Some("jpeg") if !(clear_meta && remove_c2pa) => match stripped {
            Some((bytes, "jpg")) => Ok(GenImage {
                bytes,
                ext: "jpg".to_string(),
            }),
            _ => jpeg(raw),
        },
        Some("jpeg") => jpeg(raw),
        // 未选格式：既有默认规则原样保留。
        _ if clear_meta && remove_c2pa => jpeg(raw),
        _ => match stripped {
            Some((bytes, ext)) => Ok(GenImage {
                bytes,
                ext: ext.to_string(),
            }),
            // 无法识别的容器：退化为重编码 JPEG（无法保留其元数据）。
            None => jpeg(raw),
        },
    }
}

impl OpenAiCompatible {
    async fn download(
        &self,
        url: &str,
        progress: Option<ProgressFn>,
    ) -> Result<Vec<u8>, ProviderError> {
        let resp = self
            .client
            .get(url)
            .timeout(self.request_timeout)
            .send()
            .await
            .map_err(map_reqwest_err)?;
        if !resp.status().is_success() {
            return Err(ProviderError::new(
                ProviderErrorKind::Other,
                Some(resp.status().as_u16()),
                "下载结果图失败",
            ));
        }
        let total = resp.content_length();
        let mut received: u64 = 0;
        let mut buf = Vec::new();
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(map_reqwest_err)?;
            received += chunk.len() as u64;
            buf.extend_from_slice(&chunk);
            if let Some(cb) = &progress {
                cb(DownloadProgress { received, total });
            }
        }
        Ok(buf)
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect::<String>() + "…"
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即失败
mod tests {
    use super::*;
    use std::io::Cursor as StdCursor;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn tiny_png() -> Vec<u8> {
        // 2x2 红色 PNG
        let mut buf = StdCursor::new(Vec::new());
        let img = image::RgbImage::from_pixel(2, 2, image::Rgb([255, 0, 0]));
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        buf.into_inner()
    }

    fn tiny_jpeg() -> Vec<u8> {
        let mut buf = StdCursor::new(Vec::new());
        let img = image::RgbImage::from_pixel(2, 2, image::Rgb([0, 255, 0]));
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut buf, image::ImageFormat::Jpeg)
            .unwrap();
        buf.into_inner()
    }

    async fn ref_file() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("ref.png");
        tokio::fs::write(&p, tiny_png()).await.unwrap();
        (dir, p)
    }

    fn provider(base: &str) -> OpenAiCompatible {
        OpenAiCompatible::new(
            base,
            "sk-test",
            Duration::from_secs(5),
            Duration::from_secs(5),
        )
        .unwrap()
    }

    fn req(p: std::path::PathBuf) -> GenRequest {
        GenRequest {
            prompt: "a".into(),
            image_path: p,
            model: "gpt-image-2".into(),
            params: crate::provider::GenParams::default(),
        }
    }

    #[tokio::test]
    async fn success_b64() {
        let server = MockServer::start().await;
        let b64 = base64::engine::general_purpose::STANDARD.encode(tiny_png());
        Mock::given(method("POST"))
            .and(path("/images/edits"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{ "b64_json": b64 }]
            })))
            .mount(&server)
            .await;
        let (_d, rp) = ref_file().await;
        let out = provider(&server.uri())
            .generate(req(rp), None)
            .await
            .unwrap();
        // 应为合法 JPEG
        assert!(image::load_from_memory(&out.bytes).is_ok());
    }

    // 挂一个恒定成功的 /images/edits mock，返回该 server。
    async fn ok_server() -> MockServer {
        let server = MockServer::start().await;
        let b64 = base64::engine::general_purpose::STANDARD.encode(tiny_png());
        Mock::given(method("POST"))
            .and(path("/images/edits"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{ "b64_json": b64 }]
            })))
            .mount(&server)
            .await;
        server
    }

    // E16 / D1：未设置生成参数时，请求体不得带任何可选字段。
    #[tokio::test]
    async fn params_omitted_when_unset() {
        let server = ok_server().await;
        let (_d, rp) = ref_file().await;
        provider(&server.uri())
            .generate(req(rp), None)
            .await
            .unwrap();
        let reqs = server.received_requests().await.unwrap();
        let body = String::from_utf8_lossy(&reqs[0].body);
        for f in ["aspect_ratio", "size", "output_format"] {
            assert!(
                !body.contains(&format!("name=\"{f}\"")),
                "未设置时请求体不应带 {f}"
            );
        }
        // 一个任务恒为一张图（抽卡 k 次 = k 个任务，不是 n=k）。
        assert!(body.contains("name=\"n\""), "应恒带 n=1");
    }

    // E16 / D1：显式设置的参数须逐个透传到请求体，字段名与端点文档一致。
    // 「我设了却没生效」这类怀疑只能靠这条断言消除。
    #[tokio::test]
    async fn params_passed_through_when_set() {
        let server = ok_server().await;
        let (_d, rp) = ref_file().await;
        let mut r = req(rp);
        r.params = crate::provider::GenParams {
            aspect_ratio: Some("9:16".into()),
            size: Some("1024x1024".into()),
            output_format: Some("png".into()),
            ..Default::default()
        };
        provider(&server.uri()).generate(r, None).await.unwrap();
        let reqs = server.received_requests().await.unwrap();
        let body = String::from_utf8_lossy(&reqs[0].body);
        for (f, v) in [
            ("aspect_ratio", "9:16"),
            ("size", "1024x1024"),
            ("output_format", "png"),
        ] {
            assert!(
                body.contains(&format!("name=\"{f}\"")),
                "设置后应带 {f} 字段"
            );
            assert!(body.contains(v), "应透传 {f} 的值 {v}");
        }
    }

    // 「选了 PNG 拿到 JPG」是纯粹的失信：默认那条「全清 → 统一重编码 JPEG」的规则
    // 必须让位于用户显式选择。远端给的不是 PNG 时（这里 mock 就返回 PNG 与 JPEG 各一次）
    // 也要交付 PNG。
    #[tokio::test]
    async fn explicit_png_is_delivered_as_png_even_with_full_sanitize() {
        for raw in [tiny_png(), tiny_jpeg()] {
            let server = MockServer::start().await;
            let b64 = base64::engine::general_purpose::STANDARD.encode(&raw);
            Mock::given(method("POST"))
                .and(path("/images/edits"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "data": [{ "b64_json": b64 }]
                })))
                .mount(&server)
                .await;
            let (_d, rp) = ref_file().await;
            let mut r = req(rp);
            r.params = crate::provider::GenParams {
                output_format: Some("png".into()),
                // 输出处理全开（默认值）——旧规则会在这里把结果重编码成 JPEG。
                ..Default::default()
            };
            let out = provider(&server.uri()).generate(r, None).await.unwrap();
            assert_eq!(out.ext, "png", "选了 PNG 就必须交付 PNG");
            assert_eq!(
                image::guess_format(&out.bytes).unwrap(),
                image::ImageFormat::Png,
                "扩展名与实际容器须一致"
            );
        }
    }

    // 未选格式 = 沿用既有默认（全清 → 干净 JPEG），这条是防回归。
    #[tokio::test]
    async fn unset_format_keeps_default_clean_jpeg() {
        let server = ok_server().await; // 远端返回 PNG
        let (_d, rp) = ref_file().await;
        let out = provider(&server.uri())
            .generate(req(rp), None)
            .await
            .unwrap();
        assert_eq!(out.ext, "jpg");
        assert_eq!(
            image::guess_format(&out.bytes).unwrap(),
            image::ImageFormat::Jpeg
        );
    }

    #[tokio::test]
    async fn success_url_with_progress() {
        let server = MockServer::start().await;
        let img_url = format!("{}/img.png", server.uri());
        Mock::given(method("POST"))
            .and(path("/images/edits"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{ "url": img_url }]
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/img.png"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(tiny_png()))
            .mount(&server)
            .await;
        let (_d, rp) = ref_file().await;
        let hit = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let hit2 = hit.clone();
        let cb: ProgressFn = std::sync::Arc::new(move |_p| {
            hit2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });
        let out = provider(&server.uri())
            .generate(req(rp), Some(cb))
            .await
            .unwrap();
        assert!(image::load_from_memory(&out.bytes).is_ok());
        assert!(
            hit.load(std::sync::atomic::Ordering::SeqCst) >= 1,
            "应有下载进度回调"
        );
    }

    #[tokio::test]
    async fn classifies_429_401_content_and_bad_json() {
        let (_d, rp) = ref_file().await;

        // 429
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(429).set_body_string("rate"))
            .mount(&s)
            .await;
        let e = provider(&s.uri())
            .generate(req(rp.clone()), None)
            .await
            .unwrap_err();
        assert_eq!(e.kind, ProviderErrorKind::RateLimited);

        // 401
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(401).set_body_string("no auth"))
            .mount(&s)
            .await;
        let e = provider(&s.uri())
            .generate(req(rp.clone()), None)
            .await
            .unwrap_err();
        assert_eq!(e.kind, ProviderErrorKind::Auth);

        // 400 content policy
        let s = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(400).set_body_json(
            serde_json::json!({"error": {"message":"blocked", "code":"content_policy_violation"}})))
            .mount(&s).await;
        let e = provider(&s.uri())
            .generate(req(rp.clone()), None)
            .await
            .unwrap_err();
        assert_eq!(e.kind, ProviderErrorKind::ContentPolicy);

        // 200 bad JSON
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
            .mount(&s)
            .await;
        let e = provider(&s.uri())
            .generate(req(rp), None)
            .await
            .unwrap_err();
        assert_eq!(e.kind, ProviderErrorKind::BadResponse);
    }

    #[tokio::test]
    async fn classifies_timeout() {
        let (_d, rp) = ref_file().await;
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_millis(400)))
            .mount(&s)
            .await;
        // 极短请求超时
        let p = OpenAiCompatible::new(
            s.uri(),
            "sk",
            Duration::from_millis(50),
            Duration::from_millis(80),
        )
        .unwrap();
        let e = p.generate(req(rp), None).await.unwrap_err();
        assert_eq!(e.kind, ProviderErrorKind::Timeout);
    }
}
