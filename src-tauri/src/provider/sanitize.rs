//! 生成图输出处理：按开关剥离 AI 元数据与 C2PA 内容凭据，保留原图格式。
//!
//! 集成自 `remove-ai-watermarks` 的元数据/C2PA 清除能力（不含 SynthID 与可见水印去除）。
//! 生成页默认「清除 AI 元数据 + 去除 C2PA」；此时 provider 走统一重编码 JPEG（本身已抹除
//! 全部附属段），本模块用于**用户显式保留原格式（任一开关关闭）**时的容器级定向剥离：
//!
//! - JPEG：丢弃 EXIF/XMP（APP1）、Photoshop/IPTC（APP13）、注释（COM）以清元数据；
//!   丢弃 JUMBF（APP11）以去 C2PA。保留 JFIF(APP0)/ICC(APP2)/Adobe(APP14) 等结构/色彩段。
//! - PNG：丢弃 tEXt/zTXt/iTXt/eXIf 以清元数据；丢弃 `caBX` 以去 C2PA。保留其余关键/辅助块。
//!
//! 无法识别的容器返回 `None`，交由调用方退化为重编码 JPEG。

/// 按开关做容器级剥离，保留原格式。返回 `(处理后字节, 扩展名)`。
/// 仅识别 JPEG / PNG；其它格式返回 `None`（调用方应退化为重编码 JPEG）。
pub fn strip_preserve(
    bytes: &[u8],
    clear_meta: bool,
    remove_c2pa: bool,
) -> Option<(Vec<u8>, &'static str)> {
    if is_jpeg(bytes) {
        Some((strip_jpeg(bytes, clear_meta, remove_c2pa), "jpg"))
    } else if is_png(bytes) {
        Some((strip_png(bytes, clear_meta, remove_c2pa), "png"))
    } else {
        None
    }
}

fn is_jpeg(b: &[u8]) -> bool {
    b.len() >= 2 && b[0] == 0xFF && b[1] == 0xD8
}

fn is_png(b: &[u8]) -> bool {
    b.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A])
}

/// JPEG 段级剥离。识别失败（截断/非法长度）时原样返回，绝不产出损坏图。
fn strip_jpeg(bytes: &[u8], clear_meta: bool, remove_c2pa: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    // SOI
    out.extend_from_slice(&[0xFF, 0xD8]);
    let mut i = 2usize;
    while i + 1 < bytes.len() {
        if bytes[i] != 0xFF {
            // 非标记位置（异常）：从此处起原样拷贝剩余，保底不破坏。
            out.extend_from_slice(&bytes[i..]);
            return out;
        }
        let marker = bytes[i + 1];
        // 填充 0xFF / 单独标记（RSTn、SOI、EOI、TEM）无长度字段。
        if marker == 0xFF {
            out.push(0xFF);
            i += 1;
            continue;
        }
        if marker == 0xD9 {
            // EOI
            out.extend_from_slice(&[0xFF, 0xD9]);
            i += 2;
            // EOI 之后通常无数据；若有尾随字节原样保留。
            if i < bytes.len() {
                out.extend_from_slice(&bytes[i..]);
            }
            return out;
        }
        if marker == 0xDA {
            // SOS：其后为熵编码数据直到 EOI，长度不可从段头得知——整段剩余原样拷贝。
            out.extend_from_slice(&bytes[i..]);
            return out;
        }
        // 带长度的段：长度含这 2 个长度字节自身。
        if i + 3 >= bytes.len() {
            out.extend_from_slice(&bytes[i..]);
            return out;
        }
        let len = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
        let seg_end = i + 2 + len;
        if len < 2 || seg_end > bytes.len() {
            // 长度非法/越界：原样拷贝剩余保底。
            out.extend_from_slice(&bytes[i..]);
            return out;
        }
        let payload = &bytes[i + 4..seg_end];
        if should_drop_jpeg_segment(marker, payload, clear_meta, remove_c2pa) {
            // 跳过该段（不写入 out）。
        } else {
            out.extend_from_slice(&bytes[i..seg_end]);
        }
        i = seg_end;
    }
    out
}

/// 判定某 JPEG APPn/COM 段是否应被丢弃。
fn should_drop_jpeg_segment(marker: u8, payload: &[u8], clear_meta: bool, remove_c2pa: bool) -> bool {
    match marker {
        // APP1：EXIF 或 XMP。
        0xE1 if clear_meta => {
            payload.starts_with(b"Exif\0\0")
                || payload.starts_with(b"http://ns.adobe.com/xap/1.0/\0")
                || payload.starts_with(b"http://ns.adobe.com/xmp/extension/\0")
        }
        // APP13：Photoshop IRB / IPTC。
        0xED if clear_meta => payload.starts_with(b"Photoshop 3.0\0"),
        // COM：注释（常含生成参数）。
        0xFE if clear_meta => true,
        // APP11：JUMBF（C2PA 内容凭据）。
        0xEB if remove_c2pa => true,
        _ => false,
    }
}

/// PNG 块级剥离。识别失败时原样返回。
fn strip_png(bytes: &[u8], clear_meta: bool, remove_c2pa: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    out.extend_from_slice(&bytes[0..8]); // 签名
    let mut i = 8usize;
    while i + 8 <= bytes.len() {
        let len = u32::from_be_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]) as usize;
        let ctype = &bytes[i + 4..i + 8];
        let chunk_end = i + 12 + len; // 4 长度 + 4 类型 + data + 4 CRC
        if chunk_end > bytes.len() {
            // 越界：原样拷贝剩余保底。
            out.extend_from_slice(&bytes[i..]);
            return out;
        }
        let is_iend = ctype == b"IEND";
        if !should_drop_png_chunk(ctype, clear_meta, remove_c2pa) {
            out.extend_from_slice(&bytes[i..chunk_end]);
        }
        i = chunk_end;
        if is_iend {
            break; // IEND 后无更多块。
        }
    }
    out
}

/// 判定某 PNG 块是否应被丢弃。
fn should_drop_png_chunk(ctype: &[u8], clear_meta: bool, remove_c2pa: bool) -> bool {
    match ctype {
        b"tEXt" | b"zTXt" | b"iTXt" | b"eXIf" => clear_meta,
        b"caBX" => remove_c2pa, // C2PA
        _ => false,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即失败
mod tests {
    use super::*;
    use image::codecs::jpeg::JpegEncoder;
    use image::{ExtendedColorType, ImageEncoder, RgbImage};
    use std::io::Cursor;

    /// 编码一张 2×2 纯色 JPEG（无附属段）。
    fn tiny_jpeg() -> Vec<u8> {
        let img = RgbImage::from_pixel(2, 2, image::Rgb([10, 120, 200]));
        let mut out = Cursor::new(Vec::new());
        JpegEncoder::new_with_quality(&mut out, 90)
            .encode(&img, 2, 2, ExtendedColorType::Rgb8)
            .unwrap();
        out.into_inner()
    }

    /// 编码一张 2×2 纯色 PNG。
    fn tiny_png() -> Vec<u8> {
        let img = RgbImage::from_pixel(2, 2, image::Rgb([10, 120, 200]));
        let mut out = Cursor::new(Vec::new());
        image::codecs::png::PngEncoder::new(&mut out)
            .write_image(&img, 2, 2, ExtendedColorType::Rgb8)
            .unwrap();
        out.into_inner()
    }

    /// 在 SOI 后插入一个 APPn 段（marker=0xEn/0xFE），payload 前置 id。
    fn inject_jpeg_seg(jpeg: &[u8], marker: u8, id: &[u8]) -> Vec<u8> {
        let mut v = vec![0xFF, 0xD8];
        let payload_len = id.len() + 2; // 含长度字段自身
        v.push(0xFF);
        v.push(marker);
        v.extend_from_slice(&(payload_len as u16).to_be_bytes());
        v.extend_from_slice(id);
        v.extend_from_slice(&jpeg[2..]); // 原图 SOI 之后
        v
    }

    /// 在 IHDR 后插入一个自定义块。
    fn inject_png_chunk(png: &[u8], ctype: &[u8], data: &[u8]) -> Vec<u8> {
        // 找到第一个块（IHDR）末尾：签名 8 + 长度 4 + 类型 4 + 13 data + CRC 4 = 33。
        let ihdr_end = 8 + 12 + 13;
        let mut v = Vec::new();
        v.extend_from_slice(&png[..ihdr_end]);
        v.extend_from_slice(&(data.len() as u32).to_be_bytes());
        v.extend_from_slice(ctype);
        v.extend_from_slice(data);
        v.extend_from_slice(&[0, 0, 0, 0]); // 假 CRC（剥离逻辑不校验 CRC）
        v.extend_from_slice(&png[ihdr_end..]);
        v
    }

    fn jpeg_has_marker(bytes: &[u8], marker: u8) -> bool {
        bytes.windows(2).any(|w| w[0] == 0xFF && w[1] == marker)
    }

    fn png_has_chunk(bytes: &[u8], ctype: &[u8]) -> bool {
        bytes.windows(4).any(|w| w == ctype)
    }

    #[test]
    fn jpeg_strips_exif_and_c2pa_but_keeps_decodable() {
        let base = tiny_jpeg();
        let with_exif = inject_jpeg_seg(&base, 0xE1, b"Exif\0\0");
        let with_both = inject_jpeg_seg(&with_exif, 0xEB, b"JP"); // APP11 JUMBF
        assert!(jpeg_has_marker(&with_both, 0xE1));
        assert!(jpeg_has_marker(&with_both, 0xEB));

        let (out, ext) = strip_preserve(&with_both, true, true).unwrap();
        assert_eq!(ext, "jpg");
        assert!(!jpeg_has_marker(&out, 0xEB), "APP11(C2PA) 应被剥离");
        // EXIF 段（APP1 首个）应被剥离——校验无 "Exif" 标识残留。
        assert!(
            !out.windows(4).any(|w| w == b"Exif"),
            "EXIF 标识应被剥离"
        );
        assert!(image::load_from_memory(&out).is_ok(), "剥离后仍可解码");
    }

    #[test]
    fn jpeg_keep_c2pa_when_flag_off() {
        let base = tiny_jpeg();
        let with_c2pa = inject_jpeg_seg(&base, 0xEB, b"JP");
        // 仅清元数据、不去 C2PA：APP11 应保留。
        let (out, _) = strip_preserve(&with_c2pa, true, false).unwrap();
        assert!(jpeg_has_marker(&out, 0xEB), "关闭去 C2PA 时 APP11 应保留");
        assert!(image::load_from_memory(&out).is_ok());
    }

    #[test]
    fn png_strips_text_and_c2pa_but_keeps_decodable() {
        let base = tiny_png();
        let with_text = inject_png_chunk(&base, b"tEXt", b"parameters\0seed:42");
        let with_both = inject_png_chunk(&with_text, b"caBX", b"c2pa-manifest");
        assert!(png_has_chunk(&with_both, b"tEXt"));
        assert!(png_has_chunk(&with_both, b"caBX"));

        let (out, ext) = strip_preserve(&with_both, true, true).unwrap();
        assert_eq!(ext, "png");
        assert!(!png_has_chunk(&out, b"tEXt"), "tEXt 应被剥离");
        assert!(!png_has_chunk(&out, b"caBX"), "caBX(C2PA) 应被剥离");
        assert!(image::load_from_memory(&out).is_ok(), "剥离后仍可解码");
    }

    #[test]
    fn png_keep_text_when_flag_off() {
        let base = tiny_png();
        let with_text = inject_png_chunk(&base, b"tEXt", b"keepme\0yes");
        // 不清元数据：tEXt 应保留。
        let (out, _) = strip_preserve(&with_text, false, true).unwrap();
        assert!(png_has_chunk(&out, b"tEXt"), "关闭清元数据时 tEXt 应保留");
    }

    #[test]
    fn unknown_format_returns_none() {
        assert!(strip_preserve(b"not-an-image", true, true).is_none());
    }
}
