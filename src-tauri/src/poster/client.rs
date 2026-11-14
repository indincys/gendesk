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

/// Poster 的图片类型（全软件一套，用户在 Poster 里自己定）。归属依据是 `id`。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PosterImageType {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    pub schema: String,
    pub products: Vec<PosterProduct>,
    /// 旧版 Poster 没有这一项（不支持按类型导入）时为 `None`，与「一个类型都没建」区分开。
    #[serde(default)]
    pub image_types: Option<Vec<PosterImageType>>,
}

/// 封面库（`purpose: cover`）。缺省的图片库不写出，旧请求逐字节不变。
pub const PURPOSE_COVER: &str = "cover";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageItem {
    pub external_id: String,
    pub product_id: String,
    pub path: String,
    pub sha256: String,
    pub byte_size: u64,
    pub source: String,
    /// 实况配对视频；有它时 `path` 是同名 JPG，Poster 按内容标识符核对配对。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_video: Option<LiveVideo>,
    /// `cover` 进封面库；缺省进图片库。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
    /// Poster 图片类型 ID；缺省 = 未分类。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveVideo {
    pub path: String,
    pub sha256: String,
    pub byte_size: u64,
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
    /// 新版 Poster 回报这张图入库后的类型（用户已分过类的图保持原类型）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_id: Option<String>,
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

/// 图文（`poster.image-sets/1`）：几张图按发布先后组成一条图文，带图一次导入。
pub const SETS_SCHEMA: &str = "poster.image-sets/1";

/// 图文里直接带的一张图：同 `import-images` 的一项，但不写商品和用途（商品取图文的，用途固定图片库）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetImage {
    pub external_id: String,
    pub path: String,
    pub sha256: String,
    pub byte_size: u64,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_video: Option<LiveVideo>,
}

impl SetImage {
    /// 图片库请求的一项转成图文里的一张。封面库的图不能组进图文。
    pub fn from_item(item: ImageItem) -> AppResult<Self> {
        if item.purpose.is_some() {
            return Err(AppError::InvalidInput("封面不能组进图文".into()));
        }
        Ok(Self {
            external_id: item.external_id,
            path: item.path,
            sha256: item.sha256,
            byte_size: item.byte_size,
            source: item.source,
            type_id: item.type_id,
            live_video: item.live_video,
        })
    }
}

/// 一条图文。标题、正文、话题都不带：文案在 Poster 里配，配不上就是「缺标题」。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetItem {
    pub external_id: String,
    pub product_id: String,
    pub images: Vec<SetImage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetsRequest {
    pub schema: String,
    pub source_id: String,
    pub batch_id: String,
    pub items: Vec<SetItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetResult {
    pub external_id: String,
    pub product_id: String,
    /// 图文素材 ID；预览时还没有。
    #[serde(default)]
    pub id: Option<String>,
    pub status: String,
    /// 这条图文用到的图片库图 ID，按发布先后。
    #[serde(default)]
    pub image_ids: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetsReceipt {
    pub schema: String,
    pub source_id: String,
    pub batch_id: String,
    pub applied: bool,
    pub items: Vec<SetResult>,
    #[serde(default)]
    pub paired: i64,
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
            "未找到支持图片接口的 Poster，请在设置 › 审稿后自动化中选择新版 Poster 应用".into(),
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
    Ok(catalog(executable).await?.products)
}

/// 商品目录（不含通用款）连同图片类型。
pub async fn catalog(executable: &Path) -> AppResult<Catalog> {
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
    Ok(Catalog {
        products: catalog
            .products
            .into_iter()
            .filter(|p| !p.is_generic)
            .collect(),
        ..catalog
    })
}

/// Poster `image-import-spec` 是否声明了某项能力。
async fn require_capability(executable: &Path, capability: &str, missing: &str) -> AppResult<()> {
    let spec: serde_json::Value =
        serde_json::from_slice(&run(executable, &["image-import-spec"]).await?)?;
    if spec["capabilities"]
        .as_array()
        .is_some_and(|items| items.iter().any(|v| v.as_str() == Some(capability)))
    {
        Ok(())
    } else {
        Err(AppError::InvalidInput(missing.into()))
    }
}

pub async fn preview_relocation(
    executable: &Path,
    manifest: &Path,
    request: &Request,
) -> AppResult<Receipt> {
    require_capability(
        executable,
        "same-identity-path-relink",
        "当前 Poster 尚不支持同图路径重绑，请先更新 Poster，原文件保持不动",
    )
    .await?;
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
    if request.items.iter().any(|item| item.live_video.is_some()) {
        require_capability(
            executable,
            "live-photo",
            "当前 Poster 尚不支持实况图导入，请先更新 Poster；本批保留，可安全重试",
        )
        .await?;
    }
    if request.items.iter().any(|item| item.type_id.is_some()) {
        require_capability(
            executable,
            "image-type",
            "当前 Poster 尚不支持按图片类型导入，请先更新 Poster；本批保留，可安全重试",
        )
        .await?;
    }
    if request
        .items
        .iter()
        .any(|item| item.purpose.as_deref() == Some(PURPOSE_COVER))
    {
        require_capability(
            executable,
            "cover",
            "当前 Poster 尚不支持导入封面，请先更新 Poster；本批保留，可安全重试",
        )
        .await?;
    }
    let path = manifest.to_string_lossy();
    let data = run(executable, &["import-images", &path, "--apply"]).await?;
    let receipt: Receipt = serde_json::from_slice(&data)
        .map_err(|_| AppError::InvalidInput("Poster 回执无法读取，本批可安全重试".into()))?;
    validate_receipt(request, &receipt)?;
    Ok(receipt)
}

/// Poster 的图文导入是否支持直接带图（`import-spec image-sets` 的字段表里有 `items[].images`）。
async fn require_sets_with_images(executable: &Path) -> AppResult<()> {
    const MISSING: &str = "当前 Poster 尚不支持带图导入图文，请先更新 Poster；本组保留，可安全重试";
    let data = run(executable, &["import-spec", "image-sets"])
        .await
        .map_err(|_| AppError::InvalidInput(MISSING.into()))?;
    let spec: serde_json::Value =
        serde_json::from_slice(&data).map_err(|_| AppError::InvalidInput(MISSING.into()))?;
    if spec["kind"]["schema"] == SETS_SCHEMA && spec["kind"]["fields"]["items[].images"].is_string()
    {
        Ok(())
    } else {
        Err(AppError::InvalidInput(MISSING.into()))
    }
}

/// 导入图文：先不带 `--apply` 预览（与正式导入同一套检查），通过了再落库。
/// 同一来源身份重投回执 `duplicate`，不会在 Poster 里多出一条。
pub async fn import_sets(
    executable: &Path,
    manifest: &Path,
    request: &SetsRequest,
) -> AppResult<SetsReceipt> {
    require_sets_with_images(executable).await?;
    let path = manifest.to_string_lossy();
    let preview = run(executable, &["import-image-sets", &path]).await?;
    let preview: SetsReceipt = serde_json::from_slice(&preview).map_err(|_| {
        AppError::InvalidInput("Poster 图文预检回执无法读取，本组可安全重试".into())
    })?;
    validate_sets_receipt(request, &preview, false)?;
    let data = run(executable, &["import-image-sets", &path, "--apply"]).await?;
    let receipt: SetsReceipt = serde_json::from_slice(&data)
        .map_err(|_| AppError::InvalidInput("Poster 图文回执无法读取，本组可安全重试".into()))?;
    validate_sets_receipt(request, &receipt, true)?;
    Ok(receipt)
}

pub(super) fn validate_sets_receipt(
    request: &SetsRequest,
    receipt: &SetsReceipt,
    applied: bool,
) -> AppResult<()> {
    let mut seen = std::collections::HashSet::new();
    let valid =
        receipt.schema == SETS_SCHEMA
            && receipt.source_id == request.source_id
            && receipt.batch_id == request.batch_id
            && receipt.applied == applied
            && receipt.items.len() == request.items.len()
            && receipt.items.iter().all(|result| {
                let Some(item) = request.items.iter().find(|i| {
                    i.external_id == result.external_id && i.product_id == result.product_id
                }) else {
                    return false;
                };
                let has_id = result.id.as_deref().is_some_and(|id| !id.is_empty());
                // 新建与重投都要回报这条图文实际用到的图，张数必须和这一组一致。
                let same_images = result.image_ids.as_ref().is_some_and(|ids| {
                    ids.len() == item.images.len() && ids.iter().all(|id| !id.is_empty())
                });
                seen.insert(&result.external_id)
                    && match result.status.as_str() {
                        "ready" => !applied && same_images,
                        "imported" => applied && has_id && same_images,
                        "duplicate" => has_id && same_images,
                        // 用户在 Poster 里删了这条图文：不复活，也不再要求图片对得上。
                        "deleted" => has_id,
                        _ => false,
                    }
            });
    if valid {
        Ok(())
    } else {
        Err(AppError::InvalidInput(
            "Poster 图文回执与本组图片不一致，未标记完成".into(),
        ))
    }
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
                && matches!(
                    result.status.as_str(),
                    "imported" | "updated" | "duplicate" | "deleted"
                )
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
                live_video: None,
                purpose: None,
                type_id: None,
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
                type_id: None,
            }],
        };
        assert!(validate_receipt(&request, &receipt).is_ok());
        receipt.items[0].product_id = "another-star".into();
        assert!(validate_receipt(&request, &receipt).is_err());
        receipt.items.clear();
        assert!(validate_receipt(&request, &receipt).is_err());
    }
}
