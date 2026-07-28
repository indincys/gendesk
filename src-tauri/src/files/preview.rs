//! 工单确认卡的参考图缩略图缓存。
//!
//! 确认卡要回答的是「**哪个提示词组配了哪几张参考图**」——配错的代价是整批图跑出来
//! 全错，而那要到验收时才看得出来，那时钱已经花完了。所以缩略图必须有。
//!
//! 但它不该是「打开弹窗就把 120 张 26 MP 相机图全解一遍」。这里做两件事：
//!
//! - **缓存**：键含内容标识，命中就直接返回文件路径，一次解码都不做。
//!   重开弹窗、来回滚动都是零成本。
//! - **落在 app data 下**：于是前端可以走 asset 协议（`assetSrc`）直接读，
//!   不必把几 MB base64 塞进一次 IPC 回包。
//!
//! 缓存是纯派生物，任何时候删掉都安全。

use std::path::{Path, PathBuf};

use image::codecs::jpeg::JpegEncoder;

use crate::error::AppResult;
use crate::files::decode::{self, DecodePermit};

/// 预览缩略图长边像素。够看清「是不是这张图」即可。
pub const PREVIEW_EDGE: u32 = 240;
/// 预览缩略图 JPEG 质量。
const PREVIEW_QUALITY: u8 = 72;

/// 缓存文件名：由**路径 + 纳秒 mtime + 字节数**算出。
///
/// 用纳秒而不是秒：同一秒内被改写成同样大小的文件确实存在（skill 重跑一遍工单就会），
/// 秒级粒度会让人看到上一版的图而毫无提示。
fn cache_key(src: &Path) -> AppResult<String> {
    use std::hash::{Hash, Hasher};
    let meta = std::fs::metadata(src)?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut h = std::collections::hash_map::DefaultHasher::new();
    // 规范化路径失败（比如刚被删）就退回原路径：键只需要稳定，不需要漂亮。
    let path = std::fs::canonicalize(src).unwrap_or_else(|_| src.to_path_buf());
    path.to_string_lossy().hash(&mut h);
    mtime.hash(&mut h);
    meta.len().hash(&mut h);
    Ok(format!("{:016x}.jpg", h.finish()))
}

/// 这张源图对应的缓存路径，以及它是否已经在了。
pub fn cached_path(previews_dir: &Path, src: &Path) -> AppResult<(PathBuf, bool)> {
    let p = previews_dir.join(cache_key(src)?);
    let hit = p.is_file();
    Ok((p, hit))
}

/// 生成一张预览缩略图。调用方须先确认没有命中缓存。
///
/// **先写临时文件再 rename**：直接往目标路径写，中途崩溃会留下一个 0 字节 JPEG，
/// 而它此后**永远命中缓存**——用户看到的就是一个永久的空白格子，且删不掉也说不清。
/// rename 在同目录内是原子的，要么没有、要么完整。
pub fn build(src: &Path, dest: &Path, permit: &DecodePermit) -> AppResult<()> {
    let bytes = std::fs::read(src)?;
    let img = decode::decode_scaled(&bytes, PREVIEW_EDGE, permit)?;
    let rgb = img.thumbnail(PREVIEW_EDGE, PREVIEW_EDGE).to_rgb8();

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = dest.with_extension("jpg.tmp");
    {
        let file = std::fs::File::create(&tmp)?;
        JpegEncoder::new_with_quality(std::io::BufWriter::new(file), PREVIEW_QUALITY).encode(
            &rgb,
            rgb.width(),
            rgb.height(),
            image::ExtendedColorType::Rgb8,
        )?;
    }
    match std::fs::rename(&tmp, dest) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e.into())
        }
    }
}

/// 清掉过期缓存。工单被移进 `_已收录/` 之后，它那些条目就再也不会被命中了。
///
/// 失败只当没清理过：这是缓存，清不掉的最坏后果是多占几 MB 磁盘。
pub fn gc(previews_dir: &Path, max_age: std::time::Duration) {
    let Ok(entries) = std::fs::read_dir(previews_dir) else {
        return;
    };
    let now = std::time::SystemTime::now();
    for e in entries.flatten() {
        let stale = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|age| age > max_age);
        if stale {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即失败
mod tests {
    use super::*;

    fn write_jpeg(path: &Path, w: u32, h: u32) {
        let mut img = image::RgbImage::new(w, h);
        for (x, y, px) in img.enumerate_pixels_mut() {
            *px = image::Rgb([(x % 256) as u8, (y % 256) as u8, 60]);
        }
        img.save(path).unwrap();
    }

    #[test]
    fn key_is_stable_but_changes_with_content() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("a.jpg");
        write_jpeg(&p, 800, 600);

        let k1 = cache_key(&p).unwrap();
        assert_eq!(k1, cache_key(&p).unwrap(), "没动过的文件键必须稳定");

        // 改内容（尺寸变 → 字节数变）。
        write_jpeg(&p, 900, 600);
        assert_ne!(k1, cache_key(&p).unwrap(), "内容变了键必须变");
    }

    #[test]
    fn key_changes_when_only_mtime_moves() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("a.bin");
        std::fs::write(&p, b"same-size-payload").unwrap();
        let k1 = cache_key(&p).unwrap();

        // 同样长度的另一份内容，重写一遍 —— 字节数一样，只有 mtime 会动。
        std::thread::sleep(std::time::Duration::from_millis(5));
        std::fs::write(&p, b"SAME-SIZE-PAYLOAD").unwrap();
        assert_ne!(k1, cache_key(&p).unwrap(), "改写过就该换键");
    }

    #[test]
    fn build_writes_thumbnail_and_reports_hit_next_time() {
        let tmp = tempfile::tempdir().unwrap();
        let previews = tmp.path().join("previews");
        std::fs::create_dir_all(&previews).unwrap();
        let src = tmp.path().join("big.jpg");
        write_jpeg(&src, 2400, 1800);

        let (dest, hit) = cached_path(&previews, &src).unwrap();
        assert!(!hit, "首次不该命中");
        build(&src, &dest, &DecodePermit::for_test()).unwrap();

        let (dest2, hit2) = cached_path(&previews, &src).unwrap();
        assert_eq!(dest, dest2);
        assert!(hit2, "生成之后必须命中");

        let img = image::open(&dest).unwrap();
        assert_eq!(img.width(), PREVIEW_EDGE, "长边缩到 240");
        assert_eq!(img.height(), 180);
    }

    /// 临时文件不该留下来，更不该被当成缓存命中。
    #[test]
    fn build_leaves_no_tmp_file() {
        let tmp = tempfile::tempdir().unwrap();
        let previews = tmp.path().join("previews");
        let src = tmp.path().join("s.jpg");
        write_jpeg(&src, 600, 400);
        let (dest, _) = cached_path(&previews, &src).unwrap();
        build(&src, &dest, &DecodePermit::for_test()).unwrap();

        let leftovers: Vec<_> = std::fs::read_dir(&previews)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "残留临时文件：{leftovers:?}");
    }

    #[test]
    fn build_on_unreadable_source_errors_without_creating_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let previews = tmp.path().join("previews");
        std::fs::create_dir_all(&previews).unwrap();
        let src = tmp.path().join("not-an-image.jpg");
        std::fs::write(&src, b"definitely not a jpeg").unwrap();

        let (dest, _) = cached_path(&previews, &src).unwrap();
        assert!(build(&src, &dest, &DecodePermit::for_test()).is_err());
        assert!(!dest.exists(), "失败不该留下缓存文件");
    }

    #[test]
    fn gc_removes_only_stale_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let previews = tmp.path().join("previews");
        std::fs::create_dir_all(&previews).unwrap();
        let fresh = previews.join("fresh.jpg");
        std::fs::write(&fresh, b"x").unwrap();

        // 上限设成 0 之外的一个大值：刚写的文件不该被清。
        gc(&previews, std::time::Duration::from_secs(3600));
        assert!(fresh.exists(), "新文件不该被清掉");

        // 上限 0 → 任何文件都算过期。
        gc(&previews, std::time::Duration::ZERO);
        assert!(!fresh.exists(), "过期文件应被清掉");
    }
}
