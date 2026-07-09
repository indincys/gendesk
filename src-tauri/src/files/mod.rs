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
    pub fn trash(&self) -> PathBuf {
        self.root.join("trash")
    }
    pub fn logs(&self) -> PathBuf {
        self.root.join("logs")
    }

    /// 初始化全部子目录（幂等）。
    pub fn init(&self) -> AppResult<()> {
        for d in [
            self.refs(),
            self.thumbs(),
            self.outputs(),
            self.trash(),
            self.logs(),
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

/// 输出文件名：`参考图名_YYMMDD_编号无连字符.JPG`（技术文档 14.3）。
/// 例：`productA_260708_DZ0001.JPG`。参考图名做文件系统安全清洗（保留中文）。
pub fn output_filename(ref_name: &str, code: &str, date_yymmdd: &str) -> String {
    let safe = sanitize_filename(ref_name);
    let code_nohyphen = code.replace('-', "");
    format!("{safe}_{date_yymmdd}_{code_nohyphen}.JPG")
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
            output_filename("productA", "DZ-0001", "260708"),
            "productA_260708_DZ0001.JPG"
        );
        assert_eq!(
            output_filename("商品主图", "DZ-0128", "260101"),
            "商品主图_260101_DZ0128.JPG"
        );
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
