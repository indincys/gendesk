use std::io::Write;
use std::path::Path;

use sha2::{Digest, Sha256};

use super::{client, Image};
use crate::error::{AppError, AppResult};

pub fn atomic_write(path: &Path, bytes: &[u8]) -> AppResult<()> {
    let parent = path
        .parent()
        .ok_or_else(|| AppError::InvalidInput("图片路径没有父目录".into()))?;
    std::fs::create_dir_all(parent)?;
    let temporary = path.with_extension("partial");
    let mut file = std::fs::File::create(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    std::fs::rename(temporary, path)?;
    Ok(())
}

/// 生成导入请求。成品与实况配对直接引用原位置；作品记录已不在的旧图才复制进每批固定的
/// 永久目录，重试复用原文件，任何内容冲突都不覆盖。
pub fn prepare(
    root: &str,
    batch: &str,
    source_id: String,
    images: &[Image],
) -> AppResult<client::Request> {
    let root = Path::new(root);
    if !root.is_absolute() || batch.len() != 32 || !batch.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(AppError::InvalidInput(
            "Poster 永久目录或批次编号不合法".into(),
        ));
    }
    let day = chrono::Local::now().format("%Y-%m-%d");
    // 日期只用于可读性，完整批次 ID 决定永久路径；中断后跨天重试仍找回同一目录。
    let existing = if root.is_dir() {
        std::fs::read_dir(root)?
            .filter_map(Result::ok)
            .find(|entry| entry.file_name().to_string_lossy().ends_with(batch))
            .map(|e| e.path())
    } else {
        None
    };
    let batch_root = existing.unwrap_or_else(|| root.join(format!("{day}_审稿图片_{batch}")));
    let mut items = Vec::new();
    for image in images {
        let product_id = image
            .product_id
            .as_ref()
            .filter(|id| !id.is_empty())
            .ok_or_else(|| AppError::InvalidInput("有图片尚未指定 Poster 商品".into()))?;
        let product_name = image.product_name.as_deref().unwrap_or(product_id);
        if let (Some(jpg), Some(mov)) = (&image.live_image, &image.live_video) {
            items.push(live_item(image, product_id, jpg, mov)?);
            continue;
        }
        let bytes = std::fs::read(&image.image_path)?;
        // 只交图片；图片扩展名与实际文件头都要符合。
        image::image_dimensions(&image.image_path)
            .map_err(|e| AppError::InvalidInput(format!("图片无法读取：{e}")))?;
        let ext = Path::new(&image.image_path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_lowercase();
        if !matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "webp") {
            return Err(AppError::InvalidInput("当前只支持图片导入 Poster".into()));
        }
        let product_folder = crate::files::sanitize_filename(product_name)
            .replace(['（', '）', '(', ')'], "")
            .replace(' ', "_");
        let name = crate::files::sanitize_filename(&image.item_name);
        // 新作品直接引用成品，不额外制造验收批次副本。旧待确认缓存仍按旧约定安全接完。
        let path = if image.canonical_path.as_deref() == Some(image.image_path.as_str()) {
            Path::new(&image.image_path).to_path_buf()
        } else {
            batch_root
                .join(product_folder)
                .join(format!("{name}-{}.{ext}", image.uid))
        };
        let hash = format!("{:x}", Sha256::digest(&bytes));
        if path.exists() {
            if Sha256::digest(std::fs::read(&path)?) != Sha256::digest(&bytes) {
                return Err(AppError::InvalidInput(format!(
                    "永久图片内容冲突，未覆盖：{}",
                    path.display()
                )));
            }
        } else {
            atomic_write(&path, &bytes)?;
        }
        items.push(client::ImageItem {
            external_id: image.uid.clone(),
            product_id: product_id.clone(),
            path: path.to_string_lossy().into(),
            sha256: hash,
            byte_size: bytes.len() as u64,
            source: "ai".into(),
            live_video: None,
        });
    }
    if items.is_empty() {
        return Err(AppError::InvalidInput("本批没有可导入的图片".into()));
    }
    Ok(client::Request {
        schema: client::SCHEMA.into(),
        source_id,
        batch_id: batch.into(),
        items,
    })
}

/// 实况配对：JPG 作为图片本体，MOV 作为配对视频，都引用 `成品/实况照片/` 里的原文件。
fn live_item(
    image: &Image,
    product_id: &str,
    jpg: &str,
    mov: &str,
) -> AppResult<client::ImageItem> {
    image::image_dimensions(jpg)
        .map_err(|e| AppError::InvalidInput(format!("实况静态图无法读取：{e}")))?;
    let still = std::fs::read(jpg)?;
    let video = std::fs::read(mov)?;
    Ok(client::ImageItem {
        external_id: image.uid.clone(),
        product_id: product_id.into(),
        path: jpg.into(),
        sha256: format!("{:x}", Sha256::digest(&still)),
        byte_size: still.len() as u64,
        source: "ai".into(),
        live_video: Some(client::LiveVideo {
            path: mov.into(),
            sha256: format!("{:x}", Sha256::digest(&video)),
            byte_size: video.len() as u64,
        }),
    })
}
