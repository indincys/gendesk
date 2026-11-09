//! 参考图库：按款式、分类长期保存参考图，工单按「款式 + 分类」随机抽图（0051）。
//!
//! ## 抽图规则
//!
//! 每次抽一张，候选是该款式该分类下**启用且未移出**的图，分三档依次退让：
//!
//! 1. **可用**：不在冷却期内、本单也还没用过 —— 在这一档里均匀随机。
//! 2. **冷却中**：冷却期内用过、本单没用过 —— 取最久没用的那张。
//! 3. **本单已用**：本单里已经抽到过 —— 取本单用得最少、再最久没用的那张。
//!
//! 冷却天数按分类设（默认 7 天，0 = 不冷却）。可用图不够时不拒单，而是退到下一档并在
//! 使用台账里标 `fallback`，回执里能看到哪几张是退而求其次。同一条目内绝不重复同一张图。
//!
//! ## 什么时候算「用过」
//!
//! 工单收录时抽图，抽中即记账（与收录检查点同事务）。所以抽图恰好一次：重试沿用
//! 检查点里的结果，同一份清单重复 submit 复用原 jobId 也不会重抽。工单停在 hold 时
//! 还没抽，确认后才抽。手动预览（`pick`）默认不记账，需要时显式标记。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use specta::Type;
use sqlx::{SqliteConnection, SqlitePool};

use crate::db::repo::ref_library as repo;
use crate::db::repo::refs as refs_repo;
use crate::error::{AppError, AppResult};
use crate::files::DataDirs;

const DAY: i64 = 86_400;
/// 单次随机抽取上限（预览用）。
pub const MAX_PICK: usize = 64;

// ───────────────────────────── 视图 ─────────────────────────────

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LibraryCategoryView {
    pub id: i64,
    pub name: String,
    /// 冷却天数；0 = 不冷却。
    pub cooldown_days: i64,
    pub sort_order: i64,
    /// 全部款式下这一类未移出的图片数。
    pub image_count: i64,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LibraryCategoryCount {
    pub category_id: i64,
    pub total: i64,
    pub enabled: i64,
    /// 启用且此刻不在冷却期的张数。
    pub available: i64,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LibraryStyleView {
    pub id: i64,
    pub name: String,
    pub product: String,
    pub note: String,
    pub total: i64,
    pub available: i64,
    pub by_category: Vec<LibraryCategoryCount>,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LibraryOverview {
    pub styles: Vec<LibraryStyleView>,
    pub categories: Vec<LibraryCategoryView>,
    /// 统计所用的「此刻」（unix 秒），界面据此算剩余冷却。
    pub now: i64,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LibraryImageView {
    pub id: i64,
    pub style_id: i64,
    pub style: String,
    pub product: String,
    pub category_id: i64,
    pub category: String,
    pub name: String,
    pub file_path: String,
    pub thumb_path: String,
    pub width: i64,
    pub height: i64,
    pub note: String,
    pub enabled: bool,
    pub removed: bool,
    pub last_used_at: Option<i64>,
    pub use_count: i64,
    /// 冷却到什么时候（unix 秒）；不在冷却期为空。
    pub cooling_until: Option<i64>,
    pub created_at: i64,
}

impl LibraryImageView {
    fn from_row(r: repo::ImageRow, now: i64) -> Self {
        let cooling_until = cooling_until(r.last_used_at, r.cooldown_days * DAY, now);
        Self {
            id: r.id,
            style_id: r.style_id,
            style: r.style,
            product: r.product,
            category_id: r.category_id,
            category: r.category,
            name: r.name,
            file_path: r.file_path,
            thumb_path: r.thumb_path,
            width: r.width,
            height: r.height,
            note: r.note,
            enabled: r.enabled,
            removed: r.deleted_at.is_some(),
            last_used_at: r.last_used_at,
            use_count: r.use_count,
            cooling_until,
            created_at: r.created_at,
        }
    }
}

fn cooling_until(last_used_at: Option<i64>, cooldown_secs: i64, now: i64) -> Option<i64> {
    let until = last_used_at? + cooldown_secs;
    (cooldown_secs > 0 && until > now).then_some(until)
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LibraryImportIssue {
    pub path: String,
    pub message: String,
    /// 重复时指向库里已有的那张。
    pub image_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LibraryImportReport {
    pub imported: Vec<LibraryImageView>,
    /// 之前移出、这次重新导入而放回的图。
    pub restored: Vec<LibraryImageView>,
    /// 本款式里已经有的同一张图，未重复入库。
    pub duplicates: Vec<LibraryImportIssue>,
    pub failed: Vec<LibraryImportIssue>,
}

/// 抽中的一张及其档位：fresh = 可用；cooling = 冷却期内复用；repeat = 同单重复。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LibraryPick {
    pub tier: String,
    pub image: LibraryImageView,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LibraryPickResult {
    pub style: String,
    pub category: String,
    pub requested: i64,
    /// 启用且未移出的候选总数。
    pub pool_size: i64,
    /// 其中此刻可用（不在冷却期）的张数。
    pub fresh_count: i64,
    pub picks: Vec<LibraryPick>,
    /// 是否已记为使用。
    pub marked_used: bool,
    pub message: String,
}

/// 某份工单抽到的图（回执与 CLI 用）。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct JobLibraryPick {
    pub item_name: String,
    /// 条目内第几张参考图，1 起。
    pub slot: i64,
    pub fallback: bool,
    pub image: LibraryImageView,
}

// ───────────────────────────── 工单里的图库引用 ─────────────────────────────

/// 工单条目里的一张图库参考图：要么点名 `libraryId`，要么按分类随机抽。
///
/// 随机抽时款式缺省取条目归属里的款式（organization.style，旧单取 style），
/// 产品缺省取 organization.product。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LibraryRef {
    pub library_id: Option<i64>,
    pub category: Option<String>,
    pub style: Option<String>,
    pub product: Option<String>,
}

/// 校验后的引用（款式已按归属补齐）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibrarySlot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub library_id: Option<i64>,
    #[serde(default)]
    pub style: String,
    #[serde(default)]
    pub product: String,
    #[serde(default)]
    pub category: String,
}

impl LibraryRef {
    /// 补齐默认款式与产品，并检查写法。纯函数，不查库。
    pub fn resolve(&self, style: &str, product: &str) -> Result<LibrarySlot, String> {
        let text = |v: &Option<String>| v.as_deref().map(str::trim).unwrap_or("").to_string();
        if let Some(id) = self.library_id {
            if self.category.is_some() || self.style.is_some() || self.product.is_some() {
                return Err("图库引用写了 libraryId 就不要再写 category/style/product".into());
            }
            if id <= 0 {
                return Err(format!("libraryId 必须是正整数：{id}"));
            }
            return Ok(LibrarySlot {
                library_id: Some(id),
                style: String::new(),
                product: String::new(),
                category: String::new(),
            });
        }
        let category = text(&self.category);
        if category.is_empty() {
            return Err("图库引用要写 category（如 正面图）或 libraryId".into());
        }
        let explicit_style = text(&self.style);
        let (style, product) = if explicit_style.is_empty() {
            (
                style.trim().to_string(),
                text(&self.product).or_if_empty(product.trim()),
            )
        } else {
            (explicit_style, text(&self.product))
        };
        if style.is_empty() {
            return Err(format!(
                "图库引用（{category}）没有款式：在引用里写 style，或给条目写 organization.style"
            ));
        }
        Ok(LibrarySlot {
            library_id: None,
            style,
            product,
            category,
        })
    }
}

trait OrIfEmpty {
    fn or_if_empty(self, other: &str) -> String;
}
impl OrIfEmpty for String {
    fn or_if_empty(self, other: &str) -> String {
        if self.is_empty() {
            other.to_string()
        } else {
            self
        }
    }
}

impl LibrarySlot {
    /// 写进工单内容指纹的稳定文本。
    pub fn fingerprint(&self) -> String {
        format!(
            "library:{}",
            serde_json::to_string(self).unwrap_or_default()
        )
    }
    pub fn describe(&self) -> String {
        match self.library_id {
            Some(id) => format!("图库 #{id}"),
            None if self.product.is_empty() => format!("图库 {}/{}", self.style, self.category),
            None => format!("图库 {}/{}/{}", self.product, self.style, self.category),
        }
    }
}

// ───────────────────────────── 抽图（纯逻辑） ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Fresh,
    Cooling,
    Repeat,
}

impl Tier {
    pub fn code(self) -> &'static str {
        match self {
            Tier::Fresh => "fresh",
            Tier::Cooling => "cooling",
            Tier::Repeat => "repeat",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Candidate {
    pub id: i64,
    pub last_used_at: Option<i64>,
    pub cooldown_secs: i64,
}

impl Candidate {
    fn cooling(&self, now: i64) -> bool {
        cooling_until(self.last_used_at, self.cooldown_secs, now).is_some()
    }
}

/// 从候选里抽一张。`exclude` 是硬排除（同一条目已选），`in_job` 是本单已用次数。
/// `rng(n)` 返回 `0..n` 的随机下标。
pub fn choose(
    cands: &[Candidate],
    now: i64,
    exclude: &HashSet<i64>,
    in_job: &HashMap<i64, u32>,
    rng: &mut dyn FnMut(usize) -> usize,
) -> Option<(usize, Tier)> {
    let open: Vec<usize> = (0..cands.len())
        .filter(|&i| !exclude.contains(&cands[i].id))
        .collect();
    let fresh: Vec<usize> = open
        .iter()
        .copied()
        .filter(|&i| !in_job.contains_key(&cands[i].id) && !cands[i].cooling(now))
        .collect();
    if !fresh.is_empty() {
        return Some((fresh[rng(fresh.len()) % fresh.len()], Tier::Fresh));
    }
    let last = |i: usize| cands[i].last_used_at.unwrap_or(i64::MIN);
    let oldest = |pool: Vec<usize>,
                  key: &dyn Fn(usize) -> (u32, i64),
                  rng: &mut dyn FnMut(usize) -> usize| {
        let best = pool.iter().map(|&i| key(i)).min()?;
        let tied: Vec<usize> = pool.into_iter().filter(|&i| key(i) == best).collect();
        Some(tied[rng(tied.len()) % tied.len()])
    };
    let cooling: Vec<usize> = open
        .iter()
        .copied()
        .filter(|&i| !in_job.contains_key(&cands[i].id))
        .collect();
    if !cooling.is_empty() {
        return oldest(cooling, &|i| (0, last(i)), rng).map(|i| (i, Tier::Cooling));
    }
    let used = |i: usize| in_job.get(&cands[i].id).copied().unwrap_or(0);
    oldest(open, &|i| (used(i), last(i)), rng).map(|i| (i, Tier::Repeat))
}

/// 系统随机源（与 UID 同一个 OsRng）。
pub fn os_rng(n: usize) -> usize {
    use chacha20poly1305::aead::rand_core::RngCore;
    if n == 0 {
        return 0;
    }
    (chacha20poly1305::aead::OsRng.next_u64() % n as u64) as usize
}

// ───────────────────────────── 查询 ─────────────────────────────

pub async fn overview(pool: &SqlitePool, now: i64) -> AppResult<LibraryOverview> {
    let counts = repo::counts(pool, now).await?;
    let categories = repo::list_categories(pool)
        .await?
        .into_iter()
        .map(|c| LibraryCategoryView {
            image_count: counts
                .iter()
                .filter(|n| n.category_id == c.id)
                .map(|n| n.total)
                .sum(),
            id: c.id,
            name: c.name,
            cooldown_days: c.cooldown_days,
            sort_order: c.sort_order,
        })
        .collect();
    let styles = repo::list_styles(pool)
        .await?
        .into_iter()
        .map(|s| {
            let by_category: Vec<_> = counts
                .iter()
                .filter(|n| n.style_id == s.id)
                .map(|n| LibraryCategoryCount {
                    category_id: n.category_id,
                    total: n.total,
                    enabled: n.enabled,
                    available: n.available,
                })
                .collect();
            LibraryStyleView {
                total: by_category.iter().map(|c| c.total).sum(),
                available: by_category.iter().map(|c| c.available).sum(),
                id: s.id,
                name: s.name,
                product: s.product,
                note: s.note,
                by_category,
            }
        })
        .collect();
    Ok(LibraryOverview {
        styles,
        categories,
        now,
    })
}

pub async fn list_images(
    pool: &SqlitePool,
    style_id: Option<i64>,
    category_id: Option<i64>,
    include_removed: bool,
    now: i64,
) -> AppResult<Vec<LibraryImageView>> {
    Ok(
        repo::list_images(pool, style_id, category_id, include_removed)
            .await?
            .into_iter()
            .map(|r| LibraryImageView::from_row(r, now))
            .collect(),
    )
}

/// 按名字找款式。产品写了就优先精确匹配；没写或库里款式没标产品时，同名唯一即可。
pub async fn find_style(pool: &SqlitePool, name: &str, product: &str) -> AppResult<repo::StyleRow> {
    let name = name.trim();
    let product = product.trim();
    let mut rows = repo::styles_named(pool, name).await?;
    if rows.is_empty() {
        let all: Vec<String> = repo::list_styles(pool)
            .await?
            .into_iter()
            .map(|s| s.name)
            .collect();
        return Err(AppError::InvalidInput(format!(
            "参考图库没有款式「{name}」。已有：{}。先用 gendesk library style add 建款式",
            if all.is_empty() {
                "（无）".into()
            } else {
                all.join(" / ")
            }
        )));
    }
    if !product.is_empty() {
        if let Some(i) = rows.iter().position(|s| s.product == product) {
            return Ok(rows.swap_remove(i));
        }
    }
    if rows.len() == 1 {
        return Ok(rows.swap_remove(0));
    }
    if let Some(i) = rows.iter().position(|s| s.product.is_empty()) {
        if product.is_empty() {
            return Ok(rows.swap_remove(i));
        }
    }
    Err(AppError::InvalidInput(format!(
        "款式「{name}」在多个产品下都有（{}），请写明 product",
        rows.iter()
            .map(|s| if s.product.is_empty() {
                "未标产品".to_string()
            } else {
                s.product.clone()
            })
            .collect::<Vec<_>>()
            .join(" / ")
    )))
}

pub async fn find_category(pool: &SqlitePool, name: &str) -> AppResult<repo::CategoryRow> {
    let name = name.trim();
    match repo::category_named(pool, name).await? {
        Some(c) => Ok(c),
        None => {
            let all: Vec<String> = repo::list_categories(pool)
                .await?
                .into_iter()
                .map(|c| c.name)
                .collect();
            Err(AppError::InvalidInput(format!(
                "参考图库没有分类「{name}」。已有：{}。新分类用 gendesk library category add 建",
                all.join(" / ")
            )))
        }
    }
}

// ───────────────────────────── 维护 ─────────────────────────────

fn clean_name(raw: &str, label: &str) -> AppResult<String> {
    let v = raw.trim();
    if v.is_empty() {
        return Err(AppError::InvalidInput(format!("{label}不能为空")));
    }
    if v.len() > 180
        || v.chars()
            .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
    {
        return Err(AppError::InvalidInput(format!(
            "{label}「{v}」过长或含路径分隔符、保留字符"
        )));
    }
    Ok(v.to_string())
}

fn unique_violation(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(d) if d.message().contains("UNIQUE"))
}

/// 新建或修改款式，返回 id。
pub async fn save_style(
    pool: &SqlitePool,
    id: Option<i64>,
    name: &str,
    product: &str,
    note: &str,
) -> AppResult<i64> {
    let name = clean_name(name, "款式名")?;
    let product = product.trim();
    if !product.is_empty() {
        clean_name(product, "产品名")?;
    }
    let result = match id {
        Some(id) => repo::update_style(pool, id, &name, product, note.trim())
            .await
            .map(|ok| ok.then_some(id)),
        None => repo::insert_style(pool, &name, product, note.trim(), crate::db::now_unix())
            .await
            .map(Some),
    };
    match result {
        Ok(Some(id)) => Ok(id),
        Ok(None) => Err(AppError::InvalidInput("款式不存在".into())),
        Err(e) if unique_violation(&e) => Err(AppError::InvalidInput(format!(
            "已有同名款式「{name}」{}",
            if product.is_empty() {
                String::new()
            } else {
                format!("（{product}）")
            }
        ))),
        Err(e) => Err(e.into()),
    }
}

pub async fn delete_style(pool: &SqlitePool, id: i64) -> AppResult<()> {
    if repo::delete_empty_style(pool, id).await? {
        Ok(())
    } else {
        Err(AppError::InvalidInput(
            "款式下还有图片（含已移出的），不能删除；可以改名".into(),
        ))
    }
}

pub fn check_cooldown(days: i64) -> AppResult<i64> {
    if (0..=365).contains(&days) {
        Ok(days)
    } else {
        Err(AppError::InvalidInput(format!(
            "冷却天数要在 0～365 之间：{days}"
        )))
    }
}

/// 新建或修改分类，返回 id。`sort_order` 为空时新分类排最后、旧分类不变。
pub async fn save_category(
    pool: &SqlitePool,
    id: Option<i64>,
    name: &str,
    cooldown_days: i64,
    sort_order: Option<i64>,
) -> AppResult<i64> {
    let name = clean_name(name, "分类名")?;
    let cooldown_days = check_cooldown(cooldown_days)?;
    let result = match id {
        Some(id) => {
            let old = repo::get_category(pool, id)
                .await?
                .ok_or_else(|| AppError::InvalidInput("分类不存在".into()))?;
            repo::update_category(
                pool,
                id,
                &name,
                cooldown_days,
                sort_order.unwrap_or(old.sort_order),
            )
            .await
            .map(|_| id)
        }
        None => {
            let order = match sort_order {
                Some(o) => o,
                None => repo::max_category_order(pool).await? + 10,
            };
            repo::insert_category(pool, &name, cooldown_days, order, crate::db::now_unix()).await
        }
    };
    match result {
        Ok(id) => Ok(id),
        Err(e) if unique_violation(&e) => {
            Err(AppError::InvalidInput(format!("已有同名分类「{name}」")))
        }
        Err(e) => Err(e.into()),
    }
}

pub async fn delete_category(pool: &SqlitePool, id: i64) -> AppResult<()> {
    if repo::delete_empty_category(pool, id).await? {
        Ok(())
    } else {
        Err(AppError::InvalidInput(
            "分类下还有图片（含已移出的），不能删除；可以先把图片移到别的分类".into(),
        ))
    }
}

/// 批量修改图片：改分类、启停、备注、移出或放回。
#[derive(Debug, Clone, Default, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LibraryImagePatch {
    pub category_id: Option<i64>,
    pub enabled: Option<bool>,
    pub note: Option<String>,
    /// true = 移出图库（保留文件，不再参与抽取）；false = 放回。
    pub removed: Option<bool>,
}

pub async fn update_images(
    pool: &SqlitePool,
    ids: &[i64],
    patch: &LibraryImagePatch,
) -> AppResult<i64> {
    if ids.is_empty() {
        return Err(AppError::InvalidInput("没有选中图片".into()));
    }
    if let Some(c) = patch.category_id {
        if repo::get_category(pool, c).await?.is_none() {
            return Err(AppError::InvalidInput("目标分类不存在".into()));
        }
    }
    let now = crate::db::now_unix();
    let mut tx = pool.begin().await?;
    let mut changed = 0;
    for &id in ids {
        if repo::get_image(&mut tx, id).await?.is_none() {
            return Err(AppError::InvalidInput(format!("图库里没有 #{id}")));
        }
        if let Some(c) = patch.category_id {
            repo::set_category(&mut tx, id, c).await?;
        }
        if let Some(e) = patch.enabled {
            repo::set_enabled(&mut tx, id, e).await?;
        }
        if let Some(n) = &patch.note {
            repo::set_note(&mut tx, id, n.trim()).await?;
        }
        if let Some(r) = patch.removed {
            repo::set_deleted(&mut tx, id, r.then_some(now)).await?;
        }
        changed += 1;
    }
    tx.commit().await?;
    Ok(changed)
}

/// 把本地图片导入某款式某分类。逐张容错：坏图只记失败，不中断整批。
///
/// 文件走与生成页、工单同一条落盘路径（`commands::refs::ingest_one`），存进数据目录的
/// `refs/library/`，入库成一行 ref_images + 一行图库记录。同款式里内容相同的图不重复入库。
pub async fn import_files(
    pool: &SqlitePool,
    dirs: &DataDirs,
    style_id: i64,
    category_id: i64,
    paths: &[String],
    progress: &(dyn Fn(i64, i64, &str, i64) + Send + Sync),
) -> AppResult<LibraryImportReport> {
    if repo::get_style(pool, style_id).await?.is_none() {
        return Err(AppError::InvalidInput("款式不存在".into()));
    }
    if repo::get_category(pool, category_id).await?.is_none() {
        return Err(AppError::InvalidInput("分类不存在".into()));
    }
    dirs.init()?;
    let refs_dir = dirs.refs().join("library");
    let thumbs_dir = dirs.thumbs().join("library");
    std::fs::create_dir_all(&refs_dir)?;
    std::fs::create_dir_all(&thumbs_dir)?;
    let total = paths.len() as i64;
    let now = crate::db::now_unix();
    let mut report = LibraryImportReport {
        imported: Vec::new(),
        restored: Vec::new(),
        duplicates: Vec::new(),
        failed: Vec::new(),
    };
    for (i, path) in paths.iter().enumerate() {
        let short = Path::new(path)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| path.clone());
        progress(i as i64, total, &short, report.failed.len() as i64);
        match import_one(
            pool,
            &refs_dir,
            &thumbs_dir,
            style_id,
            category_id,
            path,
            now,
        )
        .await
        {
            Ok(Imported::New(v)) => report.imported.push(v),
            Ok(Imported::Restored(v)) => report.restored.push(v),
            Ok(Imported::Duplicate(id)) => report.duplicates.push(LibraryImportIssue {
                path: path.clone(),
                message: "本款式已有这张图".into(),
                image_id: Some(id),
            }),
            Err(e) => {
                tracing::warn!(path = %path, error = %e, "图库导入失败，跳过该张");
                report.failed.push(LibraryImportIssue {
                    path: path.clone(),
                    message: e.to_string(),
                    image_id: None,
                });
            }
        }
        progress(i as i64 + 1, total, &short, report.failed.len() as i64);
    }
    Ok(report)
}

enum Imported {
    New(LibraryImageView),
    Restored(LibraryImageView),
    Duplicate(i64),
}

async fn import_one(
    pool: &SqlitePool,
    refs_dir: &Path,
    thumbs_dir: &Path,
    style_id: i64,
    category_id: i64,
    path: &str,
    now: i64,
) -> AppResult<Imported> {
    let source = PathBuf::from(path);
    if !source.is_absolute() {
        return Err(AppError::InvalidInput(format!("要用绝对路径：{path}")));
    }
    let hash_src = source.clone();
    let hash = tokio::task::spawn_blocking(move || crate::files::content_hash(&hash_src))
        .await
        .map_err(|e| AppError::Internal(e.to_string()))??;
    if let Some(found) = repo::find_by_hash(pool, style_id, &hash).await? {
        if found.deleted_at.is_none() {
            return Ok(Imported::Duplicate(found.id));
        }
        let mut conn = pool.acquire().await?;
        repo::revive(&mut conn, found.id, category_id).await?;
        let row = repo::get_image(&mut conn, found.id)
            .await?
            .ok_or_else(|| AppError::Internal("放回的图片读不到".into()))?;
        return Ok(Imported::Restored(LibraryImageView::from_row(row, now)));
    }
    let (rd, td, p) = (
        refs_dir.to_path_buf(),
        thumbs_dir.to_path_buf(),
        path.to_string(),
    );
    let ing = crate::files::decode::bounded(move |permit| {
        crate::commands::refs::ingest_one(&p, &rd, &td, permit)
    })
    .await?;
    let content_hash = ing.content_hash.clone().unwrap_or(hash);
    let mut tx = pool.begin().await?;
    let ref_id = refs_repo::insert(
        &mut *tx,
        &refs_repo::NewRefImage {
            name: ing.name,
            file_path: ing.file_path.clone(),
            thumb_path: ing.thumb_path.clone(),
            width: ing.width,
            height: ing.height,
            file_size: ing.file_size,
            content_hash: Some(content_hash.clone()),
            upload_path: ing.upload_path.clone(),
            ephemeral: false,
        },
    )
    .await?;
    let id = match repo::insert_image(&mut tx, style_id, category_id, ref_id, &content_hash, now)
        .await
    {
        Ok(id) => id,
        Err(e) if unique_violation(&e) => {
            drop(tx);
            for f in [Some(ing.file_path), Some(ing.thumb_path), ing.upload_path]
                .into_iter()
                .flatten()
            {
                let _ = std::fs::remove_file(f);
            }
            let found = repo::find_by_hash(pool, style_id, &content_hash)
                .await?
                .ok_or_else(|| AppError::Internal("重复图片读不到".into()))?;
            return Ok(Imported::Duplicate(found.id));
        }
        Err(e) => return Err(e.into()),
    };
    let row = repo::get_image(&mut tx, id)
        .await?
        .ok_or_else(|| AppError::Internal("刚导入的图片读不到".into()))?;
    tx.commit().await?;
    Ok(Imported::New(LibraryImageView::from_row(row, now)))
}

// ───────────────────────────── 预览抽取与手动记账 ─────────────────────────────

/// 按款式与分类随机抽 `count` 张（不重复）。`mark_used` 为真时记一次手动使用。
pub async fn pick(
    pool: &SqlitePool,
    style_id: i64,
    category_id: i64,
    count: usize,
    mark_used: bool,
    now: i64,
    rng: &mut (dyn FnMut(usize) -> usize + Send),
) -> AppResult<LibraryPickResult> {
    if count == 0 || count > MAX_PICK {
        return Err(AppError::InvalidInput(format!(
            "张数要在 1～{MAX_PICK} 之间"
        )));
    }
    let style = repo::get_style(pool, style_id)
        .await?
        .ok_or_else(|| AppError::InvalidInput("款式不存在".into()))?;
    let category = repo::get_category(pool, category_id)
        .await?
        .ok_or_else(|| AppError::InvalidInput("分类不存在".into()))?;
    let mut tx = pool.begin().await?;
    let rows = repo::candidates(&mut tx, style_id, category_id).await?;
    let cands: Vec<Candidate> = rows.iter().map(candidate_of).collect();
    let fresh_count = cands.iter().filter(|c| !c.cooling(now)).count() as i64;
    let mut exclude = HashSet::new();
    let mut picks = Vec::new();
    while picks.len() < count {
        let Some((i, tier)) = choose(&cands, now, &exclude, &HashMap::new(), &mut *rng) else {
            break;
        };
        exclude.insert(cands[i].id);
        if mark_used {
            repo::record_usage(
                &mut tx,
                &repo::Usage {
                    image_id: cands[i].id,
                    used_at: now,
                    source: "manual",
                    job_id: None,
                    item_name: None,
                    slot: None,
                    fallback: tier != Tier::Fresh,
                },
            )
            .await?;
        }
        picks.push(LibraryPick {
            tier: tier.code().into(),
            image: LibraryImageView::from_row(rows[i].clone(), now),
        });
    }
    tx.commit().await?;
    let reused = picks.iter().filter(|p| p.tier != "fresh").count();
    let message = if picks.is_empty() {
        format!("{}/{} 没有启用的图片", style.name, category.name)
    } else if picks.len() < count {
        format!("库里只有 {} 张启用的图，少于要的 {count} 张", picks.len())
    } else if reused > 0 {
        format!("可用图不够，其中 {reused} 张是冷却期内用过的")
    } else {
        "全部来自冷却期外的图".into()
    };
    Ok(LibraryPickResult {
        style: style.name,
        category: category.name,
        requested: count as i64,
        pool_size: cands.len() as i64,
        fresh_count,
        picks,
        marked_used: mark_used,
        message,
    })
}

fn candidate_of(r: &repo::ImageRow) -> Candidate {
    Candidate {
        id: r.id,
        last_used_at: r.last_used_at,
        cooldown_secs: r.cooldown_days * DAY,
    }
}

/// 手动把几张图记为用过（在 GenDesk 以外用了这些图时）。
pub async fn mark_used(pool: &SqlitePool, ids: &[i64], now: i64) -> AppResult<i64> {
    if ids.is_empty() {
        return Err(AppError::InvalidInput("没有给图片编号".into()));
    }
    let mut tx = pool.begin().await?;
    for &id in ids {
        let row = repo::get_image(&mut tx, id)
            .await?
            .ok_or_else(|| AppError::InvalidInput(format!("图库里没有 #{id}")))?;
        let fallback = cooling_until(row.last_used_at, row.cooldown_days * DAY, now).is_some();
        repo::record_usage(
            &mut tx,
            &repo::Usage {
                image_id: id,
                used_at: now,
                source: "manual",
                job_id: None,
                item_name: None,
                slot: None,
                fallback,
            },
        )
        .await?;
    }
    tx.commit().await?;
    Ok(ids.len() as i64)
}

// ───────────────────────────── 工单收录 ─────────────────────────────

/// 工单里一个待抽的图库位置。
pub struct SlotRequest<'a> {
    pub item: usize,
    pub slot: usize,
    pub item_name: &'a str,
    pub spec: &'a LibrarySlot,
}

/// 一个位置抽到的图。存进收录检查点，重试时原样沿用。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlotPick {
    pub item: usize,
    pub slot: usize,
    pub image_id: i64,
    pub ref_image_id: i64,
    pub fallback: bool,
}

/// 为一份工单的全部图库位置抽图并记账，在调用方的事务里完成。
///
/// 先处理点名的 `libraryId`（它们算作本单已用，随机抽时尽量避开），再按出现顺序逐个
/// 随机抽。任一位置无图可抽就整单报错，此时什么都没记。
pub async fn draw_for_job(
    conn: &mut SqliteConnection,
    pool: &SqlitePool,
    job_id: &str,
    requests: &[SlotRequest<'_>],
    now: i64,
    rng: &mut (dyn FnMut(usize) -> usize + Send),
) -> AppResult<Vec<SlotPick>> {
    let mut picks: Vec<Option<SlotPick>> = vec![None; requests.len()];
    let mut in_job: HashMap<i64, u32> = HashMap::new();
    let mut per_item: HashMap<usize, HashSet<i64>> = HashMap::new();
    for (n, req) in requests.iter().enumerate() {
        let Some(id) = req.spec.library_id else {
            continue;
        };
        let row = repo::get_image(&mut *conn, id)
            .await?
            .filter(|r| r.deleted_at.is_none())
            .ok_or_else(|| {
                AppError::InvalidInput(format!(
                    "条目「{}」点名的图库图片 #{id} 不存在或已移出",
                    req.item_name
                ))
            })?;
        if !per_item.entry(req.item).or_default().insert(id) {
            return Err(AppError::InvalidInput(format!(
                "条目「{}」重复引用图库图片 #{id}",
                req.item_name
            )));
        }
        let fallback = in_job.contains_key(&id)
            || cooling_until(row.last_used_at, row.cooldown_days * DAY, now).is_some();
        *in_job.entry(id).or_default() += 1;
        picks[n] = Some(SlotPick {
            item: req.item,
            slot: req.slot,
            image_id: id,
            ref_image_id: row.ref_image_id,
            fallback,
        });
    }
    // 同一（款式, 分类）的候选只查一次；抽中后就地把最近使用改成 now。
    let mut pools: HashMap<(i64, i64), (Vec<repo::ImageRow>, Vec<Candidate>)> = HashMap::new();
    for (n, req) in requests.iter().enumerate() {
        if req.spec.library_id.is_some() {
            continue;
        }
        let style = find_style(pool, &req.spec.style, &req.spec.product).await?;
        let category = find_category(pool, &req.spec.category).await?;
        let key = (style.id, category.id);
        if let std::collections::hash_map::Entry::Vacant(e) = pools.entry(key) {
            let rows = repo::candidates(&mut *conn, style.id, category.id).await?;
            let cands = rows.iter().map(candidate_of).collect();
            e.insert((rows, cands));
        }
        let Some((rows, cands)) = pools.get_mut(&key) else {
            continue;
        };
        let exclude = per_item.entry(req.item).or_default();
        let Some((i, tier)) = choose(cands, now, exclude, &in_job, &mut *rng) else {
            let enabled = cands.len();
            return Err(AppError::InvalidInput(format!(
                "条目「{}」要从 {} 抽图，但{}",
                req.item_name,
                req.spec.describe(),
                if enabled == 0 {
                    "这一类没有启用的图片；先导入或启用图片".to_string()
                } else {
                    format!("这一类只有 {enabled} 张启用的图，不够这个条目不重复地用")
                }
            )));
        };
        let id = cands[i].id;
        exclude.insert(id);
        *in_job.entry(id).or_default() += 1;
        cands[i].last_used_at = Some(now);
        picks[n] = Some(SlotPick {
            item: req.item,
            slot: req.slot,
            image_id: id,
            ref_image_id: rows[i].ref_image_id,
            fallback: tier != Tier::Fresh,
        });
    }
    let mut out = Vec::with_capacity(requests.len());
    for (req, pick) in requests.iter().zip(picks) {
        let pick = pick.ok_or_else(|| AppError::Internal("图库抽图结果缺位".into()))?;
        repo::record_usage(
            &mut *conn,
            &repo::Usage {
                image_id: pick.image_id,
                used_at: now,
                source: "intake",
                job_id: Some(job_id),
                item_name: Some(req.item_name),
                slot: Some(pick.slot as i64 + 1),
                fallback: pick.fallback,
            },
        )
        .await?;
        out.push(pick);
    }
    Ok(out)
}

/// 某份工单抽到的图，按台账顺序。
pub async fn picks_for_job(pool: &SqlitePool, job_id: &str) -> AppResult<Vec<JobLibraryPick>> {
    let now = crate::db::now_unix();
    let mut conn = pool.acquire().await?;
    let mut out = Vec::new();
    for u in repo::usage_for_job(pool, job_id).await? {
        if let Some(row) = repo::get_image(&mut conn, u.image_id).await? {
            out.push(JobLibraryPick {
                item_name: u.item_name.unwrap_or_default(),
                slot: u.slot.unwrap_or(0),
                fallback: u.fallback,
                image: LibraryImageView::from_row(row, now),
            });
        }
    }
    Ok(out)
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LibraryUsageView {
    pub used_at: i64,
    pub source: String,
    pub job_id: Option<String>,
    pub item_name: Option<String>,
    pub slot: Option<i64>,
    pub fallback: bool,
}

pub async fn usage_of_image(pool: &SqlitePool, image_id: i64) -> AppResult<Vec<LibraryUsageView>> {
    Ok(repo::usage_for_image(pool, image_id, 50)
        .await?
        .into_iter()
        .map(|u| LibraryUsageView {
            used_at: u.used_at,
            source: u.source,
            job_id: u.job_id,
            item_name: u.item_name,
            slot: u.slot,
            fallback: u.fallback,
        })
        .collect())
}

/// 不查库地检查一组图库位置能否抽到（validate 用）：每个位置报候选与可用张数。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SlotCheck {
    pub reference: String,
    pub ok: bool,
    pub pool_size: i64,
    pub fresh_count: i64,
    pub message: String,
}

pub async fn check_slots(pool: &SqlitePool, slots: &[LibrarySlot]) -> AppResult<Vec<SlotCheck>> {
    let now = crate::db::now_unix();
    let mut conn = pool.acquire().await?;
    let mut out = Vec::new();
    for slot in slots {
        let reference = slot.describe();
        let result: AppResult<(i64, i64)> = async {
            if let Some(id) = slot.library_id {
                let row = repo::get_image(&mut conn, id)
                    .await?
                    .filter(|r| r.deleted_at.is_none())
                    .ok_or_else(|| {
                        AppError::InvalidInput(format!("图库图片 #{id} 不存在或已移出"))
                    })?;
                let fresh = cooling_until(row.last_used_at, row.cooldown_days * DAY, now).is_none();
                return Ok((1, i64::from(fresh)));
            }
            let style = find_style(pool, &slot.style, &slot.product).await?;
            let category = find_category(pool, &slot.category).await?;
            let rows = repo::candidates(&mut conn, style.id, category.id).await?;
            let fresh = rows
                .iter()
                .map(candidate_of)
                .filter(|c| !c.cooling(now))
                .count();
            Ok((rows.len() as i64, fresh as i64))
        }
        .await;
        out.push(match result {
            Ok((size, fresh)) => SlotCheck {
                ok: size > 0,
                message: if size == 0 {
                    "没有启用的图片".into()
                } else {
                    format!("{size} 张候选，{fresh} 张不在冷却期")
                },
                reference,
                pool_size: size,
                fresh_count: fresh,
            },
            Err(e) => SlotCheck {
                reference,
                ok: false,
                pool_size: 0,
                fresh_count: 0,
                message: e.to_string(),
            },
        });
    }
    Ok(out)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即失败
mod tests {
    use super::*;
    use crate::db::test_support::test_pool;

    fn cand(id: i64, last: Option<i64>, days: i64) -> Candidate {
        Candidate {
            id,
            last_used_at: last,
            cooldown_secs: days * DAY,
        }
    }

    #[test]
    fn fresh_images_win_and_cooling_falls_back_to_the_oldest() {
        let now = 100 * DAY;
        let cands = vec![
            cand(1, Some(now - DAY), 7),
            cand(2, Some(now - 3 * DAY), 7),
            cand(3, Some(now - 8 * DAY), 7),
            cand(4, None, 7),
        ];
        let none = HashSet::new();
        let job = HashMap::new();
        let mut seen = HashSet::new();
        for k in 0..20 {
            let (i, tier) = choose(&cands, now, &none, &job, &mut |n| k % n).unwrap();
            assert_eq!(tier, Tier::Fresh);
            seen.insert(cands[i].id);
        }
        assert_eq!(seen, HashSet::from([3, 4]));
        let exclude = HashSet::from([3, 4]);
        let (i, tier) = choose(&cands, now, &exclude, &job, &mut |_| 0).unwrap();
        assert_eq!((cands[i].id, tier), (2, Tier::Cooling));
    }

    #[test]
    fn zero_cooldown_never_cools_but_still_avoids_repeats_in_one_job() {
        let now = 10 * DAY;
        let cands = vec![cand(1, Some(now), 0), cand(2, Some(now), 0)];
        let job = HashMap::from([(1, 1u32)]);
        let (i, tier) = choose(&cands, now, &HashSet::new(), &job, &mut |_| 0).unwrap();
        assert_eq!((cands[i].id, tier), (2, Tier::Fresh));
        let job = HashMap::from([(1, 2u32), (2, 1u32)]);
        let (i, tier) = choose(&cands, now, &HashSet::new(), &job, &mut |_| 0).unwrap();
        assert_eq!((cands[i].id, tier), (2, Tier::Repeat));
        assert!(choose(&cands, now, &HashSet::from([1, 2]), &job, &mut |_| 0).is_none());
    }

    #[test]
    fn references_take_default_style_from_organization() {
        let r = LibraryRef {
            category: Some(" 正面图 ".into()),
            ..Default::default()
        };
        let slot = r.resolve("王一博", "明星周边").unwrap();
        assert_eq!(
            (
                slot.style.as_str(),
                slot.product.as_str(),
                slot.category.as_str()
            ),
            ("王一博", "明星周边", "正面图")
        );
        let explicit = LibraryRef {
            category: Some("礼盒图".into()),
            style: Some("肖战".into()),
            ..Default::default()
        };
        let slot = explicit.resolve("王一博", "明星周边").unwrap();
        assert_eq!((slot.style.as_str(), slot.product.as_str()), ("肖战", ""));
        assert!(r.resolve("", "").is_err());
        let both = LibraryRef {
            library_id: Some(3),
            category: Some("正面图".into()),
            ..Default::default()
        };
        assert!(both.resolve("王一博", "").is_err());
    }

    fn png(seed: u8) -> Vec<u8> {
        use std::io::Cursor;
        let img = image::RgbImage::from_pixel(2, 2, image::Rgb([seed, 0, 0]));
        let mut buf = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        buf.into_inner()
    }

    async fn seed(pool: &SqlitePool, data: &Path, n: u8) -> (i64, i64, Vec<i64>) {
        let style = save_style(pool, None, "王一博", "", "").await.unwrap();
        let category = find_category(pool, "正面图").await.unwrap().id;
        let src = data.join("src");
        std::fs::create_dir_all(&src).unwrap();
        let mut paths = Vec::new();
        for k in 0..n {
            let p = src.join(format!("正面{k}.png"));
            std::fs::write(&p, png(k)).unwrap();
            paths.push(p.to_string_lossy().to_string());
        }
        let dirs = DataDirs::new(data.join("data"));
        let report = import_files(pool, &dirs, style, category, &paths, &|_, _, _, _| {})
            .await
            .unwrap();
        assert_eq!(report.imported.len(), n as usize);
        (
            style,
            category,
            report.imported.iter().map(|v| v.id).collect(),
        )
    }

    #[tokio::test]
    async fn import_dedupes_within_a_style_and_hides_library_refs_from_the_generate_page() {
        let (pool, dir) = test_pool().await;
        let (style, category, ids) = seed(&pool, dir.path(), 2).await;
        let again = dir
            .path()
            .join("src/正面0.png")
            .to_string_lossy()
            .to_string();
        let dirs = DataDirs::new(dir.path().join("data"));
        let report = import_files(
            &pool,
            &dirs,
            style,
            category,
            std::slice::from_ref(&again),
            &|_, _, _, _| {},
        )
        .await
        .unwrap();
        assert_eq!(report.duplicates.len(), 1);
        assert_eq!(report.duplicates[0].image_id, Some(ids[0]));
        assert!(refs_repo::list_active(&pool).await.unwrap().is_empty());

        update_images(
            &pool,
            &[ids[0]],
            &LibraryImagePatch {
                removed: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let report = import_files(&pool, &dirs, style, category, &[again], &|_, _, _, _| {})
            .await
            .unwrap();
        assert_eq!(report.restored.len(), 1);
        assert!(!report.restored[0].removed);
    }

    #[tokio::test]
    async fn job_draws_skip_recent_images_and_record_usage() {
        let (pool, dir) = test_pool().await;
        let (_, _, ids) = seed(&pool, dir.path(), 3).await;
        let now = crate::db::now_unix();
        mark_used(&pool, &[ids[0]], now - DAY).await.unwrap();
        let spec = LibrarySlot {
            library_id: None,
            style: "王一博".into(),
            product: String::new(),
            category: "正面图".into(),
        };
        let requests: Vec<SlotRequest> = (0..2)
            .map(|item| SlotRequest {
                item,
                slot: 0,
                item_name: if item == 0 { "甲" } else { "乙" },
                spec: &spec,
            })
            .collect();
        let mut tx = pool.begin().await.unwrap();
        let picks = draw_for_job(&mut tx, &pool, "job-1", &requests, now, &mut |_| 0)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let got: HashSet<i64> = picks.iter().map(|p| p.image_id).collect();
        assert_eq!(got, HashSet::from([ids[1], ids[2]]));
        assert!(picks.iter().all(|p| !p.fallback));

        // 第三条只剩冷却中的图：退到最久没用的那张（昨天手动用过的 ids[0]），标 fallback。
        let requests: Vec<SlotRequest> = vec![SlotRequest {
            item: 0,
            slot: 0,
            item_name: "丙",
            spec: &spec,
        }];
        let mut tx = pool.begin().await.unwrap();
        let picks = draw_for_job(&mut tx, &pool, "job-2", &requests, now, &mut |_| 0)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        assert_eq!(picks[0].image_id, ids[0]);
        assert!(picks[0].fallback);
        let recorded = picks_for_job(&pool, "job-1").await.unwrap();
        assert_eq!(recorded.len(), 2);
        assert_eq!(recorded[0].slot, 1);
        assert!(recorded.iter().all(|p| p.image.cooling_until.is_some()));
    }

    #[tokio::test]
    async fn an_item_can_not_use_more_distinct_images_than_the_category_has() {
        let (pool, dir) = test_pool().await;
        seed(&pool, dir.path(), 1).await;
        let spec = LibrarySlot {
            library_id: None,
            style: "王一博".into(),
            product: String::new(),
            category: "正面图".into(),
        };
        let requests: Vec<SlotRequest> = (0..2)
            .map(|slot| SlotRequest {
                item: 0,
                slot,
                item_name: "甲",
                spec: &spec,
            })
            .collect();
        let mut tx = pool.begin().await.unwrap();
        let err = draw_for_job(&mut tx, &pool, "job", &requests, 0, &mut |_| 0)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("只有 1 张"), "{err}");
        drop(tx);
        let used: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ref_library_usage")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(used, 0);
    }

    #[tokio::test]
    async fn styles_resolve_by_name_and_product() {
        let (pool, _d) = test_pool().await;
        save_style(&pool, None, "奶油白款", "陶瓷水杯", "")
            .await
            .unwrap();
        assert_eq!(
            find_style(&pool, "奶油白款", "").await.unwrap().product,
            "陶瓷水杯"
        );
        save_style(&pool, None, "奶油白款", "保温杯", "")
            .await
            .unwrap();
        assert!(find_style(&pool, "奶油白款", "").await.is_err());
        assert_eq!(
            find_style(&pool, "奶油白款", "保温杯")
                .await
                .unwrap()
                .product,
            "保温杯"
        );
        assert!(save_style(&pool, None, "奶油白款", "保温杯", "")
            .await
            .is_err());
        assert!(find_style(&pool, "不存在", "").await.is_err());
    }
}
