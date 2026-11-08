//! Poster 的小型 JSON/CLI 接口。参数直接传进程，不经 shell，也不拉起 GUI。
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::error::{AppError, AppResult};

pub const SCHEMA: &str = "poster.images/1";

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PosterProduct {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub is_generic: bool,
}

#[derive(Debug, Deserialize)]
pub struct Catalog {
    pub schema: String,
    pub products: Vec<PosterProduct>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageItem {
    pub external_id: String,
    pub product_id: String,
    pub path: String,
    pub sha256: String,
    pub byte_size: u64,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub schema: String,
    pub source_id: String,
    pub batch_id: String,
    pub items: Vec<ImageItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemResult {
    pub external_id: String,
    pub product_id: String,
    pub image_id: String,
    pub status: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Receipt {
    pub schema: String,
    pub source_id: String,
    pub batch_id: String,
    pub applied: bool,
    pub items: Vec<ItemResult>,
}

pub fn discover_executable() -> PathBuf {
    let home = super::home();
    let local = home.join("Applications/Poster.app/Contents/MacOS/Poster");
    if local.is_file() {
        return local;
    }
    PathBuf::from("/Applications/Poster.app/Contents/MacOS/Poster")
}

async fn run(executable: &Path, args: &[&str]) -> AppResult<Vec<u8>> {
    if !executable.is_absolute() || !executable.is_file() {
        return Err(AppError::InvalidInput(
            "未找到支持图片接口的 Poster，请在设置 › Poster 图片接入中选择新版 Poster 应用".into(),
        ));
    }
    let mut cmd = tokio::process::Command::new(executable);
    cmd.args(args).kill_on_drop(true);
    // 测试隔离通过子进程继承 POSTER_TEST_ROOT；生产环境不修改对方数据位置。
    let result = tokio::time::timeout(Duration::from_secs(120), cmd.output())
        .await
        .map_err(|_| AppError::Timeout("Poster 接收超时，保留本批记录，可安全重试".into()))??;
    if !result.status.success() {
        let detail = String::from_utf8_lossy(&result.stderr);
        return Err(AppError::InvalidInput(format!(
            "Poster 接口未完成：{}",
            detail.chars().take(1200).collect::<String>().trim()
        )));
    }
    Ok(result.stdout)
}

pub async fn products(executable: &Path) -> AppResult<Vec<PosterProduct>> {
    let data = run(executable, &["export-image-products"]).await?;
    let catalog: Catalog = serde_json::from_slice(&data).map_err(|_| {
        AppError::InvalidInput("Poster 尚不支持图片接口，请安装包含图片接收接口的版本".into())
    })?;
    if catalog.schema != SCHEMA {
        return Err(AppError::InvalidInput("Poster 图片接口版本不兼容".into()));
    }
    let mut ids = std::collections::HashSet::new();
    if catalog
        .products
        .iter()
        .any(|p| p.id.is_empty() || !ids.insert(p.id.clone()))
    {
        return Err(AppError::InvalidInput(
            "Poster 商品目录存在空或重复 ID".into(),
        ));
    }
    Ok(catalog
        .products
        .into_iter()
        .filter(|p| !p.is_generic)
        .collect())
}

pub async fn preview_relocation(
    executable: &Path,
    manifest: &Path,
    request: &Request,
) -> AppResult<Receipt> {
    let spec: serde_json::Value =
        serde_json::from_slice(&run(executable, &["image-import-spec"]).await?)?;
    if !spec["capabilities"].as_array().is_some_and(|items| {
        items
            .iter()
            .any(|v| v.as_str() == Some("same-identity-path-relink"))
    }) {
        return Err(AppError::InvalidInput(
            "当前 Poster 尚不支持同图路径重绑，请先更新 Poster，原文件保持不动".into(),
        ));
    }
    let data = run(executable, &["import-images", &manifest.to_string_lossy()]).await?;
    let receipt: Receipt = serde_json::from_slice(&data)?;
    let mut seen = std::collections::HashSet::new();
    if receipt.applied
        || receipt.schema != request.schema
        || receipt.source_id != request.source_id
        || receipt.batch_id != request.batch_id
        || receipt.items.len() != request.items.len()
        || receipt.items.iter().any(|r| {
            !seen.insert(&r.external_id)
                || r.image_id.is_empty()
                || !matches!(r.status.as_str(), "ready" | "duplicate" | "deleted")
                || !request
                    .items
                    .iter()
                    .any(|i| i.external_id == r.external_id && i.product_id == r.product_id)
        })
    {
        return Err(AppError::InvalidInput(
            "Poster 路径整理预检不匹配，未新增或移动任何原图".into(),
        ));
    }
    Ok(receipt)
}

pub async fn import(executable: &Path, manifest: &Path, request: &Request) -> AppResult<Receipt> {
    let path = manifest.to_string_lossy();
    let data = run(executable, &["import-images", &path, "--apply"]).await?;
    let receipt: Receipt = serde_json::from_slice(&data)
        .map_err(|_| AppError::InvalidInput("Poster 回执无法读取，本批可安全重试".into()))?;
    validate_receipt(request, &receipt)?;
    Ok(receipt)
}

fn validate_receipt(request: &Request, receipt: &Receipt) -> AppResult<()> {
    let mut seen = std::collections::HashSet::new();
    let valid = receipt.schema == SCHEMA
        && receipt.source_id == request.source_id
        && receipt.batch_id == request.batch_id
        && receipt.applied
        && receipt.items.len() == request.items.len()
        && receipt.items.iter().all(|result| {
            seen.insert(&result.external_id)
                && !result.image_id.is_empty()
                && matches!(result.status.as_str(), "imported" | "duplicate" | "deleted")
                && request.items.iter().any(|item| {
                    item.external_id == result.external_id && item.product_id == result.product_id
                })
        });
    if valid {
        Ok(())
    } else {
        Err(AppError::InvalidInput(
            "Poster 回执与本批图片不一致，未标记完成".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receipt_must_cover_exactly_the_confirmed_images_and_products() {
        let request = Request {
            schema: SCHEMA.into(),
            source_id: "source".into(),
            batch_id: "batch".into(),
            items: vec![ImageItem {
                external_id: "a".into(),
                product_id: "p".into(),
                path: "".into(),
                sha256: "".into(),
                byte_size: 1,
                source: "ai".into(),
            }],
        };
        let mut receipt = Receipt {
            schema: SCHEMA.into(),
            source_id: "source".into(),
            batch_id: "batch".into(),
            applied: true,
            items: vec![ItemResult {
                external_id: "a".into(),
                product_id: "p".into(),
                image_id: "id".into(),
                status: "imported".into(),
            }],
        };
        assert!(validate_receipt(&request, &receipt).is_ok());
        receipt.items[0].product_id = "another-star".into();
        assert!(validate_receipt(&request, &receipt).is_err());
        receipt.items.clear();
        assert!(validate_receipt(&request, &receipt).is_err());
    }
}
