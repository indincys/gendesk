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

/// 缩略图长边像素。
const THUMB_MAX: u32 = 512;
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
        ] {
            std::fs::create_dir_all(d)?;
        }
        Ok(())
    }
}

/// 生成缩略图：长边缩到 <=512，输出 JPEG q80。返回原图 (宽, 高)。
pub fn generate_thumbnail(src: &Path, dest: &Path) -> AppResult<(u32, u32)> {
    let img = ImageReader::open(src)?.with_guessed_format()?.decode()?;
    let (w, h) = (img.width(), img.height());

    // thumbnail 在保持比例下缩到给定框内；已小于框则不放大。
    let thumb = if w > THUMB_MAX || h > THUMB_MAX {
        img.thumbnail(THUMB_MAX, THUMB_MAX)
    } else {
        img
    };
    let rgb = thumb.to_rgb8();

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::File::create(dest)?;
    let mut encoder = JpegEncoder::new_with_quality(BufWriter::new(file), THUMB_QUALITY);
    encoder
        .encode(
            &rgb,
            rgb.width(),
            rgb.height(),
            image::ExtendedColorType::Rgb8,
        )
        .map_err(AppError::from)?;
    Ok((w, h))
}

/// 上传副本长边上限（E41）：超过则生成压缩副本用于上传，原图仅展示。
const UPLOAD_MAX_EDGE: u32 = 2048;
/// 上传副本触发阈值（E41）：原图字节数超过此值也压缩（即便分辨率不高）。
const UPLOAD_MAX_BYTES: u64 = 3 * 1024 * 1024;
/// 上传副本 JPEG 质量。
const UPLOAD_QUALITY: u8 = 85;

/// 文件内容 hash（E30b 去重）：SipHash64(全字节) + 字节数前缀，十六进制字符串。
/// 非加密哈希，仅用于「同一文件」判定：同内容必同值，异内容碰撞概率对个人库可忽略。
pub fn content_hash(src: &Path) -> AppResult<String> {
    use std::hash::{Hash, Hasher};
    let bytes = std::fs::read(src)?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    Ok(format!("{:x}-{:016x}", bytes.len(), h.finish()))
}

/// 为超限图片生成上传用压缩副本（E41）：长边缩到 <=2048，转 JPEG q85。
/// 返回 `Some((副本路径, 字节数))`；若原图未超限则返回 `None`（上传直接用原图）。
pub fn make_upload_copy(src: &Path, dest: &Path) -> AppResult<Option<u64>> {
    let orig_size = std::fs::metadata(src).map(|m| m.len()).unwrap_or(0);
    let img = ImageReader::open(src)?.with_guessed_format()?.decode()?;
    let (w, h) = (img.width(), img.height());
    let oversize = w > UPLOAD_MAX_EDGE || h > UPLOAD_MAX_EDGE || orig_size > UPLOAD_MAX_BYTES;
    if !oversize {
        return Ok(None);
    }

    let scaled = if w > UPLOAD_MAX_EDGE || h > UPLOAD_MAX_EDGE {
        img.thumbnail(UPLOAD_MAX_EDGE, UPLOAD_MAX_EDGE)
    } else {
        img
    };
    let rgb = scaled.to_rgb8();
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::File::create(dest)?;
    let mut encoder = JpegEncoder::new_with_quality(BufWriter::new(file), UPLOAD_QUALITY);
    encoder
        .encode(
            &rgb,
            rgb.width(),
            rgb.height(),
            image::ExtendedColorType::Rgb8,
        )
        .map_err(AppError::from)?;
    let size = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
    Ok(Some(size))
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
        let (w, h) = generate_thumbnail(&src, &dest).unwrap();
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

    // E41：超长边图片生成压缩副本（长边 <=2048）；小图返回 None。
    #[test]
    fn make_upload_copy_compresses_oversize_only() {
        let tmp = tempfile::tempdir().unwrap();
        // 3000x1000 超长边 → 应压缩。
        let big = tmp.path().join("big.png");
        image::RgbImage::new(3000, 1000).save(&big).unwrap();
        let up = tmp.path().join("big_up.jpg");
        let out = make_upload_copy(&big, &up).unwrap();
        assert!(out.is_some(), "超限图应生成副本");
        let copy = image::open(&up).unwrap();
        assert!(copy.width() <= UPLOAD_MAX_EDGE && copy.height() <= UPLOAD_MAX_EDGE);
        assert_eq!(copy.width(), 2048, "长边缩到 2048");

        // 500x500 小图 → 无需副本。
        let small = tmp.path().join("small.png");
        image::RgbImage::new(500, 500).save(&small).unwrap();
        let up2 = tmp.path().join("small_up.jpg");
        assert!(
            make_upload_copy(&small, &up2).unwrap().is_none(),
            "小图不压缩"
        );
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
