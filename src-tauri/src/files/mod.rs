//! 文件系统模块（执行计划 1.5 / 技术文档 5.2）。
//!
//! app_data 目录树、缩略图生成（长边 512 JPEG q80）、输出命名器、废纸篓搬运/清理。

// 命名器/废纸篓搬运等先于 M2/M3 消费者落地；未使用项在对应里程碑接入后收紧。
#![allow(dead_code)]

use std::io::BufWriter;
use std::path::{Path, PathBuf};

use image::codecs::jpeg::JpegEncoder;
use image::ImageReader;

use crate::error::{AppError, AppResult};

pub mod decode;
pub mod preview;

use decode::DecodePermit;

/// 缩略图长边像素。
pub const THUMB_MAX: u32 = 512;
/// 缩略图 JPEG 质量。
const THUMB_QUALITY: u8 = 80;

/// 应用数据目录布局。
pub struct DataDirs {
    pub root: PathBuf,
}

impl DataDirs {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
    pub fn db(&self) -> PathBuf {
        self.root.join("gendesk.db")
    }
    pub fn refs(&self) -> PathBuf {
        self.root.join("refs")
    }
    pub fn thumbs(&self) -> PathBuf {
        self.root.join("thumbs")
    }
    /// 工单确认卡的参考图缩略图缓存。
    ///
    /// 落在 app data 下是为了走 asset 协议（`tauri.conf.json` 的 scope 是
    /// `$APPDATA/**`）——工单目录在交接根下、够不着，而为一张预览图去放宽整个应用的
    /// 文件读取范围代价与收益完全不成比例。缓存到这里就绕开了这个取舍。
    /// **纯缓存**：删掉任何时候都安全，下次看时重建。
    pub fn previews(&self) -> PathBuf {
        self.root.join("previews")
    }
    pub fn outputs(&self) -> PathBuf {
        self.root.join("outputs")
    }
    /// 生成结果暂存（未验收）。验收通过后再输出到 outputs/{批次}。
    pub fn results(&self) -> PathBuf {
        self.root.join("results")
    }
    pub fn trash(&self) -> PathBuf {
        self.root.join("trash")
    }
    pub fn logs(&self) -> PathBuf {
        self.root.join("logs")
    }
    /// 图生视频成片与封面落盘处（`clips/{clip_id}.mp4` + `.jpg`）。
    ///
    /// 与 outputs/ 分开：成片不是「验收通过的图片输出」，它有自己的验收与去向
    /// （入资产库做视频型素材包）。混在 outputs/{批次}/ 下会让批次文件夹的含义漂移。
    pub fn clips(&self) -> PathBuf {
        self.root.join("clips")
    }

    /// 初始化全部子目录（幂等）。
    pub fn init(&self) -> AppResult<()> {
        for d in [
            self.refs(),
            self.thumbs(),
            self.outputs(),
            self.results(),
            self.trash(),
            self.logs(),
            self.clips(),
            self.previews(),
        ] {
            std::fs::create_dir_all(d)?;
        }
        Ok(())
    }
}

/// 生成缩略图：长边缩到 <=512，输出 JPEG q80。返回**原图** (宽, 高)。
///
/// 返回值会写进 `ref_images.width/height`，所以它必须是原图尺寸而不是解码出来那张的
/// 尺寸 —— `decode_scaled` 交回来的图通常大于 512 但远小于原图。
pub fn generate_thumbnail(src: &Path, dest: &Path, permit: &DecodePermit) -> AppResult<(u32, u32)> {
    let bytes = std::fs::read(src)?;
    // 尺寸只读文件头，不解码。
    let (w, h) = ImageReader::new(std::io::Cursor::new(&bytes))
        .with_guessed_format()?
        .into_dimensions()?;
    let img = decode::decode_scaled(&bytes, THUMB_MAX, permit)?;
    write_thumbnail_from(&img, dest)?;
    Ok((w, h))
}

/// 上传副本长边上限（E41）：超过则生成压缩副本用于上传，原图仅展示。
pub const UPLOAD_MAX_EDGE: u32 = 2048;
/// 上传副本触发阈值（E41）：原图字节数超过此值也压缩（即便分辨率不高）。
pub const UPLOAD_MAX_BYTES: u64 = 3 * 1024 * 1024;
/// 上传副本 JPEG 质量。
const UPLOAD_QUALITY: u8 = 85;

/// E41 判据的**单点**：要不要为这张图另出一份上传副本。
///
/// 调用方拿它决定「解码要解到多大」，所以它必须在解码之前就能回答 ——
/// 三个入参全部来自文件头与字节数，不需要像素。
pub fn is_oversize(w: u32, h: u32, byte_len: u64) -> bool {
    w > UPLOAD_MAX_EDGE || h > UPLOAD_MAX_EDGE || byte_len > UPLOAD_MAX_BYTES
}

/// 已解码的图 → 缩到框内 → 写 JPEG。
///
/// 入参是**已经解码好的图**而不是路径：`ingest_one` 一次解码要同时喂缩略图和上传副本，
/// 走路径就意味着为第二个产物再解一遍（那正是这次要修的东西）。
fn write_jpeg_scaled(
    img: &image::DynamicImage,
    dest: &Path,
    max_edge: u32,
    quality: u8,
) -> AppResult<()> {
    let rgb = if img.width() > max_edge || img.height() > max_edge {
        img.thumbnail(max_edge, max_edge).to_rgb8()
    } else {
        img.to_rgb8()
    };
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::File::create(dest)?;
    JpegEncoder::new_with_quality(BufWriter::new(file), quality)
        .encode(
            &rgb,
            rgb.width(),
            rgb.height(),
            image::ExtendedColorType::Rgb8,
        )
        .map_err(AppError::from)
}

/// 已解码的图 → 缩略图（长边 ≤512，JPEG q80）。
pub fn write_thumbnail_from(img: &image::DynamicImage, dest: &Path) -> AppResult<()> {
    write_jpeg_scaled(img, dest, THUMB_MAX, THUMB_QUALITY)
}

/// 已解码的图 → 上传副本（长边 ≤2048，JPEG q85）。调用方须先过 [`is_oversize`]。
pub fn write_upload_copy_from(img: &image::DynamicImage, dest: &Path) -> AppResult<()> {
    write_jpeg_scaled(img, dest, UPLOAD_MAX_EDGE, UPLOAD_QUALITY)
}

/// 文件内容 hash（E30b 去重）：SipHash64(全字节) + 字节数前缀，十六进制字符串。
/// 非加密哈希，仅用于「同一文件」判定：同内容必同值，异内容碰撞概率对个人库可忽略。
pub fn content_hash(src: &Path) -> AppResult<String> {
    Ok(content_hash_bytes(&std::fs::read(src)?))
}

/// 同上，但吃已经在手里的字节。
///
/// `ingest_one` 为了拷贝本来就要把整个文件读进内存，再 `content_hash(&dest)` 就是
/// 第三遍读同一份 6 MB。**必须与 [`content_hash`] 逐字节同值**：库里既有的
/// `ref_images.content_hash` 全是老口径算出来的，一旦分叉，E30b 去重就会把所有存量
/// 图判成「新图」。`Vec<u8>` 的 Hash 实现本就转发给切片实现，故两者天然同值 ——
/// 但这是承重的，有测试钉死。
pub fn content_hash_bytes(bytes: &[u8]) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    format!("{:x}-{:016x}", bytes.len(), h.finish())
}

/// 输出文件名：`参考图名_YYMMDD_编号无连字符_抽卡序号.EXT`（需求 14.3 / E17 D2）。
/// 例：`productA_260708_DZ0001_2.JPG`。抽卡序号避免同组合多张通过时文件名冲突。
/// 参考图名做文件系统安全清洗（保留中文）。扩展名（大写）跟随源结果格式
/// （任务1：用户保留原格式时可能为 PNG）。
pub fn output_filename(
    ref_name: &str,
    code: &str,
    date_yymmdd: &str,
    draw_index: i64,
    ext: &str,
) -> String {
    let safe = sanitize_filename(ref_name);
    let code_nohyphen = code.replace('-', "");
    let ext = normalize_ext(ext);
    format!("{safe}_{date_yymmdd}_{code_nohyphen}_{draw_index}.{ext}")
}

/// 从结果文件路径取输出用扩展名（大写）；无扩展名退化为 `JPG`。
pub fn output_ext_from_path(path: &str) -> String {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("jpg");
    normalize_ext(ext)
}

/// 规范扩展名：去点、转大写、空则 JPG。
fn normalize_ext(ext: &str) -> String {
    let e = ext.trim().trim_start_matches('.').to_uppercase();
    if e.is_empty() {
        "JPG".to_string()
    } else {
        e
    }
}

/// Unix 秒 → `YYMMDD`（本地按 UTC 近似；输出命名用）。
/// 采用 Howard Hinnant 的 civil_from_days 算法，无需外部 crate。
pub fn date_yymmdd(unix_secs: i64) -> String {
    let days = unix_secs.div_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    format!("{:02}{:02}{:02}", year.rem_euclid(100), m, d)
}

/// 清洗文件名：替换非法字符与控制符为下划线；保留中文与常规字符。
pub fn sanitize_filename(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    // 去首尾空白与点（Windows 不允许尾点/尾空格）
    out = out.trim().trim_end_matches('.').to_string();
    if out.is_empty() {
        out.push('_');
    }
    out
}

/// 将文件搬入废纸篓目录，返回新路径（保留原文件名，冲突加数字后缀）。
pub fn move_to_trash(src: &Path, trash_dir: &Path) -> AppResult<PathBuf> {
    std::fs::create_dir_all(trash_dir)?;
    let file_name = src
        .file_name()
        .ok_or_else(|| AppError::Io("源文件无文件名".into()))?;
    let mut dest = trash_dir.join(file_name);
    let mut n = 1;
    while dest.exists() {
        let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
        let ext = src.extension().and_then(|s| s.to_str()).unwrap_or("");
        let candidate = if ext.is_empty() {
            format!("{stem}_{n}")
        } else {
            format!("{stem}_{n}.{ext}")
        };
        dest = trash_dir.join(candidate);
        n += 1;
    }
    std::fs::rename(src, &dest)?;
    Ok(dest)
}

/// 物理删除文件（废纸篓清理）。不存在视为成功（幂等）。
pub fn purge(path: &Path) -> AppResult<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即失败
mod tests {
    use super::*;

    #[test]
    fn output_filename_strips_hyphen_and_keeps_chinese() {
        assert_eq!(
            output_filename("productA", "DZ-0001", "260708", 1, "jpg"),
            "productA_260708_DZ0001_1.JPG"
        );
        assert_eq!(
            output_filename("商品主图", "DZ-0128", "260101", 2, "jpg"),
            "商品主图_260101_DZ0128_2.JPG"
        );
    }

    // 任务1：保留原格式时扩展名跟随源结果（大写）。
    #[test]
    fn output_filename_follows_source_ext() {
        assert_eq!(
            output_filename("p", "DZ-0001", "260708", 1, "png"),
            "p_260708_DZ0001_1.PNG"
        );
        assert_eq!(output_ext_from_path("/data/results/12.png"), "PNG");
        assert_eq!(output_ext_from_path("/data/results/12.jpg"), "JPG");
        assert_eq!(output_ext_from_path("/data/results/noext"), "JPG");
    }

    // E17 D2：同一组合不同抽卡序号的文件名不冲突。
    #[test]
    fn output_filename_draw_index_avoids_collision() {
        let a = output_filename("productA", "DZ-0001", "260708", 1, "jpg");
        let b = output_filename("productA", "DZ-0001", "260708", 2, "jpg");
        assert_ne!(a, b, "同组合两次抽卡的输出文件名必须不同");
    }

    #[test]
    fn sanitize_replaces_illegal_chars() {
        assert_eq!(sanitize_filename("a/b:c*d?"), "a_b_c_d_");
        assert_eq!(sanitize_filename("  正常名  "), "正常名");
        assert_eq!(sanitize_filename(""), "_");
    }

    #[test]
    fn dirs_init_creates_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = DataDirs::new(tmp.path());
        dirs.init().unwrap();
        for d in [
            dirs.refs(),
            dirs.thumbs(),
            dirs.outputs(),
            dirs.trash(),
            dirs.logs(),
            dirs.previews(),
        ] {
            assert!(d.is_dir(), "{d:?} 未创建");
        }
    }

    #[test]
    fn thumbnail_downscales_long_edge_and_reports_original_dims() {
        let tmp = tempfile::tempdir().unwrap();
        // 造一张 1000x600 的源图
        let src = tmp.path().join("src.png");
        let mut buf = image::RgbImage::new(1000, 600);
        for (x, _y, px) in buf.enumerate_pixels_mut() {
            *px = image::Rgb([(x % 256) as u8, 100, 150]);
        }
        buf.save(&src).unwrap();

        let dest = tmp.path().join("thumb.jpg");
        let (w, h) = generate_thumbnail(&src, &dest, &DecodePermit::for_test()).unwrap();
        assert_eq!((w, h), (1000, 600), "应返回原图尺寸");

        let thumb = image::open(&dest).unwrap();
        assert!(thumb.width() <= THUMB_MAX && thumb.height() <= THUMB_MAX);
        assert_eq!(thumb.width(), 512, "长边应缩到 512");
        assert_eq!(thumb.height(), 307, "按比例 600*512/1000≈307");
    }

    // E30b：同内容文件 hash 一致，异内容不一致。
    #[test]
    fn content_hash_matches_same_bytes_only() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a.bin");
        let b = tmp.path().join("b.bin");
        let c = tmp.path().join("c.bin");
        std::fs::write(&a, b"same-content").unwrap();
        std::fs::write(&b, b"same-content").unwrap();
        std::fs::write(&c, b"different").unwrap();
        let ha = content_hash(&a).unwrap();
        assert_eq!(ha, content_hash(&b).unwrap(), "同内容 hash 一致");
        assert_ne!(ha, content_hash(&c).unwrap(), "异内容 hash 不同");
    }

    // E41：超长边图片生成压缩副本（长边 <=2048）；小图不生成。
    // （原 `make_upload_copy(路径)` 已拆成 `is_oversize` 判据 + `write_upload_copy_from`
    //   写入，好让 `ingest_one` 一次解码同时喂缩略图和上传副本；断言口径不变。）
    #[test]
    fn upload_copy_compresses_oversize_only() {
        let tmp = tempfile::tempdir().unwrap();
        // 3000x1000 超长边 → 应压缩。
        let big = tmp.path().join("big.png");
        image::RgbImage::new(3000, 1000).save(&big).unwrap();
        let bytes = std::fs::read(&big).unwrap();
        assert!(
            is_oversize(3000, 1000, bytes.len() as u64),
            "超长边应判超限"
        );

        let up = tmp.path().join("big_up.jpg");
        let img =
            decode::decode_scaled(&bytes, UPLOAD_MAX_EDGE, &DecodePermit::for_test()).unwrap();
        write_upload_copy_from(&img, &up).unwrap();
        let copy = image::open(&up).unwrap();
        assert!(copy.width() <= UPLOAD_MAX_EDGE && copy.height() <= UPLOAD_MAX_EDGE);
        assert_eq!(copy.width(), 2048, "长边缩到 2048");

        // 500x500 小图 → 无需副本。
        let small = tmp.path().join("small.png");
        image::RgbImage::new(500, 500).save(&small).unwrap();
        let small_len = std::fs::metadata(&small).unwrap().len();
        assert!(!is_oversize(500, 500, small_len), "小图不压缩");
    }

    /// E41 的第二个触发条件：分辨率不高但字节数超限，也要出上传副本。
    /// 判据里少写一项 `byte_len` 就会把这个分支静默吃掉。
    #[test]
    fn upload_copy_triggers_on_bytes_even_when_small() {
        let tmp = tempfile::tempdir().unwrap();
        let noisy = tmp.path().join("noisy.png");
        // 1600x1600 随机噪声 PNG 压不动（原始 7.3 MB），稳定超过 UPLOAD_MAX_BYTES(3MB)，
        // 而长宽都在 2048 以内 —— 正好只命中「字节数」这一条触发。
        let mut img = image::RgbImage::new(1600, 1600);
        let mut seed = 0x9E37_79B9u32;
        for px in img.pixels_mut() {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *px = image::Rgb([(seed >> 16) as u8, (seed >> 8) as u8, seed as u8]);
        }
        img.save(&noisy).unwrap();
        let len = std::fs::metadata(&noisy).unwrap().len();
        assert!(len > UPLOAD_MAX_BYTES, "测试前提：这张图必须超过字节阈值");
        assert!(is_oversize(1600, 1600, len), "字节超限也应判超限");

        let up = tmp.path().join("noisy_up.jpg");
        let bytes = std::fs::read(&noisy).unwrap();
        let decoded =
            decode::decode_scaled(&bytes, UPLOAD_MAX_EDGE, &DecodePermit::for_test()).unwrap();
        write_upload_copy_from(&decoded, &up).unwrap();
        // 尺寸没超限，故不缩放，只是转成 JPEG。
        let copy = image::open(&up).unwrap();
        assert_eq!((copy.width(), copy.height()), (1600, 1600));
    }

    /// 承重：`content_hash_bytes` 与 `content_hash` 必须同值，否则 E30b 去重会把
    /// 库里全部存量图判成新图。
    #[test]
    fn content_hash_bytes_matches_file_variant() {
        let tmp = tempfile::tempdir().unwrap();
        for payload in [b"".as_slice(), b"x".as_slice(), b"same-content".as_slice()] {
            let p = tmp.path().join("f.bin");
            std::fs::write(&p, payload).unwrap();
            assert_eq!(
                content_hash(&p).unwrap(),
                content_hash_bytes(payload),
                "两种口径必须逐字节同值"
            );
        }
    }

    /// 回归陷阱：走 DCT 缩放解码后，返回的仍必须是**原图**尺寸。
    /// 取错就是今后每张导入图都被静默记成缩放后的尺寸，且不会报错。
    #[test]
    fn thumbnail_reports_original_dims_for_scaled_jpeg() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("big.jpg");
        let mut img = image::RgbImage::new(3200, 2400);
        for (x, y, px) in img.enumerate_pixels_mut() {
            *px = image::Rgb([(x % 256) as u8, (y % 256) as u8, 90]);
        }
        img.save(&src).unwrap();

        let dest = tmp.path().join("t.jpg");
        let (w, h) = generate_thumbnail(&src, &dest, &DecodePermit::for_test()).unwrap();
        assert_eq!((w, h), (3200, 2400), "必须是原图尺寸，不是缩放解码后的尺寸");

        let thumb = image::open(&dest).unwrap();
        assert_eq!(thumb.width(), THUMB_MAX, "长边仍应缩到 512");
        assert_eq!(thumb.height(), 384, "按比例 2400*512/3200");
    }

    #[test]
    fn move_to_trash_handles_name_collision() {
        let tmp = tempfile::tempdir().unwrap();
        let trash = tmp.path().join("trash");
        let a = tmp.path().join("a.jpg");
        std::fs::write(&a, b"1").unwrap();
        let p1 = move_to_trash(&a, &trash).unwrap();
        assert_eq!(p1.file_name().unwrap(), "a.jpg");
        // 同名再来一次
        std::fs::write(&a, b"2").unwrap();
        let p2 = move_to_trash(&a, &trash).unwrap();
        assert_eq!(p2.file_name().unwrap(), "a_1.jpg");
        assert!(p1.exists() && p2.exists());
    }
}
