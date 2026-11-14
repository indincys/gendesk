//! 审稿通过的图自动导入 Poster 对应款式（设置里手动开启，默认关闭）。
//!
//! GenDesk 负责款式适配、交接台账（0046/0050/0055）与安全重试；Poster 只收内容。开启后验收
//! 不再询问：自动按已保存的绑定或 20 款明星名匹配商品；匹配不上的款式让批次停在「失败」并写明
//! 原因，用户在设置里指定一次商品后自动续导。同时开启实况图时，批次等配对落盘后带着视频一起导入。
//!
//! 通过的图按 [`set_key`] 分组：同一次提交 × 同一款式 × 同一需求的图是同一套背景，组成 Poster 里
//! 的一条图文（图片包，`import-image-sets`，不带文案，在 Poster 里配）。同组的图可能分几次通过，
//! 所以每组是一个独立的批次，跨验收收图，等这一组没有图待审了才按任务编号排序组成；第 1 张是封面。
//! 进封面库的图、以及这一组图文组成之后才通过的图，照旧每次验收一个批次散进图片库（`import-images`）。
//!
//! 同一套匹配也用在投单前（`unresolved`，对不上就让工单待确认、不生图）、CLI 绑定
//! （`bind_style`）与历史成品补导（`backfill`）。
//!
//! 商品之内进哪个库位（封面库、某个图片类型、未分类）由作品归属的 `posterType` 或需求名决定，
//! 见 `slot`；在交付时按当时的 Poster 类型目录解析，随请求一起冻结。
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chacha20poly1305::aead::{rand_core::RngCore, OsRng};
use serde::{Deserialize, Serialize};
use specta::Type;
use sqlx::{FromRow, Sqlite, SqlitePool, Transaction};

use crate::db::{now_unix, repo::settings};
use crate::error::{AppError, AppResult};
use crate::library::Organization;

pub mod backfill;
pub mod client;
pub mod slot;
pub(crate) mod storage;
#[cfg(test)]
mod tests;

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PosterSettings {
    /// 审稿通过后自动导入。新字段名：旧版 `enabled`（逐批询问）不会被当成同意自动导入。
    #[serde(default)]
    pub auto_import: bool,
    pub executable: String,
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

pub async fn load_settings(pool: &SqlitePool) -> AppResult<PosterSettings> {
    match settings::get_by_key(pool, "poster").await? {
        Some(json) => serde_json::from_str(&json)
            .map_err(|e| AppError::InvalidInput(format!("Poster 设置无法读取：{e}"))),
        None => Ok(PosterSettings {
            auto_import: false,
            executable: client::discover_executable().to_string_lossy().into(),
        }),
    }
}

/// 作品记录被删除后仍要导入时，原图副本落在这里（新作品直接引用成品，不复制）。
pub fn fallback_root(data_root: &Path) -> PathBuf {
    data_root.join("Poster图片库")
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
    // 验收过程中中断：已提交的作品在开启自动导入时就已同意进入 Poster。
    sqlx::query("UPDATE poster_batches SET state='queued' WHERE state='preparing'")
        .execute(pool)
        .await?;
    Ok(())
}

/// 验收开始时建批（仅自动导入开启时）。返回是否参与导入。
pub async fn begin_review(pool: &SqlitePool, data_root: &Path, id: &str) -> AppResult<bool> {
    if !load_settings(pool).await?.auto_import {
        return Ok(false);
    }
    sqlx::query(
        "INSERT INTO poster_batches(id,created_at,state,library_root) VALUES(?,?,'preparing',?)",
    )
    .bind(id)
    .bind(now_unix())
    .bind(fallback_root(data_root).to_string_lossy().as_ref())
    .execute(pool)
    .await?;
    Ok(true)
}

/// 验收结束：有图就排队，没有图（全部失败或无结果图）就删掉空批次。
pub async fn finish_review(pool: &SqlitePool, id: &str) -> AppResult<()> {
    let mut tx = pool.begin().await?;
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM poster_images WHERE batch_id=?")
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    if count == 0 {
        sqlx::query("DELETE FROM poster_batches WHERE id=? AND state='preparing'")
            .bind(id)
            .execute(&mut *tx)
            .await?;
    } else {
        sqlx::query("UPDATE poster_batches SET state='queued' WHERE id=? AND state='preparing'")
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// 一张通过图属于哪一组图片包：同一次提交（GenDesk 批次）× 同一款式 × 同一库位名
/// （显式 `posterType`，缺省需求名）。同一套背景一起生成的图，天然是一套。
/// 进封面库的图不组图文，返回 `None`。键是 JSON 数组，首项是 GenDesk 批次号。
pub fn set_key(
    batch_id: i64,
    organization: &Organization,
    sku: &str,
    style: &str,
) -> Option<String> {
    let (label, _) = slot::wanted(organization);
    if slot::is_cover(label.trim()) {
        return None;
    }
    let (sku, style) = organization.poster_keys(sku, style);
    serde_json::to_string(&(batch_id, sku, style, label.trim())).ok()
}

/// 键里的 GenDesk 批次号（结算时只看这一批的任务）。
fn set_key_batch(key: &str) -> Option<i64> {
    serde_json::from_str::<(i64, String, String, String)>(key)
        .ok()
        .map(|k| k.0)
}

/// 一张作品在交接台账里的位置：任务编号决定图文内顺序；有 `set_key` 的进那一组的图片包。
pub struct Placement<'a> {
    pub task_id: i64,
    pub set_key: Option<&'a str>,
}

/// 验收事务内登记一张作品。`wants_live` = 同时开启了实况图，要等配对。
///
/// 有图片包键时登记进这一组还没组成的图文批次（没有就新建，与登记同一事务）；这一组的图文已经
/// 组成（或已放弃）后才通过的图，Poster 里没法再往那条图文里加，照旧散进图片库。
#[allow(clippy::too_many_arguments)]
pub async fn track_image(
    tx: &mut Transaction<'_, Sqlite>,
    batch: &str,
    work_id: i64,
    uid: &str,
    item_name: &str,
    (sku, style): (&str, &str),
    image_path: &Path,
    wants_live: bool,
    placement: Placement<'_>,
) -> AppResult<()> {
    let target = match placement.set_key {
        Some(key) => open_set_batch(tx, batch, key).await?,
        None => batch.to_string(),
    };
    sqlx::query(
        "INSERT INTO poster_images(batch_id,work_id,uid,item_name,sku,style,image_path,wants_live,task_id)
        VALUES(?,?,?,?,?,?,?,?,?)",
    )
    .bind(&target)
    .bind(work_id)
    .bind(uid)
    .bind(item_name)
    .bind(sku)
    .bind(style)
    .bind(image_path.to_string_lossy().as_ref())
    .bind(wants_live)
    .bind(placement.task_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// 这一组还在收图的图文批次：请求没冻结、没完成也没放弃。没有就新建；已经组成过就退回散图批次。
async fn open_set_batch(
    tx: &mut Transaction<'_, Sqlite>,
    loose: &str,
    key: &str,
) -> AppResult<String> {
    let existing: Vec<(String, String, bool)> = sqlx::query_as(
        "SELECT id,state,request_json IS NULL FROM poster_batches WHERE set_key=? ORDER BY created_at,rowid",
    )
    .bind(key)
    .fetch_all(&mut **tx)
    .await?;
    if let Some((id, _, _)) = existing
        .iter()
        .find(|(_, state, open)| *open && matches!(state.as_str(), "queued" | "failed"))
    {
        return Ok(id.clone());
    }
    if !existing.is_empty() {
        return Ok(loose.to_string());
    }
    let id = new_id();
    // 直接排队：交付时先看这一组还有没有图待审，有就继续等，不会提前组成。
    sqlx::query(
        "INSERT INTO poster_batches(id,created_at,state,library_root,set_key)
        SELECT ?,?,'queued',library_root,? FROM poster_batches WHERE id=?",
    )
    .bind(&id)
    .bind(now_unix())
    .bind(key)
    .bind(loose)
    .execute(&mut **tx)
    .await?;
    Ok(id)
}

/// 这一组还有几张图没审完（待生成、生成中、重试中、待验收）。生成失败的不等：它要人工恢复，
/// 恢复后再通过的图散进图片库。批次退休后任务行不在了，自然是 0。
async fn pending_in_set(pool: &SqlitePool, key: &str) -> AppResult<i64> {
    let Some(batch) = set_key_batch(key) else {
        return Ok(0);
    };
    let rows: Vec<(i64, String, String, String, String)> = sqlx::query_as(
        "SELECT t.id,t.params_json,COALESCE(p.sku_code,''),COALESCE(p.style,''),COALESCE(g.name,'')
        FROM tasks t LEFT JOIN prompts p ON p.id=t.prompt_id LEFT JOIN prompt_groups g ON g.id=p.group_id
        WHERE t.batch_id=? AND t.status IN ('q','run','retry','rev')",
    )
    .bind(batch)
    .fetch_all(pool)
    .await?;
    let mut pending = 0;
    for (task, params, sku, style, group) in rows {
        // 与验收时同一口径定归属；归属读不出的任务验收也会报错，不可能进这一组，不等它。
        let organization = match Organization::from_params(&params) {
            Ok(found) => found.unwrap_or_else(|| Organization::legacy(&sku, &style, &group)),
            Err(error) => {
                tracing::warn!(%error, task, "任务归属无法读取，不计入图片包等待");
                continue;
            }
        };
        if set_key(batch, &organization, &sku, &style).as_deref() == Some(key) {
            pending += 1;
        }
    }
    Ok(pending)
}

/// 实况配对落盘：写回等待它的 Poster 条目。返回是否有批次在等（调用方据此触发导入）。
pub async fn attach_live_pair(
    tx: &mut Transaction<'_, Sqlite>,
    image_path: &str,
    jpg: &Path,
    mov: &Path,
) -> AppResult<bool> {
    let updated = sqlx::query(
        "UPDATE poster_images SET live_image=?,live_video=?
        WHERE image_path=? AND wants_live=1 AND live_image IS NULL",
    )
    .bind(jpg.to_string_lossy().as_ref())
    .bind(mov.to_string_lossy().as_ref())
    .bind(image_path)
    .execute(&mut **tx)
    .await?;
    Ok(updated.rows_affected() > 0)
}

#[derive(Debug, Clone, Serialize, Type, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct PosterBatch {
    pub id: String,
    pub created_at: i64,
    pub state: String,
    pub last_error: String,
    pub count: i64,
    /// 仍在等实况图配对的张数（>0 时批次排队但暂不导入）。
    pub waiting_live: i64,
    /// 这一批是一条图文（图片包），不是散进图片库的图。
    pub is_set: bool,
    /// 图文还在收图时，这一组还没审完的张数（>0 时等审完再组成）。
    #[sqlx(skip)]
    pub waiting_review: i64,
}

pub async fn batches(pool: &SqlitePool) -> AppResult<Vec<PosterBatch>> {
    let mut list: Vec<PosterBatch> = sqlx::query_as(
        "SELECT b.id,b.created_at,b.state,b.last_error,COUNT(i.work_id) AS count,
            SUM(i.wants_live=1 AND i.live_image IS NULL) AS waiting_live,
            b.set_key IS NOT NULL AS is_set
        FROM poster_batches b JOIN poster_images i ON i.batch_id=b.id
        WHERE b.state<>'skipped'
        GROUP BY b.id ORDER BY CASE b.state WHEN 'failed' THEN 0 WHEN 'queued' THEN 1 ELSE 2 END,
        b.created_at DESC,b.rowid DESC LIMIT 50",
    )
    .fetch_all(pool)
    .await?;
    let open: Vec<(String, String)> = sqlx::query_as(
        "SELECT id,set_key FROM poster_batches
        WHERE set_key IS NOT NULL AND request_json IS NULL AND state IN ('queued','failed')",
    )
    .fetch_all(pool)
    .await?;
    for (id, key) in open {
        if let Some(batch) = list.iter_mut().find(|b| b.id == id) {
            batch.waiting_review = pending_in_set(pool, &key).await?;
        }
    }
    Ok(list)
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
    pub wants_live: bool,
    pub live_image: Option<String>,
    pub live_video: Option<String>,
    /// 作品归属快照；作品记录已删时为空（按未分类处理）。
    pub organization_json: Option<String>,
    /// 交付时按 Poster 类型目录解析出的库位。
    #[sqlx(skip)]
    pub slot: slot::Slot,
}

async fn images(pool: &SqlitePool, batch: &str) -> AppResult<Vec<Image>> {
    Ok(sqlx::query_as(
        "SELECT i.uid,i.item_name,i.sku,i.style,i.image_path,i.product_id,i.product_name,
            i.wants_live,i.live_image,i.live_video, w.image_path AS canonical_path,
            w.organization_json
        FROM poster_images i LEFT JOIN accepted_works w ON w.id=i.work_id AND w.uid=i.uid WHERE i.batch_id=?
        ORDER BY i.task_id,i.work_id",
    )
    .bind(batch)
    .fetch_all(pool)
    .await?)
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

/// 作品目录与参考图库统一使用的日历产品名。只有这些产品下才按款式名识别明星，
/// 其他产品的款式名不参与归属（「王一博」可能只是某个杯子的款式名）。
const CALENDAR_PRODUCTS: &[&str] = &["2027明星日历", "明星日历2027"];

/// 款式名对应的明星：正好是「檀健次」或参考图库写法「檀健次款」。不取前缀，
/// 「谢霆锋演唱会款」这类变体款式要用户绑定一次。
fn style_artist(style: &str) -> Option<&'static str> {
    ARTISTS
        .iter()
        .find(|(artist, _)| style == *artist || style.strip_suffix('款') == Some(*artist))
        .map(|(artist, _)| *artist)
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
    let from_style = style_artist(style);
    if from_sku.is_some() && from_style.is_some() && from_sku != from_style {
        return Err("SKU 与明星标记互相矛盾，请先修正款式信息");
    }
    if from_sku.is_some() {
        return Ok(from_sku);
    }
    Ok(from_style.filter(|_| CALENDAR_PRODUCTS.contains(&sku)))
}

async fn bindings(pool: &SqlitePool) -> AppResult<Vec<Binding>> {
    let json = settings::get_by_key(pool, "posterBindings")
        .await?
        .unwrap_or_else(|| "[]".into());
    Ok(serde_json::from_str(&json)?)
}

/// 一个 `(sku, style)` 解析到的 Poster 商品；`None` 时附带原因。
fn resolve<'a>(
    sku: &str,
    style: &str,
    products: &'a [client::PosterProduct],
    bindings: &[Binding],
) -> (Option<&'a client::PosterProduct>, String) {
    let identity = identity(sku, style);
    let bound = bindings.iter().find(|b| b.sku == sku && b.style == style);
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
        _ if product.is_none() && bound.is_some() => "原绑定商品已不可用，请重新核对目标".into(),
        _ if product.is_none() => "尚未绑定款式，请核对后选择对应商品".into(),
        _ => String::new(),
    };
    (product, warning)
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
        let (product, warning) = resolve(&image.sku, &image.style, products, bindings);
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

/// 对不上商品时给人看的款式标签：有产品写「产品 / 款式」，否则只写有的那一个。
fn label(sku: &str, style: &str) -> String {
    match (sku.is_empty(), style.is_empty()) {
        (false, false) => format!("{sku} / {style}"),
        (true, _) => style.into(),
        (false, true) => sku.into(),
    }
}

/// 一个款式解析到哪个 Poster 商品（CLI `poster resolve`、投单检查与补导共用）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetCheck {
    pub product: String,
    pub style: String,
    pub target_id: Option<String>,
    pub target_name: Option<String>,
    pub warning: String,
}

fn check_with(
    keys: &[(String, String)],
    products: &[client::PosterProduct],
    bindings: &[Binding],
) -> Vec<TargetCheck> {
    let mut out: Vec<TargetCheck> = Vec::new();
    for (sku, style) in keys {
        if out.iter().any(|c| &c.product == sku && &c.style == style) {
            continue;
        }
        let (product, warning) = resolve(sku, style, products, bindings);
        out.push(TargetCheck {
            product: sku.clone(),
            style: style.clone(),
            target_id: product.map(|p| p.id.clone()),
            target_name: product.map(|p| p.name.clone()),
            warning,
        });
    }
    out
}

/// 读 Poster 商品目录（含图片类型）。
pub async fn catalog(pool: &SqlitePool) -> AppResult<client::Catalog> {
    let settings = load_settings(pool).await?;
    client::catalog(Path::new(&settings.executable)).await
}

/// 读 Poster 商品目录，逐个款式给出归属。
pub async fn check_targets(
    pool: &SqlitePool,
    keys: &[(String, String)],
) -> AppResult<Vec<TargetCheck>> {
    let products = catalog(pool).await?.products;
    check_targets_in(pool, keys, &products).await
}

/// 同 [`check_targets`]，商品目录由调用方给（CLI 同时要类型目录，只读一次 Poster）。
pub async fn check_targets_in(
    pool: &SqlitePool,
    keys: &[(String, String)],
    products: &[client::PosterProduct],
) -> AppResult<Vec<TargetCheck>> {
    Ok(check_with(keys, products, &bindings(pool).await?))
}

/// 投单前检查显式指定的 Poster 类型（`organization.posterType`）：开启自动导入时，返回对不上
/// 的说明；按需求名自动对应的不检查（对不上只是未分类）。读不到 Poster 时返回错误。
pub async fn unresolved_types(pool: &SqlitePool, labels: &[String]) -> AppResult<Vec<String>> {
    if labels.is_empty() || !load_settings(pool).await?.auto_import {
        return Ok(Vec::new());
    }
    let types = catalog(pool).await?.image_types;
    let mut out = Vec::new();
    for label in labels {
        if let Err(reason) = slot::resolve(label, true, types.as_deref()) {
            if !out.contains(&reason) {
                out.push(reason);
            }
        }
    }
    Ok(out)
}

/// 投单前检查：开启自动导入时，返回审稿通过后进不了 Poster 的款式说明；
/// 未开启或全部能对上时返回空。读不到 Poster 商品时返回错误，由调用方决定是否放行。
pub async fn unresolved(pool: &SqlitePool, keys: &[(String, String)]) -> AppResult<Vec<String>> {
    if keys.is_empty() || !load_settings(pool).await?.auto_import {
        return Ok(Vec::new());
    }
    Ok(check_targets(pool, keys)
        .await?
        .into_iter()
        .filter(|c| c.target_id.is_none())
        .map(|c| format!("「{}」{}", label(&c.product, &c.style), c.warning))
        .collect())
}

/// 有明确明星身份时，不可选成另一位已知明星的商品。
fn guard_choice(sku: &str, style: &str, product: &client::PosterProduct) -> AppResult<()> {
    let identity = identity(sku, style).map_err(|e| AppError::InvalidInput(e.into()))?;
    if let Some(artist) = identity {
        if ARTISTS
            .iter()
            .any(|(other, _)| *other != artist && product.name == format!("{other}日历（2027）"))
        {
            return Err(AppError::InvalidInput(format!(
                "{artist} 的图片不能归入「{}」",
                product.name
            )));
        }
    }
    Ok(())
}

fn upsert_binding(saved: &mut Vec<Binding>, sku: &str, style: &str, product_id: &str) {
    saved.retain(|b| !(b.sku == sku && b.style == style));
    saved.push(Binding {
        sku: sku.into(),
        style: style.into(),
        product_id: product_id.into(),
    });
}

/// CLI `poster bind`：为一个款式指定 Poster 商品（按商品 ID 或完整名称），之后同款自动导入。
/// 停在「需处理」的同款批次由调用方随后唤醒的后台导入一并续上。
pub async fn bind_style(
    pool: &SqlitePool,
    sku: &str,
    style: &str,
    target: &str,
) -> AppResult<TargetCheck> {
    if sku.trim().is_empty() && style.trim().is_empty() {
        return Err(AppError::InvalidInput("产品与款式不能都为空".into()));
    }
    let settings = load_settings(pool).await?;
    let products = client::products(Path::new(&settings.executable)).await?;
    let by_id = products.iter().find(|p| p.id == target);
    let product = match by_id {
        Some(p) => p,
        None => {
            let named = products
                .iter()
                .filter(|p| p.name == target)
                .collect::<Vec<_>>();
            match named.as_slice() {
                [only] => *only,
                [] => {
                    return Err(AppError::InvalidInput(format!(
                        "Poster 里没有商品「{target}」，可用：{}",
                        products
                            .iter()
                            .map(|p| p.name.as_str())
                            .collect::<Vec<_>>()
                            .join("、")
                    )))
                }
                _ => {
                    return Err(AppError::InvalidInput(format!(
                        "Poster 里有多个「{target}」，请改用商品 ID"
                    )))
                }
            }
        }
    };
    guard_choice(sku, style, product)?;
    let mut saved = bindings(pool).await?;
    upsert_binding(&mut saved, sku, style, &product.id);
    settings::set_by_key(pool, "posterBindings", &serde_json::to_string(&saved)?).await?;
    Ok(check_with(&[(sku.into(), style.into())], &products, &saved).remove(0))
}

/// 设置页「指定商品」用：本批各款式与可选商品。
pub async fn preview(pool: &SqlitePool, id: &str) -> AppResult<PosterPreview> {
    let settings = load_settings(pool).await?;
    let products = client::products(Path::new(&settings.executable)).await?;
    let groups = groups(&images(pool, id).await?, &products, &bindings(pool).await?);
    Ok(PosterPreview { groups, products })
}

/// 校验并保存款式 → 商品绑定，写回本批条目。`choices` 必须覆盖 `groups` 的每一组。
async fn apply_bindings(
    pool: &SqlitePool,
    id: &str,
    preview: &PosterPreview,
    choices: &HashMap<&str, &str>,
) -> AppResult<()> {
    let mut saved = bindings(pool).await?;
    let mut targets = Vec::new();
    for group in &preview.groups {
        identity(&group.sku, &group.style).map_err(|e| AppError::InvalidInput(e.into()))?;
        let chosen = choices.get(group.key.as_str()).copied().unwrap_or_default();
        let product = preview
            .products
            .iter()
            .find(|p| p.id == chosen)
            .ok_or_else(|| AppError::InvalidInput("请选择仍有效的 Poster 商品".into()))?;
        guard_choice(&group.sku, &group.style, product)?;
        upsert_binding(&mut saved, &group.sku, &group.style, &product.id);
        targets.push((group, product));
    }
    let mut tx = pool.begin().await?;
    for (group, product) in targets {
        sqlx::query("UPDATE poster_images SET product_id=?,product_name=? WHERE batch_id=? AND sku=? AND style=?")
            .bind(&product.id).bind(&product.name).bind(id).bind(&group.sku).bind(&group.style).execute(&mut *tx).await?;
    }
    sqlx::query("INSERT INTO settings(key,value_json) VALUES('posterBindings',?) ON CONFLICT(key) DO UPDATE SET value_json=excluded.value_json")
        .bind(serde_json::to_string(&saved)?).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

/// 用户为失败批次指定商品：保存绑定并重新排队（之后同款式自动沿用）。
pub async fn bind(pool: &SqlitePool, id: &str, choices: &[PosterChoice]) -> AppResult<()> {
    let state: Option<String> = sqlx::query_scalar("SELECT state FROM poster_batches WHERE id=?")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    if !matches!(state.as_deref(), Some("failed" | "queued")) {
        return Err(AppError::InvalidInput("本批已处理，请刷新后查看".into()));
    }
    let preview = preview(pool, id).await?;
    let mut selected = preview
        .groups
        .iter()
        .filter_map(|g| g.product_id.as_deref().map(|p| (g.key.as_str(), p)))
        .collect::<HashMap<_, _>>();
    selected.extend(
        choices
            .iter()
            .map(|c| (c.key.as_str(), c.product_id.as_str())),
    );
    apply_bindings(pool, id, &preview, &selected).await?;
    sqlx::query("UPDATE poster_batches SET state='queued',last_error='' WHERE id=? AND state IN ('failed','queued')")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// 放弃导入一个失败批次（图片仍在成品目录，只是不进 Poster）。
pub async fn skip(pool: &SqlitePool, id: &str) -> AppResult<()> {
    let updated =
        sqlx::query("UPDATE poster_batches SET state='skipped' WHERE id=? AND state='failed'")
            .bind(id)
            .execute(pool)
            .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::InvalidInput("只有失败的批次可以放弃导入".into()));
    }
    Ok(())
}

/// 处理排队与失败批次。调用方持共用互斥锁；关闭自动导入时原样保留，打开后续导。
pub async fn deliver_pending(pool: &SqlitePool, data_root: &Path) -> AppResult<()> {
    let pending: Vec<String> = sqlx::query_scalar("SELECT id FROM poster_batches WHERE state IN ('queued','failed') ORDER BY created_at,rowid")
        .fetch_all(pool).await?;
    for id in pending {
        if !load_settings(pool).await?.auto_import {
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

/// 本批是否还在等实况图。转换失败的那张把批次转为失败并写明原因。
async fn waiting_for_live(pool: &SqlitePool, items: &[Image]) -> AppResult<bool> {
    let mut waiting = false;
    for item in items
        .iter()
        .filter(|i| i.wants_live && i.live_image.is_none())
    {
        let job: Option<(Option<String>,)> =
            sqlx::query_as("SELECT error FROM live_photo_jobs WHERE image_path=?")
                .bind(&item.image_path)
                .fetch_optional(pool)
                .await?;
        match job {
            Some((Some(error),)) => {
                return Err(AppError::InvalidInput(format!(
                    "「{}」实况图转换失败：{error}。在设置里重试实况图后会自动导入",
                    item.item_name
                )))
            }
            Some((None,)) => waiting = true,
            // 队列里已没有它（例如被手动清掉）：退回只导静态图，不让整批永远卡住。
            None => tracing::warn!(image=%item.image_path, "实况图队列缺少条目，改为导入静态图"),
        }
    }
    Ok(waiting)
}

/// 自动匹配商品：已有绑定优先，其次 20 款明星名唯一匹配。匹配不上就报出款式名。
async fn resolve_products(
    pool: &SqlitePool,
    products: Vec<client::PosterProduct>,
    id: &str,
) -> AppResult<()> {
    let preview = PosterPreview {
        groups: groups(&images(pool, id).await?, &products, &bindings(pool).await?),
        products,
    };
    let missing = preview
        .groups
        .iter()
        .filter(|g| g.product_id.is_none())
        .map(|g| format!("「{}」{}", label(&g.sku, &g.style), g.warning))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(AppError::InvalidInput(format!(
            "未匹配到 Poster 商品：{}。在设置里为本批指定商品后自动续导",
            missing.join("；")
        )));
    }
    let choices = preview
        .groups
        .iter()
        .filter_map(|g| g.product_id.as_deref().map(|p| (g.key.as_str(), p)))
        .collect::<HashMap<_, _>>();
    apply_bindings(pool, id, &preview, &choices).await
}

/// 逐张定库位（封面 / 图片类型 / 未分类）。显式类型对不上时整批停下并写明原因，
/// 在 Poster 建好同名类型（或更新 Poster）后重试即可。
fn assign_slots(
    mut items: Vec<Image>,
    types: Option<&[client::PosterImageType]>,
) -> AppResult<Vec<Image>> {
    let mut problems: Vec<String> = Vec::new();
    for item in &mut items {
        match slot::of_work(item.organization_json.as_deref(), types) {
            Ok(found) => item.slot = found,
            Err(reason) => {
                if !problems.contains(&reason) {
                    problems.push(reason);
                }
            }
        }
    }
    if problems.is_empty() {
        Ok(items)
    } else {
        Err(AppError::InvalidInput(format!(
            "图片类型对不上：{}",
            problems.join("；")
        )))
    }
}

/// 交付前的共同准备：等实况、定商品、定库位，返回按任务编号排好的图与来源实例 ID。
/// `None` = 还在等实况图（批次已放回排队）。
async fn ready_images(
    pool: &SqlitePool,
    executable: &Path,
    id: &str,
) -> AppResult<Option<(Vec<Image>, String)>> {
    if waiting_for_live(pool, &images(pool, id).await?).await? {
        sqlx::query("UPDATE poster_batches SET state='queued',last_error='' WHERE id=?")
            .bind(id)
            .execute(pool)
            .await?;
        return Ok(None);
    }
    let catalog = client::catalog(executable).await?;
    resolve_products(pool, catalog.products, id).await?;
    let items = assign_slots(images(pool, id).await?, catalog.image_types.as_deref())?;
    let source_json = settings::get_by_key(pool, "posterSource")
        .await?
        .ok_or_else(|| AppError::Internal("Poster 来源标识未初始化".into()))?;
    Ok(Some((items, serde_json::from_str(&source_json)?)))
}

fn library_root(root: String, data_root: &Path) -> String {
    if root.is_empty() {
        fallback_root(data_root).to_string_lossy().into_owned()
    } else {
        root
    }
}

/// 一条图文：这一组审完才组成（还有图待审就继续排队等），请求冻结后重试原样复用。
/// 先预览再落库；回执里的图文素材 ID 记进 `material_id` 对账。
async fn deliver_set(
    pool: &SqlitePool,
    data_root: &Path,
    id: &str,
    key: &str,
    root: String,
    request_json: Option<String>,
) -> AppResult<()> {
    let settings = load_settings(pool).await?;
    let executable = Path::new(&settings.executable);
    let request: client::SetsRequest = if let Some(json) = request_json {
        serde_json::from_str(&json)?
    } else {
        if pending_in_set(pool, key).await? > 0 {
            sqlx::query("UPDATE poster_batches SET state='queued',last_error='' WHERE id=?")
                .bind(id)
                .execute(pool)
                .await?;
            return Ok(());
        }
        let Some((items, source_id)) = ready_images(pool, executable, id).await? else {
            return Ok(());
        };
        let count = items.len() as i64;
        let (root, batch) = (library_root(root, data_root), id.to_string());
        let request = tokio::task::spawn_blocking(move || {
            storage::prepare_set(&root, &batch, source_id, &items)
        })
        .await
        .map_err(|e| AppError::Io(e.to_string()))??;
        // 冻结与「这一组的图都在请求里」同一条语句判定：验收与交付不共用一把锁，冻结前若又登记
        // 进了一张图，这次不冻结，等登记它的那次验收结束再来组。
        let frozen = sqlx::query(
            "UPDATE poster_batches SET request_json=? WHERE id=? AND request_json IS NULL
            AND (SELECT COUNT(*) FROM poster_images WHERE batch_id=?)=?",
        )
        .bind(serde_json::to_string(&request)?)
        .bind(id)
        .bind(id)
        .bind(count)
        .execute(pool)
        .await?;
        if frozen.rows_affected() != 1 {
            return Ok(());
        }
        request
    };
    let manifest = data_root.join("Poster交接").join(format!("{id}.json"));
    let bytes = serde_json::to_vec_pretty(&request)?;
    let manifest_path = manifest.clone();
    tokio::task::spawn_blocking(move || storage::atomic_write(&manifest_path, &bytes))
        .await
        .map_err(|e| AppError::Io(e.to_string()))??;
    let receipt = client::import_sets(executable, &manifest, &request).await?;
    let material = receipt.items.first().and_then(|item| item.id.clone());
    sqlx::query(
        "UPDATE poster_batches SET state='done',last_error='',receipt_json=?,material_id=? WHERE id=?",
    )
    .bind(serde_json::to_string(&receipt)?)
    .bind(material)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// 只结算某一次提交下还在收图的图文（那一批有任务生成失败、或有图移进废纸篓时调用），
/// 不顺带重试别的失败批次。
pub async fn settle_sets_of(
    pool: &SqlitePool,
    data_root: &Path,
    gendesk_batch: i64,
) -> AppResult<()> {
    if !load_settings(pool).await?.auto_import {
        return Ok(());
    }
    let open: Vec<(String, String)> = sqlx::query_as(
        "SELECT id,set_key FROM poster_batches
        WHERE set_key IS NOT NULL AND request_json IS NULL AND state='queued' ORDER BY created_at,rowid",
    )
    .fetch_all(pool)
    .await?;
    for (id, key) in open {
        if set_key_batch(&key) != Some(gendesk_batch) {
            continue;
        }
        if let Err(error) = deliver_one(pool, data_root, &id).await {
            sqlx::query("UPDATE poster_batches SET state='failed',last_error=? WHERE id=? AND state IN ('queued','failed')")
                .bind(error.to_string()).bind(&id).execute(pool).await?;
            tracing::warn!(%error, batch=%id, "Poster 图文导入待重试");
        }
    }
    Ok(())
}

async fn deliver_one(pool: &SqlitePool, data_root: &Path, id: &str) -> AppResult<()> {
    let (root, request_json, set_key): (String, Option<String>, Option<String>) =
        sqlx::query_as("SELECT library_root,request_json,set_key FROM poster_batches WHERE id=?")
            .bind(id)
            .fetch_one(pool)
            .await?;
    if let Some(key) = set_key {
        return deliver_set(pool, data_root, id, &key, root, request_json).await;
    }
    let settings = load_settings(pool).await?;
    let executable = Path::new(&settings.executable);
    let request: client::Request = if let Some(json) = request_json {
        serde_json::from_str(&json)?
    } else {
        let Some((items, source_id)) = ready_images(pool, executable, id).await? else {
            return Ok(());
        };
        let batch = id.to_string();
        let root = library_root(root, data_root);
        let request =
            tokio::task::spawn_blocking(move || storage::prepare(&root, &batch, source_id, &items))
                .await
                .map_err(|e| AppError::Io(e.to_string()))??;
        // 请求一经生成就冻结：之后的重试复用同一批路径与哈希，不会因设置变化而改道。
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
    let receipt = client::import(executable, &manifest, &request).await?;
    sqlx::query("UPDATE poster_batches SET state='done',last_error='',receipt_json=? WHERE id=?")
        .bind(serde_json::to_string(&receipt)?)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}
