//! GenDesk 负责审稿后的选择、款式适配、永久图片与持久化交接；Poster 只收图片。
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chacha20poly1305::aead::{rand_core::RngCore, OsRng};
use serde::{Deserialize, Serialize};
use specta::Type;
use sqlx::{FromRow, SqlitePool};

use crate::db::{now_unix, repo::settings};
use crate::error::{AppError, AppResult};

pub mod client;
pub(crate) mod storage;
#[cfg(test)]
mod tests;

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PosterSettings {
    /// 只有主动开启才在审稿后询问；旧设置没有此字段时也保持关闭。
    #[serde(default)]
    pub enabled: bool,
    pub executable: String,
    pub library_root: String,
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn new_id() -> String {
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub async fn load_settings(pool: &SqlitePool, data_root: &Path) -> AppResult<PosterSettings> {
    let stored = settings::get_by_key(pool, "poster").await?;
    match stored {
        Some(json) => serde_json::from_str(&json)
            .map_err(|e| AppError::InvalidInput(format!("Poster 设置无法读取：{e}"))),
        None => Ok(PosterSettings {
            enabled: false,
            executable: client::discover_executable().to_string_lossy().into(),
            library_root: data_root.join("Poster图片库").to_string_lossy().into(),
        }),
    }
}

pub async fn initialize(pool: &SqlitePool) -> AppResult<()> {
    if settings::get_by_key(pool, "posterSource").await?.is_none() {
        settings::set_by_key(
            pool,
            "posterSource",
            &serde_json::to_string(&format!("gendesk-{}", new_id()))?,
        )
        .await?;
    }
    // 接受过程中中断：已有作品仍要问用户；绝不据此推断「已同意导入」。
    sqlx::query("UPDATE poster_batches SET state='awaiting' WHERE state='preparing'")
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn begin_acceptance(pool: &SqlitePool, id: &str) -> AppResult<()> {
    sqlx::query("INSERT INTO poster_batches(id,created_at,state) VALUES(?,?,'preparing')")
        .bind(id)
        .bind(now_unix())
        .execute(pool)
        .await?;
    Ok(())
}

/// 在审稿开始时确定本批是否参与接入；关闭时不建台账、不留待确认副本。
pub async fn begin_review(pool: &SqlitePool, data_root: &Path, id: &str) -> AppResult<bool> {
    let enabled = load_settings(pool, data_root).await?.enabled;
    if enabled {
        begin_acceptance(pool, id).await?;
    }
    Ok(enabled)
}

async fn require_enabled(pool: &SqlitePool, data_root: &Path) -> AppResult<PosterSettings> {
    let settings = load_settings(pool, data_root).await?;
    if !settings.enabled {
        return Err(AppError::InvalidInput(
            "请先在设置中开启「审稿后询问导入 Poster」".into(),
        ));
    }
    Ok(settings)
}

pub async fn finish_acceptance(pool: &SqlitePool, id: &str) -> AppResult<()> {
    sqlx::query("UPDATE poster_batches SET state='awaiting' WHERE id=? AND state='preparing'")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, Type, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct PosterBatch {
    pub id: String,
    pub created_at: i64,
    pub state: String,
    pub last_error: String,
    pub count: i64,
}

pub async fn batches(pool: &SqlitePool) -> AppResult<Vec<PosterBatch>> {
    Ok(sqlx::query_as(
        "SELECT b.id,b.created_at,b.state,b.last_error,COUNT(i.work_id) AS count
        FROM poster_batches b JOIN poster_images i ON i.batch_id=b.id
        GROUP BY b.id ORDER BY CASE b.state WHEN 'awaiting' THEN 0 WHEN 'failed' THEN 1 ELSE 2 END,
        b.created_at DESC,b.rowid DESC LIMIT 100",
    )
    .fetch_all(pool)
    .await?)
}

#[derive(Debug, Clone, FromRow)]
pub struct Image {
    pub canonical_path: Option<String>,
    pub uid: String,
    pub item_name: String,
    pub sku: String,
    pub style: String,
    pub image_path: String,
    pub product_id: Option<String>,
    pub product_name: Option<String>,
}

async fn images(pool: &SqlitePool, batch: &str) -> AppResult<Vec<Image>> {
    Ok(sqlx::query_as(
        "SELECT i.uid,i.item_name,i.sku,i.style,i.image_path,i.product_id,i.product_name, w.image_path AS canonical_path
        FROM poster_images i LEFT JOIN accepted_works w ON w.id=i.work_id AND w.uid=i.uid WHERE i.batch_id=? ORDER BY i.work_id",
    )
    .bind(batch)
    .fetch_all(pool)
    .await?)
}

#[cfg(test)]
pub fn pending_path(data_root: &Path, batch: &str, uid: &str, ext: &str) -> PathBuf {
    data_root
        .join("Poster待确认")
        .join(batch)
        .join(format!("{uid}.{ext}"))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Binding {
    sku: String,
    style: String,
    product_id: String,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PosterGroup {
    pub key: String,
    pub sku: String,
    pub style: String,
    pub count: i64,
    pub product_id: Option<String>,
    pub warning: String,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PosterPreview {
    pub groups: Vec<PosterGroup>,
    pub products: Vec<client::PosterProduct>,
}

#[derive(Debug, Clone, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PosterChoice {
    pub key: String,
    pub product_id: String,
}

// 只认这批产品的完整别名，不从文件名、前缀包含或数字 SKU 猜人。名称只用于首次绑定；
// 用户确认后保存的是 Poster 商品 ID，改商品显示名不改变归属。
const ARTISTS: &[(&str, &str)] = &[
    ("五月天", "mayday"),
    ("刘宇宁", "liuyuning"),
    ("华晨宇", "huachenyu"),
    ("周深", "zhoushen"),
    ("宋雨琦", "songyuqi"),
    ("林俊杰", "linjunjie"),
    ("檀健次", "tanjianci"),
    ("汪苏泷", "wangsulong"),
    ("王一博", "wangyibo"),
    ("王俊凯", "wangjunkai"),
    ("白鹿", "bailu"),
    ("肖战", "xiaozhan"),
    ("蔡徐坤", "caixukun"),
    ("薛之谦", "xuezhiqian"),
    ("虞书欣", "yushuxin"),
    ("谢霆锋", "xietingfeng"),
    ("赵露思", "zhaolusi"),
    ("邓紫棋", "dengziqi"),
    ("陈奕迅", "chenyixun"),
    ("鞠婧祎", "jujingyi"),
];

fn group_key(sku: &str, style: &str) -> String {
    // 长度前缀使任意合法字符都不能让两个二元组碰撞。
    format!("{}:{sku}{style}", sku.len())
}

fn identity(sku: &str, style: &str) -> Result<Option<&'static str>, &'static str> {
    let from_sku = ARTISTS
        .iter()
        .find(|(artist, code)| {
            sku == format!("{artist}日历")
                || sku == format!("{artist}日历（2027）")
                || sku == format!("calendar-2027-{code}")
        })
        .map(|(artist, _)| *artist);
    let from_style = ARTISTS
        .iter()
        .find(|(artist, _)| *artist == style)
        .map(|(artist, _)| *artist);
    if from_sku.is_some() && from_style.is_some() && from_sku != from_style {
        return Err("SKU 与明星标记互相矛盾，请先修正款式信息");
    }
    Ok(from_sku)
}

async fn bindings(pool: &SqlitePool) -> AppResult<Vec<Binding>> {
    let json = settings::get_by_key(pool, "posterBindings")
        .await?
        .unwrap_or_else(|| "[]".into());
    Ok(serde_json::from_str(&json)?)
}

fn groups(
    items: &[Image],
    products: &[client::PosterProduct],
    bindings: &[Binding],
) -> Vec<PosterGroup> {
    let mut result: Vec<PosterGroup> = Vec::new();
    for image in items {
        let key = group_key(&image.sku, &image.style);
        if let Some(existing) = result.iter_mut().find(|g| g.key == key) {
            existing.count += 1;
            continue;
        }
        let identity = identity(&image.sku, &image.style);
        let bound = bindings
            .iter()
            .find(|b| b.sku == image.sku && b.style == image.style);
        let product = if identity.is_err() {
            None
        } else if let Some(binding) = bound {
            // 已绑定的目标被删除时不按名称偷偷接到另一条新商品。
            products.iter().find(|p| p.id == binding.product_id)
        } else if let Ok(Some(artist)) = identity {
            let name = format!("{artist}日历（2027）");
            let matching = products
                .iter()
                .filter(|p| p.name == name)
                .collect::<Vec<_>>();
            if matching.len() == 1 {
                matching.first().copied()
            } else {
                None
            }
        } else {
            None
        };
        let warning = match identity {
            Err(error) => error.into(),
            _ if product.is_none() && bound.is_some() => {
                "原绑定商品已不可用，请重新核对目标".into()
            }
            _ if product.is_none() => "尚未绑定款式，请核对后选择对应商品".into(),
            _ => String::new(),
        };
        result.push(PosterGroup {
            key,
            sku: image.sku.clone(),
            style: image.style.clone(),
            count: 1,
            product_id: product.map(|p| p.id.clone()),
            warning,
        });
    }
    result
}

pub async fn preview(pool: &SqlitePool, data_root: &Path, id: &str) -> AppResult<PosterPreview> {
    let settings = require_enabled(pool, data_root).await?;
    let products = client::products(Path::new(&settings.executable)).await?;
    let groups = groups(&images(pool, id).await?, &products, &bindings(pool).await?);
    Ok(PosterPreview { groups, products })
}

pub async fn confirm(
    pool: &SqlitePool,
    data_root: &Path,
    id: &str,
    choices: &[PosterChoice],
) -> AppResult<()> {
    let settings = require_enabled(pool, data_root).await?;
    let root = Path::new(&settings.library_root);
    if !root.is_absolute() {
        return Err(AppError::InvalidInput(
            "Poster 图片永久目录必须是绝对路径".into(),
        ));
    }
    let preview = preview(pool, data_root, id).await?;
    let selected = choices
        .iter()
        .map(|c| (c.key.as_str(), c.product_id.as_str()))
        .collect::<HashMap<_, _>>();
    if preview.groups.is_empty()
        || selected.len() != choices.len()
        || selected.len() != preview.groups.len()
    {
        return Err(AppError::InvalidInput(
            "请给本批每个款式指定唯一的 Poster 商品".into(),
        ));
    }
    let mut saved = bindings(pool).await?;
    let mut targets = Vec::new();
    for group in &preview.groups {
        identity(&group.sku, &group.style).map_err(|e| AppError::InvalidInput(e.into()))?;
        let chosen = selected
            .get(group.key.as_str())
            .copied()
            .unwrap_or_default();
        let product = preview
            .products
            .iter()
            .find(|p| p.id == chosen)
            .ok_or_else(|| AppError::InvalidInput("请选择仍有效的 Poster 商品".into()))?;
        // 有明确明星身份时，首次绑定不可选成另一位已知明星；已确认的 ID 可随商品改名保留。
        if let Ok(Some(artist)) = identity(&group.sku, &group.style) {
            if ARTISTS.iter().any(|(other, _)| {
                *other != artist && product.name == format!("{other}日历（2027）")
            }) {
                return Err(AppError::InvalidInput(format!(
                    "{artist} 的图片不能归入「{}」",
                    product.name
                )));
            }
        }
        saved.retain(|b| !(b.sku == group.sku && b.style == group.style));
        saved.push(Binding {
            sku: group.sku.clone(),
            style: group.style.clone(),
            product_id: product.id.clone(),
        });
        targets.push((group, product));
    }
    let mut tx = pool.begin().await?;
    let updated = sqlx::query("UPDATE poster_batches SET state='queued',library_root=?,last_error='' WHERE id=? AND state='awaiting'")
        .bind(&settings.library_root).bind(id).execute(&mut *tx).await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::InvalidInput("本批已处理，请刷新后查看".into()));
    }
    for (group, product) in targets {
        sqlx::query("UPDATE poster_images SET product_id=?,product_name=? WHERE batch_id=? AND sku=? AND style=?")
            .bind(&product.id).bind(&product.name).bind(id).bind(&group.sku).bind(&group.style).execute(&mut *tx).await?;
    }
    sqlx::query("INSERT INTO settings(key,value_json) VALUES('posterBindings',?) ON CONFLICT(key) DO UPDATE SET value_json=excluded.value_json")
        .bind(serde_json::to_string(&saved)?).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

pub async fn skip(pool: &SqlitePool, data_root: &Path, id: &str) -> AppResult<()> {
    let updated =
        sqlx::query("UPDATE poster_batches SET state='skipped' WHERE id=? AND state='awaiting'")
            .bind(id)
            .execute(pool)
            .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::InvalidInput(
            "本批已处理，不能取消已确认的导入".into(),
        ));
    }
    cleanup_pending(data_root, id).await;
    Ok(())
}

async fn cleanup_pending(data_root: &Path, id: &str) {
    if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return;
    }
    let path = data_root.join("Poster待确认").join(id);
    if let Err(error) = tokio::fs::remove_dir_all(&path).await {
        if error.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(%error, path=%path.display(), "清理 Poster 待确认副本失败");
        }
    }
}

/// 仅处理已确认的 queued/failed。调用方持共用互斥锁；启动恢复只重试一轮，无后台轮询。
pub async fn deliver_pending(pool: &SqlitePool, data_root: &Path) -> AppResult<()> {
    let pending: Vec<String> = sqlx::query_scalar("SELECT id FROM poster_batches WHERE state IN ('queued','failed') ORDER BY created_at,rowid")
        .fetch_all(pool).await?;
    for id in pending {
        if !load_settings(pool, data_root).await?.enabled {
            break;
        }
        if let Err(error) = deliver_one(pool, data_root, &id).await {
            sqlx::query("UPDATE poster_batches SET state='failed',last_error=? WHERE id=? AND state IN ('queued','failed')")
                .bind(error.to_string()).bind(&id).execute(pool).await?;
            tracing::warn!(%error, batch=%id, "Poster 图片导入待重试");
        }
    }
    Ok(())
}

async fn deliver_one(pool: &SqlitePool, data_root: &Path, id: &str) -> AppResult<()> {
    let (root, request_json): (String, Option<String>) =
        sqlx::query_as("SELECT library_root,request_json FROM poster_batches WHERE id=?")
            .bind(id)
            .fetch_one(pool)
            .await?;
    let request: client::Request = if let Some(json) = request_json {
        serde_json::from_str(&json)?
    } else {
        let items = images(pool, id).await?;
        let source_json = settings::get_by_key(pool, "posterSource")
            .await?
            .ok_or_else(|| AppError::Internal("Poster 来源标识未初始化".into()))?;
        let source_id: String = serde_json::from_str(&source_json)?;
        let batch = id.to_string();
        let request =
            tokio::task::spawn_blocking(move || storage::prepare(&root, &batch, source_id, &items))
                .await
                .map_err(|e| AppError::Io(e.to_string()))??;
        sqlx::query("UPDATE poster_batches SET request_json=? WHERE id=?")
            .bind(serde_json::to_string(&request)?)
            .bind(id)
            .execute(pool)
            .await?;
        request
    };
    let manifest = data_root.join("Poster交接").join(format!("{id}.json"));
    let bytes = serde_json::to_vec_pretty(&request)?;
    let manifest_path = manifest.clone();
    tokio::task::spawn_blocking(move || storage::atomic_write(&manifest_path, &bytes))
        .await
        .map_err(|e| AppError::Io(e.to_string()))??;
    let settings = load_settings(pool, data_root).await?;
    if !settings.enabled {
        return Ok(());
    }
    let receipt = client::import(Path::new(&settings.executable), &manifest, &request).await?;
    sqlx::query("UPDATE poster_batches SET state='done',last_error='',receipt_json=? WHERE id=?")
        .bind(serde_json::to_string(&receipt)?)
        .bind(id)
        .execute(pool)
        .await?;
    cleanup_pending(data_root, id).await;
    Ok(())
}
