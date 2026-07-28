//! 解码预算与 JPEG 缩放解码。
//!
//! 存在的理由是一次实测事故：一份 120 张 26 MP 相机图（6240×4160）的工单，
//! 光是**预览**就把 `gendesk` 拉到 600% CPU / 608 MB RSS。两件事叠出来的——
//! 每张图为了做一张 240 px 缩略图而被全分辨率解码（单张 ~78 MB 位图），
//! 而这些解码任务没有任何数量上限、关掉弹窗也不会停。
//!
//! 这里给出两个机制：
//!
//! 1. **预算**：进程级信号量。`spawn_blocking` 不可取消，所以唯一的杠杆是
//!    「同时允许几个解码开始」。许可做成 [`DecodePermit`] 令牌并作为
//!    `generate_thumbnail` / `make_upload_copy` 的必填入参 —— 类型层面强制，
//!    而不是靠「记得先拿许可」这种约定。本仓库刚被漏掉装配点烧过一次
//!    （`MAX_CONCURRENCY` 分叉：设置页与 DB 都显示 50，只有真正跑的信号量是 10）。
//! 2. **缩放解码**：JPEG 走 DCT 缩放，按 1/2·1/4·1/8 直接解出小图。
//!
//! 信号量放进程级 `OnceLock` 而不是 `AppState`：`generate_thumbnail` 的调用方之一
//! 是 `engine::dispatcher`，那里拿不到 `AppState`，穿线要动引擎装配与全部测试夹具。
//! CPU/内存预算是机器属性、不是业务真相，故不违反「业务真相只在 Rust 命令层」那条。

use std::io::Cursor;
use std::sync::{Arc, OnceLock};

use image::{DynamicImage, GrayImage, ImageReader, RgbImage};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::error::{AppError, AppResult};

/// 同时在跑的解码数上限。
///
/// **约束是内存不是核**：回落路径（PNG，或我们不敢碰的 JPEG）仍会物化整张位图，
/// 26 MP 就是 78 MB。3 个许可 = 234 MB 瞬时上限；再往上就是事故截图里那个 608 MB。
/// 取一半核数还是为了把机器留给用户——这是个后台任务，不该让人觉得电脑卡了。
pub fn max_concurrent() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get() / 2)
        .unwrap_or(2)
        .clamp(2, 3)
}

fn semaphore() -> &'static Arc<Semaphore> {
    static SEM: OnceLock<Arc<Semaphore>> = OnceLock::new();
    SEM.get_or_init(|| Arc::new(Semaphore::new(max_concurrent())))
}

/// 「我已经过了解码预算」的凭据。
///
/// 不可 Clone、不可自行构造（测试除外）。持有它的那段代码就是正在占一个许可的那段，
/// 丢弃即归还。
pub struct DecodePermit(Option<OwnedSemaphorePermit>);

impl DecodePermit {
    /// 测试用：不占真实许可。测试是串行的小图，没有封顶的必要。
    #[cfg(test)]
    pub fn for_test() -> Self {
        Self(None)
    }
}

/// 取一个解码许可。信号量永不 close，故失败分支实际不可达，但也不该 unwrap。
pub async fn acquire() -> DecodePermit {
    match Arc::clone(semaphore()).acquire_owned().await {
        Ok(p) => DecodePermit(Some(p)),
        // 只有 close() 过才会走到这里。宁可放行一次也不要卡死整条链路。
        Err(_) => DecodePermit(None),
    }
}

/// 在解码预算内跑一段阻塞的图像工作。
///
/// **先在异步侧拿许可，再 spawn**：`tokio::sync::Semaphore` 没有阻塞式 acquire，
/// 在 `spawn_blocking` 内部 `try_acquire` 轮询就成了忙等——那是在用「修 CPU 占用」
/// 的名义制造 CPU 占用。
pub async fn bounded<F, T>(f: F) -> AppResult<T>
where
    F: FnOnce(&DecodePermit) -> AppResult<T> + Send + 'static,
    T: Send + 'static,
{
    let permit = acquire().await;
    tokio::task::spawn_blocking(move || f(&permit))
        .await
        .map_err(|e| AppError::Internal(format!("图像处理任务失败：{e}")))?
}

/// 解码到「长边至少还有 `max_edge`」的尺寸。
///
/// JPEG 且能确定安全时走 DCT 缩放（内存与耗时都掉一个量级），否则原样全解码。
/// 返回的图**通常大于** `max_edge`（DCT 只有 1/2·1/4·1/8 三档），调用方仍需
/// 自己 `thumbnail()` 到确切尺寸。
pub fn decode_scaled(
    bytes: &[u8],
    max_edge: u32,
    _permit: &DecodePermit,
) -> AppResult<DynamicImage> {
    if let Some(img) = try_decode_scaled_jpeg(bytes, max_edge) {
        return Ok(img);
    }
    decode_full(bytes)
}

/// 既有路径：交给 `image` 全分辨率解码。所有回落都落到这里。
fn decode_full(bytes: &[u8]) -> AppResult<DynamicImage> {
    Ok(ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()?
        .decode()?)
}

/// SOF 段里的四个数。自己扫是为了两件事，见 [`try_decode_scaled_jpeg`]。
struct JpegHeader {
    width: u32,
    height: u32,
    components: u8,
    precision: u8,
}

/// 走 jpeg-decoder 的缩放解码；任何一点不确定就返回 `None` 让调用方回落。
///
/// **不调 `decoder.info()`**，尽管它才是「官方」的元信息入口。两个原因：
///
/// 1. 它在 `scale()` **之后**返回的是缩放后尺寸。而 `generate_thumbnail` 的返回值
///    会写进 `ref_images.width/height`，取错顺序的后果是今后每张导入图都被静默记成
///    780×520 而不是 6240×4160，且不会有任何报错。
/// 2. 它对组件数 2 或 ≥5 的基线 JPEG 直接 `panic!()`，而这类文件能通过 SOF 校验
///    （parser 只挡了 0 和「渐进式且 >4」）。release 侧 `panic = "abort"`，
///    那就是整个应用因为一张畸形图而消失。
///
/// 自己扫 SOF 把这两个问题一次解决：尺寸取自原始头部，组件数先验后用。
fn try_decode_scaled_jpeg(bytes: &[u8], max_edge: u32) -> Option<DynamicImage> {
    let head = scan_jpeg_header(bytes)?;
    // 只吃能确定对应关系的两种：8 位三通道（→RGB24）与 8 位单通道（→L8）。
    // CMYK32 是 jpeg-decoder 原样吐出的，Adobe 那套反转 YCCK 约定猜错就是静默错色，
    // 只会在某一台扫描仪的输出上暴露 —— 不猜，回落。
    if head.precision != 8 || !matches!(head.components, 1 | 3) {
        return None;
    }
    // 本来就不比目标大：缩放解码没有意义，走既有路径少一条分支。
    if head.width <= max_edge && head.height <= max_edge {
        return None;
    }
    let edge = u16::try_from(max_edge).ok()?;

    let mut dec = jpeg_decoder::Decoder::new(Cursor::new(bytes));
    let (w, h) = dec.scale(edge, edge).ok()?;
    let pixels = dec.decode().ok()?;
    let (w, h) = (u32::from(w), u32::from(h));

    // `from_raw` 会校验缓冲区长度，长度对不上返回 None —— 又一道回落。
    match head.components {
        3 => RgbImage::from_raw(w, h, pixels).map(DynamicImage::ImageRgb8),
        1 => GrayImage::from_raw(w, h, pixels).map(DynamicImage::ImageLuma8),
        _ => None,
    }
}

/// 只走标记、不碰像素地扫出第一个 SOF 段。不是 JPEG 或结构不对就 `None`。
fn scan_jpeg_header(bytes: &[u8]) -> Option<JpegHeader> {
    // SOI。
    if bytes.len() < 4 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return None;
    }
    let mut i = 2usize;
    loop {
        // 每个段以 0xFF 开头，其间允许任意多个 0xFF 填充字节。
        if *bytes.get(i)? != 0xFF {
            return None;
        }
        while bytes.get(i) == Some(&0xFF) {
            i += 1;
        }
        let marker = *bytes.get(i)?;
        i += 1;
        match marker {
            // 无长度字段的独立标记：TEM 与 RST0..7。
            0x01 | 0xD0..=0xD7 => continue,
            // 扫到 EOI / 进了扫描数据还没见到 SOF：不是我们能处理的排布。
            0xD9 | 0xDA => return None,
            _ => {}
        }
        let len = usize::from(u16::from_be_bytes([*bytes.get(i)?, *bytes.get(i + 1)?]));
        if len < 2 {
            return None;
        }
        // SOF0..SOF15，挖掉夹在这段编号里的 DHT(C4) / JPG(C8) / DAC(CC)。
        if matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
            let payload = bytes.get(i + 2..i + len)?;
            return Some(JpegHeader {
                precision: *payload.first()?,
                height: u32::from(u16::from_be_bytes([*payload.get(1)?, *payload.get(2)?])),
                width: u32::from(u16::from_be_bytes([*payload.get(3)?, *payload.get(4)?])),
                components: *payload.get(5)?,
            });
        }
        // len 含它自己那两个字节，且已校验 ≥2，故 i 严格递增、循环必然终止。
        i += len;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即失败
mod tests {
    use super::*;
    use image::codecs::jpeg::JpegEncoder;
    use image::ExtendedColorType;

    /// 造一张有内容的 JPEG（纯色图会被 DCT 完美还原，掩盖色彩通道错位）。
    fn jpeg_bytes(w: u32, h: u32, gray: bool) -> Vec<u8> {
        let mut out = Vec::new();
        if gray {
            let mut img = GrayImage::new(w, h);
            for (x, y, px) in img.enumerate_pixels_mut() {
                *px = image::Luma([((x * 7 + y * 3) % 256) as u8]);
            }
            JpegEncoder::new_with_quality(&mut out, 92)
                .encode(&img, w, h, ExtendedColorType::L8)
                .unwrap();
        } else {
            let mut img = RgbImage::new(w, h);
            for (x, y, px) in img.enumerate_pixels_mut() {
                // 三个通道明显不同，通道错位会让平均差异爆掉。
                *px = image::Rgb([(x % 256) as u8, (y % 256) as u8, 40]);
            }
            JpegEncoder::new_with_quality(&mut out, 92)
                .encode(&img, w, h, ExtendedColorType::Rgb8)
                .unwrap();
        }
        out
    }

    #[test]
    fn scan_header_reads_original_dims() {
        let b = jpeg_bytes(2400, 1200, false);
        let h = scan_jpeg_header(&b).unwrap();
        assert_eq!((h.width, h.height), (2400, 1200));
        assert_eq!(h.components, 3);
        assert_eq!(h.precision, 8);
    }

    #[test]
    fn scan_header_rejects_non_jpeg() {
        assert!(scan_jpeg_header(b"\x89PNG\r\n\x1a\n").is_none());
        assert!(scan_jpeg_header(b"").is_none());
        // SOI 之后就截断：不能读越界，只能 None。
        assert!(scan_jpeg_header(b"\xFF\xD8\xFF\xC0").is_none());
    }

    #[test]
    fn scaled_decode_is_at_least_requested_and_not_wastefully_large() {
        let b = jpeg_bytes(2400, 1200, false);
        let img = decode_scaled(&b, 300, &DecodePermit::for_test()).unwrap();
        // DCT 只有 1/2·1/4·1/8：2400 → 300 正好是 1/8。
        assert!(img.width() >= 300, "长边不得小于请求值，否则缩略图会糊");
        assert!(img.width() < 600, "不该白解一档更大的（{}）", img.width());
    }

    #[test]
    fn small_jpeg_falls_back_to_full_decode() {
        let b = jpeg_bytes(200, 100, false);
        let img = decode_scaled(&b, 512, &DecodePermit::for_test()).unwrap();
        assert_eq!((img.width(), img.height()), (200, 100));
    }

    #[test]
    fn png_takes_fallback_path_at_full_size() {
        let mut buf = Vec::new();
        let img = RgbImage::new(1200, 800);
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png)
            .unwrap();
        assert!(
            try_decode_scaled_jpeg(&buf, 240).is_none(),
            "PNG 不该走 JPEG 路径"
        );
        let out = decode_scaled(&buf, 240, &DecodePermit::for_test()).unwrap();
        assert_eq!((out.width(), out.height()), (1200, 800));
    }

    /// 灰度是最容易写错的分支：通道数不对 `from_raw` 会返回 None 静默回落，
    /// 于是「能出图」并不说明走对了路。这里直接断言它确实走了缩放路径。
    #[test]
    fn grayscale_jpeg_decodes_scaled() {
        let b = jpeg_bytes(1600, 800, true);
        let img = try_decode_scaled_jpeg(&b, 200).expect("8 位灰度应走缩放解码");
        assert!(img.width() >= 200 && img.width() < 400);
    }

    /// 头完好但扫描数据被砍断：`scale()` 能过（它只读 SOF），`decode()` 必然失败，
    /// 于是回落到 image，image 同样失败 → Err。**关键是它不该 panic**，
    /// 更不该拿一张半截图当成功返回。
    #[test]
    fn truncated_jpeg_falls_back_and_errors() {
        let full = jpeg_bytes(2400, 1200, false);
        // 砍在 SOS（0xFFDA）刚开始的地方：SOF 还在，像素数据一个字节都没有。
        let sos = full
            .windows(2)
            .position(|w| w == [0xFF, 0xDA])
            .expect("测试前提：编码结果里应有 SOS 标记");
        let cut = &full[..sos + 12];
        assert!(
            scan_jpeg_header(cut).is_some(),
            "测试前提：截断后 SOF 仍应可读，否则测的就不是解码失败那条路"
        );
        assert!(decode_scaled(cut, 240, &DecodePermit::for_test()).is_err());
    }

    /// 防色彩空间回归：缩放解码 + 缩放，与旧的全解码 + 缩放，逐像素平均绝对差
    /// 必须很小。通道错位、YCbCr 系数搞反这类问题会让这个数字直接爆掉，
    /// 而肉眼看缩略图「好像也还行」。
    #[test]
    fn scaled_decode_matches_full_decode_visually() {
        let b = jpeg_bytes(1600, 1200, false);
        let scaled = decode_scaled(&b, 200, &DecodePermit::for_test())
            .unwrap()
            .thumbnail(200, 200)
            .to_rgb8();
        let full = decode_full(&b).unwrap().thumbnail(200, 200).to_rgb8();
        assert_eq!(scaled.dimensions(), full.dimensions());

        let total: u64 = scaled
            .pixels()
            .zip(full.pixels())
            .map(|(a, b)| {
                a.0.iter()
                    .zip(b.0.iter())
                    .map(|(x, y)| u64::from(x.abs_diff(*y)))
                    .sum::<u64>()
            })
            .sum();
        let mae = total as f64 / (scaled.pixels().len() * 3) as f64;
        assert!(
            mae < 8.0,
            "缩放解码与全解码的平均绝对差 {mae:.2} 过大，疑似色彩通道错位"
        );
    }

    #[test]
    fn budget_is_bounded_and_at_least_two() {
        let n = max_concurrent();
        assert!((2..=3).contains(&n), "许可数 {n} 越界");
    }

    #[tokio::test]
    async fn bounded_runs_and_releases_permit() {
        for _ in 0..(max_concurrent() * 3) {
            let v = bounded(|_p| Ok(7u8)).await.unwrap();
            assert_eq!(v, 7);
        }
    }
}
