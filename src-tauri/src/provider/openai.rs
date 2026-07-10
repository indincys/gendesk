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

        let part = reqwest::multipart::Part::bytes(image_bytes).file_name(file_name);
        let mut form = reqwest::multipart::Form::new()
            .part("image", part)
            .text("prompt", req.prompt)
            .text("model", req.model)
            .text("n", "1");
        // E16 / D1：仅透传显式设置的参数；未设置不带该字段（跟随提示词/模型默认）。
        if let Some(size) = req.params.size {
            form = form.text("size", size);
        }
        if let Some(quality) = req.params.quality {
            form = form.text("quality", quality);
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

        Ok(GenImage {
            jpeg: to_jpeg(&raw)?,
        })
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
        assert!(image::load_from_memory(&out.jpeg).is_ok());
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

    // E16 / D1：未设置生成参数时，请求体不得带 size / quality 字段。
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
        assert!(!body.contains("name=\"size\""), "未设置时请求体不应带 size");
        assert!(
            !body.contains("name=\"quality\""),
            "未设置时请求体不应带 quality"
        );
    }

    // E16 / D1：显式设置的参数须透传到请求体。
    #[tokio::test]
    async fn params_passed_through_when_set() {
        let server = ok_server().await;
        let (_d, rp) = ref_file().await;
        let mut r = req(rp);
        r.params = crate::provider::GenParams {
            size: Some("1024x1024".into()),
            quality: Some("high".into()),
        };
        provider(&server.uri()).generate(r, None).await.unwrap();
        let reqs = server.received_requests().await.unwrap();
        let body = String::from_utf8_lossy(&reqs[0].body);
        assert!(body.contains("name=\"size\""), "设置后应带 size 字段");
        assert!(body.contains("1024x1024"), "应透传 size 值");
        assert!(body.contains("name=\"quality\""), "设置后应带 quality 字段");
        assert!(body.contains("high"), "应透传 quality 值");
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
        assert!(image::load_from_memory(&out.jpeg).is_ok());
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
