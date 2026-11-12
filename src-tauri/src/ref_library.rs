//! 参考图库：按款式、分类长期保存参考图，工单按「款式 + 分类」随机抽图（0051）。
//!
//! ## 抽图规则
//!
//! 每次抽一张，候选是该款式该分类下**启用、未移出且原文件还在**的图（不符合的自动屏蔽），
//! 分几档依次退让：
//!
//! 1. **可用**：不在冷却期内、本单也还没用过 —— 在这一档里均匀随机。
//! 2. **冷却降级**：可用的不够时，冷却天数沿 [`COOLDOWN_LADDER`] 往下放宽
//!    （如 15 → 10 → 7 → 5 → 3 → 2 → 1 天），在第一档凑得出图的冷却天数下**仍然随机**。
//! 3. **最久没用**：一天内全都用过 —— 取最久没用的那张。
//! 4. **本单已用**：本单里已经抽到过 —— 取本单用得最少、再最久没用的那张。
//!
//! 冷却天数按分类设（默认 7 天，0 = 免冷却，适合素材很少的分类）。可用图不够时不拒单，
//! 而是退到下一档并在使用台账里标 `fallback`、记下实际执行的冷却天数，回执里能看到哪几张
//! 是降级抽的、降到了几天。同一条目（或同一个素材包）内绝不重复同一张图。
//!
//! ## 什么时候算「用过」
//!
//! 工单收录时抽图，抽中即记账（与收录检查点同事务）。所以抽图恰好一次：重试沿用
//! 检查点里的结果，同一份清单重复 submit 复用原 jobId 也不会重抽。工单停在 hold 时
//! 还没抽，确认后才抽。手动预览（`pick`）默认不记账，需要时显式标记。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use specta::Type;
use sqlx::{SqliteConnection, SqlitePool};

use crate::db::repo::ref_library as repo;
use crate::db::repo::refs as refs_repo;
use crate::error::{AppError, AppResult};
use crate::files::DataDirs;

/// 图库导入同时处理几张（见 `import_files`）。解码另有全局上限，这个数只管 I/O 重叠。
const IMPORT_PARALLEL: usize = 4;

const DAY: i64 = 86_400;
/// 单次随机抽取上限（预览用）。
pub const MAX_PICK: usize = 64;
/// 新分类不指定时的冷却天数；与 0051 的列默认值一致（测试守着）。
pub const DEFAULT_COOLDOWN_DAYS: i64 = 7;
/// 冷却天数上限；与 0051 的 CHECK 一致（测试守着）。
pub const MAX_COOLDOWN_DAYS: i64 = 365;
/// 冷却降级阶梯（天，从大到小）。只用比分类配置更短的那几档。
pub const COOLDOWN_LADDER: &[i64] = &[30, 15, 10, 7, 5, 3, 2, 1];
/// 一个素材包最多几张图；与工单条目 `refs` 上限一致，抽出的包能原样当一条的参考图。
pub const MAX_PACK_IMAGES: usize = 16;

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
    /// 取值范围，界面输入框照此限制，不自己抄数字。
    pub limits: LibraryLimits,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LibraryLimits {
    pub max_pick: u32,
    pub max_cooldown_days: i64,
    pub default_cooldown_days: i64,
}

impl LibraryLimits {
    pub fn current() -> Self {
        Self {
            max_pick: MAX_PICK as u32,
            max_cooldown_days: MAX_COOLDOWN_DAYS,
            default_cooldown_days: DEFAULT_COOLDOWN_DAYS,
        }
    }
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

/// 抽中的一张及其档位：fresh = 可用；cooling = 冷却期内复用（降级）；repeat = 同单重复。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LibraryPick {
    pub tier: String,
    /// 这一张实际按几天冷却抽出来的；小于分类配置即为降级，0 = 一天内用过、取最久没用的。
    pub cooldown_days: i64,
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
    /// 实际执行的冷却天数（降级后）；旧记录为空。
    pub cooldown_days: Option<i64>,
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
    /// 按 `days` 天冷却算是否已经过了冷却期。
    fn rested(&self, now: i64, days: i64) -> bool {
        self.last_used_at.map_or(true, |t| now - t >= days * DAY)
    }
}

/// 一次抽取的结果：下标、档位，以及实际执行的冷却天数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Choice {
    pub index: usize,
    pub tier: Tier,
    pub cooldown_days: i64,
}

/// 从候选里抽一张。`exclude` 是硬排除（同一条目已选），`in_job` 是本单已用次数。
/// `rng(n)` 返回 `0..n` 的随机下标。只给测试用；业务路径都要降级天数，走 [`choose_detailed`]。
#[cfg(test)]
pub fn choose(
    cands: &[Candidate],
    now: i64,
    exclude: &HashSet<i64>,
    in_job: &HashMap<i64, u32>,
    rng: &mut dyn FnMut(usize) -> usize,
) -> Option<(usize, Tier)> {
    choose_detailed(cands, now, exclude, in_job, rng).map(|c| (c.index, c.tier))
}

/// [`choose`] 的完整版：额外给出实际执行的冷却天数（降级到了哪一档）。
pub fn choose_detailed(
    cands: &[Candidate],
    now: i64,
    exclude: &HashSet<i64>,
    in_job: &HashMap<i64, u32>,
    rng: &mut dyn FnMut(usize) -> usize,
) -> Option<Choice> {
    let open: Vec<usize> = (0..cands.len())
        .filter(|&i| !exclude.contains(&cands[i].id))
        .collect();
    let unused: Vec<usize> = open
        .iter()
        .copied()
        .filter(|&i| !in_job.contains_key(&cands[i].id))
        .collect();
    let mut random = |pool: &[usize], tier: Tier, cooldown_days: i64| {
        let index = pool[rng(pool.len()) % pool.len()];
        Choice {
            index,
            tier,
            cooldown_days,
        }
    };
    let fresh: Vec<usize> = unused
        .iter()
        .copied()
        .filter(|&i| !cands[i].cooling(now))
        .collect();
    if let Some(&first) = fresh.first() {
        return Some(random(
            &fresh,
            Tier::Fresh,
            cands[first].cooldown_secs / DAY,
        ));
    }
    // 冷却降级：只取比配置更短的档，第一档凑得出图就在那一档里随机。
    for &days in COOLDOWN_LADDER {
        let pool: Vec<usize> = unused
            .iter()
            .copied()
            .filter(|&i| cands[i].cooldown_secs > days * DAY && cands[i].rested(now, days))
            .collect();
        if !pool.is_empty() {
            return Some(random(&pool, Tier::Cooling, days));
        }
    }
    let last = |i: usize| cands[i].last_used_at.unwrap_or(i64::MIN);
    let oldest = |pool: Vec<usize>,
                  key: &dyn Fn(usize) -> (u32, i64),
                  rng: &mut dyn FnMut(usize) -> usize| {
        let best = pool.iter().map(|&i| key(i)).min()?;
        let tied: Vec<usize> = pool.into_iter().filter(|&i| key(i) == best).collect();
        Some(tied[rng(tied.len()) % tied.len()])
    };
    let choice = |index: usize, tier: Tier| Choice {
        index,
        tier,
        cooldown_days: 0,
    };
    if !unused.is_empty() {
        return oldest(unused, &|i| (0, last(i)), rng).map(|i| choice(i, Tier::Cooling));
    }
    let used = |i: usize| in_job.get(&cands[i].id).copied().unwrap_or(0);
    oldest(open, &|i| (used(i), last(i)), rng).map(|i| choice(i, Tier::Repeat))
}

/// 自动屏蔽：原文件已不在磁盘上的图不参与随机抽取（抽中了也发不出去）。
fn usable(rows: Vec<repo::ImageRow>) -> Vec<repo::ImageRow> {
    rows.into_iter()
        .filter(|r| {
            let ok = Path::new(&r.file_path).is_file();
            if !ok {
                tracing::warn!(id = r.id, path = %r.file_path, "图库图片原文件不存在，跳过");
            }
            ok
        })
        .collect()
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
        limits: LibraryLimits::current(),
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
    if (0..=MAX_COOLDOWN_DAYS).contains(&days) {
        Ok(days)
    } else {
        Err(AppError::InvalidInput(format!(
            "冷却天数要在 0～{MAX_COOLDOWN_DAYS} 之间：{days}"
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
    // 几百上千张一次导入时，逐张串行会让 CPU 大半时间在等磁盘。这里同时铺开几张：
    // 解码仍受 `files::decode` 的全局许可封顶（2～3 个），多出来的并发只用来重叠
    // 读文件、算哈希与写库。同一批里内容相同的两张靠 `(style_id, content_hash)`
    // 唯一约束兜底（`import_one` 里撞约束就清掉刚写的文件并记为重复）。
    // `buffered` 保持回执与输入同序；进度按完成数报。
    let short_of = |path: &String| {
        Path::new(path)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| path.clone())
    };
    if let Some(first) = paths.first() {
        progress(0, total, &short_of(first), 0);
    }
    let (refs_dir, thumbs_dir) = (refs_dir.as_path(), thumbs_dir.as_path());
    // 先收成 Vec 再进 stream：闭包留在 `Map` 适配器里会让外层 future 的 Send 推导卡在
    // 高阶生命周期上（「FnOnce is not general enough」）。
    let jobs: Vec<_> = paths
        .iter()
        .map(|path| async move {
            let r = import_one(pool, refs_dir, thumbs_dir, style_id, category_id, path, now).await;
            (path, r)
        })
        .collect();
    let mut results = futures_util::stream::iter(jobs).buffered(IMPORT_PARALLEL);
    let mut done = 0i64;
    while let Some((path, result)) = results.next().await {
        match result {
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
        done += 1;
        progress(done, total, &short_of(path), report.failed.len() as i64);
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
    let rows = usable(repo::candidates(&mut tx, style_id, category_id).await?);
    let cands: Vec<Candidate> = rows.iter().map(candidate_of).collect();
    let fresh_count = cands.iter().filter(|c| !c.cooling(now)).count() as i64;
    let mut exclude = HashSet::new();
    let mut picks = Vec::new();
    while picks.len() < count {
        let Some(Choice {
            index: i,
            tier,
            cooldown_days,
        }) = choose_detailed(&cands, now, &exclude, &HashMap::new(), &mut *rng)
        else {
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
                    cooldown_days: Some(cooldown_days),
                },
            )
            .await?;
        }
        picks.push(LibraryPick {
            tier: tier.code().into(),
            cooldown_days,
            image: LibraryImageView::from_row(rows[i].clone(), now),
        });
    }
    tx.commit().await?;
    let reused = picks.iter().filter(|p| p.tier != "fresh").count();
    let floor = picks.iter().map(|p| p.cooldown_days).min().unwrap_or(0);
    let message = if picks.is_empty() {
        format!("{}/{} 没有启用的图片", style.name, category.name)
    } else if picks.len() < count {
        format!("库里只有 {} 张启用的图，少于要的 {count} 张", picks.len())
    } else if reused > 0 {
        format!(
            "{} 天冷却内可用图不够，{reused} 张按降级冷却抽取（最低 {floor} 天）",
            category.cooldown_days
        )
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
                cooldown_days: None,
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
    /// 实际执行的冷却天数；点名的图与旧检查点为空。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cooldown_days: Option<i64>,
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
            cooldown_days: None,
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
            let rows = usable(repo::candidates(&mut *conn, style.id, category.id).await?);
            let cands = rows.iter().map(candidate_of).collect();
            e.insert((rows, cands));
        }
        let Some((rows, cands)) = pools.get_mut(&key) else {
            continue;
        };
        let exclude = per_item.entry(req.item).or_default();
        let Some(Choice {
            index: i,
            tier,
            cooldown_days,
        }) = choose_detailed(cands, now, exclude, &in_job, &mut *rng)
        else {
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
            cooldown_days: Some(cooldown_days),
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
                cooldown_days: pick.cooldown_days,
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
                cooldown_days: u.cooldown_days,
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
    pub cooldown_days: Option<i64>,
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
            cooldown_days: u.cooldown_days,
        })
        .collect())
}

// ───────────────────────────── 素材包 ─────────────────────────────

/// 素材包里的一组：某分类抽几张。款式、产品缺省取整包的。
///
/// 与工单 `refs` 里的图库引用同一形状（多一个 `count`），同一份分类清单既能交给 GenDesk
/// 收录时逐条抽，也能在这里一次抽成包导出给外部。
#[derive(Debug, Clone, Deserialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackSlotSpec {
    pub category: String,
    #[serde(default)]
    pub count: Option<u32>,
    #[serde(default)]
    pub style: Option<String>,
    #[serde(default)]
    pub product: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackSpec {
    #[serde(default)]
    pub style: String,
    #[serde(default)]
    pub product: String,
    pub slots: Vec<PackSlotSpec>,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PackPick {
    /// 包内顺序，1 起（对应 Image 1、Image 2…）。
    pub slot: i64,
    pub tier: String,
    /// 分类配置的冷却天数。
    pub configured_cooldown_days: i64,
    /// 实际执行的冷却天数；小于配置即为降级。
    pub cooldown_days: i64,
    pub image: LibraryImageView,
    /// 导出时这张图在导出目录里的文件名。
    pub exported_file: Option<String>,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PackResult {
    /// 记账时写进台账 job_id，`gendesk library usage --job-id <packId>` 可回查。
    pub pack_id: String,
    pub marked_used: bool,
    pub picks: Vec<PackPick>,
    /// 每个分类一句：可用多少、是否降级到了几天。
    pub notes: Vec<String>,
    pub export_dir: Option<String>,
    pub manifest_path: Option<String>,
}

/// 导出目录里的清单文件名。
pub const PACK_MANIFEST: &str = "素材包.json";

fn new_pack_id(now: i64) -> String {
    let stamp = chrono::DateTime::from_timestamp(now, 0)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%Y%m%d-%H%M%S")
                .to_string()
        })
        .unwrap_or_else(|| now.to_string());
    format!("pack-{stamp}-{:04x}", os_rng(0x10000))
}

/// 按分类清单抽一个素材包：包内不重复，每类先用冷却期外的图，不够就沿阶梯降级。
///
/// `export_dir` 给了就把每张图（摆正/压缩过的上传副本，与 GenDesk 发给上游的同一份）
/// 拷进去并写 [`PACK_MANIFEST`]；**导出即记为使用**——文件离开 GenDesk 就无从得知何时被用。
/// 没导出时 `mark_used` 决定是否记账（默认只预览）。任一类凑不够不重复的图就整包报错，
/// 什么都不记、不拷。
pub async fn draw_pack(
    pool: &SqlitePool,
    spec: &PackSpec,
    mark_used: bool,
    export_dir: Option<&Path>,
    now: i64,
    rng: &mut (dyn FnMut(usize) -> usize + Send),
) -> AppResult<PackResult> {
    if spec.slots.is_empty() {
        return Err(AppError::InvalidInput("素材包至少要有一个分类".into()));
    }
    let mut wanted = Vec::new();
    for (n, slot) in spec.slots.iter().enumerate() {
        let count = slot.count.unwrap_or(1) as usize;
        if count == 0 {
            return Err(AppError::InvalidInput(format!(
                "第 {} 组（{}）count 要 ≥1",
                n + 1,
                slot.category
            )));
        }
        let lib_ref = LibraryRef {
            library_id: None,
            category: Some(slot.category.clone()),
            style: slot.style.clone(),
            product: slot.product.clone(),
        };
        let resolved = lib_ref
            .resolve(&spec.style, &spec.product)
            .map_err(AppError::InvalidInput)?;
        wanted.push((resolved, count));
    }
    let total: usize = wanted.iter().map(|(_, c)| c).sum();
    if total > MAX_PACK_IMAGES {
        return Err(AppError::InvalidInput(format!(
            "素材包一共 {total} 张，超过上限 {MAX_PACK_IMAGES}（与一条工单的参考图上限一致）"
        )));
    }
    let export = export_dir.map(Path::to_path_buf);
    if let Some(dir) = &export {
        if !dir.is_absolute() {
            return Err(AppError::InvalidInput(format!(
                "导出目录要用绝对路径：{}",
                dir.display()
            )));
        }
        if dir.join(PACK_MANIFEST).exists() {
            return Err(AppError::InvalidInput(format!(
                "{} 里已经有一个素材包，换一个空目录",
                dir.display()
            )));
        }
    }
    let mark = mark_used || export.is_some();
    let pack_id = new_pack_id(now);

    let mut tx = pool.begin().await?;
    let mut exclude = HashSet::new();
    let mut in_pack: HashMap<i64, u32> = HashMap::new();
    let mut pools: HashMap<(i64, i64), (Vec<repo::ImageRow>, Vec<Candidate>)> = HashMap::new();
    let mut picks: Vec<(PackPick, i64, bool)> = Vec::new();
    let mut notes = Vec::new();
    for (slot, count) in &wanted {
        let style = find_style(pool, &slot.style, &slot.product).await?;
        let category = find_category(pool, &slot.category).await?;
        let key = (style.id, category.id);
        if let std::collections::hash_map::Entry::Vacant(e) = pools.entry(key) {
            let rows = usable(repo::candidates(&mut tx, style.id, category.id).await?);
            let cands = rows.iter().map(candidate_of).collect();
            e.insert((rows, cands));
        }
        let Some((rows, cands)) = pools.get_mut(&key) else {
            continue;
        };
        let fresh = cands.iter().filter(|c| !c.cooling(now)).count();
        let mut floor = category.cooldown_days;
        for _ in 0..*count {
            let Some(c) = choose_detailed(cands, now, &exclude, &in_pack, &mut *rng) else {
                return Err(AppError::InvalidInput(format!(
                    "{} 只有 {} 张可用的图（已屏蔽停用、移出和文件丢失的），素材包要 {count} 张且不能重复",
                    slot.describe(),
                    cands.len()
                )));
            };
            if c.tier == Tier::Repeat {
                return Err(AppError::InvalidInput(format!(
                    "{} 的图不够这个素材包不重复地用",
                    slot.describe()
                )));
            }
            let id = cands[c.index].id;
            exclude.insert(id);
            *in_pack.entry(id).or_default() += 1;
            cands[c.index].last_used_at = Some(now);
            floor = floor.min(c.cooldown_days);
            let row = rows[c.index].clone();
            let ref_image_id = row.ref_image_id;
            picks.push((
                PackPick {
                    slot: picks.len() as i64 + 1,
                    tier: c.tier.code().into(),
                    configured_cooldown_days: category.cooldown_days,
                    cooldown_days: c.cooldown_days,
                    image: LibraryImageView::from_row(row, now),
                    exported_file: None,
                },
                ref_image_id,
                c.tier != Tier::Fresh,
            ));
        }
        notes.push(if category.cooldown_days == 0 {
            format!("{}：免冷却，{} 张候选", slot.describe(), cands.len())
        } else if floor < category.cooldown_days {
            format!(
                "{}：{} 天冷却外只有 {fresh} 张，降级到 {floor} 天冷却",
                slot.describe(),
                category.cooldown_days
            )
        } else {
            format!(
                "{}：{} 天冷却外有 {fresh} 张，未降级",
                slot.describe(),
                category.cooldown_days
            )
        });
    }

    let mut manifest_path = None;
    if let Some(dir) = &export {
        std::fs::create_dir_all(dir)?;
        let mut entries = Vec::new();
        for (pick, ref_image_id, _) in &mut picks {
            let src = repo::upload_source(&mut tx, *ref_image_id)
                .await?
                .ok_or_else(|| AppError::Internal(format!("图库 #{} 没有文件", pick.image.id)))?;
            let ext = Path::new(&src)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("jpg")
                .to_lowercase();
            let name = format!(
                "{:02}_{}_{}.{ext}",
                pick.slot,
                crate::files::sanitize_filename(&pick.image.category),
                pick.image.id
            );
            std::fs::copy(&src, dir.join(&name))
                .map_err(|e| AppError::Internal(format!("导出 {src} 失败：{e}")))?;
            entries.push(serde_json::json!({
                "slot": pick.slot,
                "file": name,
                "libraryId": pick.image.id,
                "style": pick.image.style,
                "product": pick.image.product,
                "category": pick.image.category,
                "tier": pick.tier,
                "configuredCooldownDays": pick.configured_cooldown_days,
                "cooldownDays": pick.cooldown_days,
                "sourceName": pick.image.name,
            }));
            pick.exported_file = Some(name);
        }
        let manifest = serde_json::json!({
            "packId": pack_id,
            "createdAt": now,
            "style": spec.style,
            "product": spec.product,
            "markedUsed": true,
            "note": "GenDesk 参考图素材包：文件已按拍摄方向摆正（与 GenDesk 发给生图接口的同一份），按 slot 顺序即 Image 1、Image 2…。导出时已记为使用；这些文件再投回 GenDesk 时当普通参考图文件用，不要改写成 libraryId，否则会重复记账。",
            "notes": notes,
            "images": entries,
        });
        let path = dir.join(PACK_MANIFEST);
        std::fs::write(&path, serde_json::to_vec_pretty(&manifest)?)?;
        manifest_path = Some(path.to_string_lossy().to_string());
    }
    if mark {
        for (pick, _, fallback) in &picks {
            repo::record_usage(
                &mut tx,
                &repo::Usage {
                    image_id: pick.image.id,
                    used_at: now,
                    source: "manual",
                    job_id: Some(&pack_id),
                    item_name: Some("素材包"),
                    slot: Some(pick.slot),
                    fallback: *fallback,
                    cooldown_days: Some(pick.cooldown_days),
                },
            )
            .await?;
        }
    }
    tx.commit().await?;
    Ok(PackResult {
        pack_id,
        marked_used: mark,
        picks: picks.into_iter().map(|(p, _, _)| p).collect(),
        notes,
        export_dir: export.map(|d| d.to_string_lossy().to_string()),
        manifest_path,
    })
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
            let rows = usable(repo::candidates(&mut conn, style.id, category.id).await?);
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
    fn cooldown_relaxes_down_the_ladder_one_step_at_a_time() {
        let now = 100 * DAY;
        let cands = vec![
            cand(1, Some(now - 12 * DAY), 15),
            cand(2, Some(now - 8 * DAY), 15),
            cand(3, Some(now - 4 * DAY), 15),
            cand(4, Some(now - 30 * 3600), 15),
            cand(5, Some(now - 3600), 15),
        ];
        let job = HashMap::new();
        let mut exclude = HashSet::new();
        // 15 天内全用过：10 → 7 → 3 → 1 天各放出一张，最后只剩一小时前用过的取最久没用。
        for (id, days) in [(1, 10), (2, 7), (3, 3), (4, 1), (5, 0)] {
            let c = choose_detailed(&cands, now, &exclude, &job, &mut |_| 0).unwrap();
            assert_eq!(
                (cands[c.index].id, c.tier, c.cooldown_days),
                (id, Tier::Cooling, days)
            );
            exclude.insert(id);
        }
    }

    #[test]
    fn a_relaxed_step_is_still_random_and_fresh_reports_the_configured_days() {
        let now = 100 * DAY;
        let cands = vec![
            cand(1, Some(now - 6 * DAY), 7),
            cand(2, Some(now - 6 * DAY - 3600), 7),
            cand(3, Some(now), 7),
        ];
        let none = HashSet::new();
        let job = HashMap::new();
        let seen: HashSet<i64> = (0..4)
            .map(|k| {
                let c = choose_detailed(&cands, now, &none, &job, &mut |n| k % n).unwrap();
                assert_eq!(c.cooldown_days, 5);
                cands[c.index].id
            })
            .collect();
        assert_eq!(
            seen,
            HashSet::from([1, 2]),
            "降级档里随机，不是固定取最久没用"
        );
        let fresh = vec![cand(9, None, 15)];
        let c = choose_detailed(&fresh, now, &none, &job, &mut |_| 0).unwrap();
        assert_eq!((c.tier, c.cooldown_days), (Tier::Fresh, 15));
        let exempt = vec![cand(9, Some(now), 0)];
        let c = choose_detailed(&exempt, now, &none, &job, &mut |_| 0).unwrap();
        assert_eq!(
            (c.tier, c.cooldown_days),
            (Tier::Fresh, 0),
            "免冷却永远可用"
        );
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

    /// Rust 里的默认值与上限要和 0051 的列定义一致，界面和 CLI 都从这里取。
    #[tokio::test]
    async fn cooldown_constants_match_the_schema() {
        let (pool, _dir) = test_pool().await;
        sqlx::query("INSERT INTO ref_library_categories(name, created_at) VALUES('默认值', 0)")
            .execute(&pool)
            .await
            .unwrap();
        let days: i64 = sqlx::query_scalar(
            "SELECT cooldown_days FROM ref_library_categories WHERE name='默认值'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(days, DEFAULT_COOLDOWN_DAYS);
        for (days, ok) in [(MAX_COOLDOWN_DAYS, true), (MAX_COOLDOWN_DAYS + 1, false)] {
            let inserted = sqlx::query(
                "INSERT INTO ref_library_categories(name, cooldown_days, created_at) VALUES(?, ?, 0)",
            )
            .bind(format!("上限{days}"))
            .bind(days)
            .execute(&pool)
            .await;
            assert_eq!(inserted.is_ok(), ok, "{days}");
            assert_eq!(check_cooldown(days).is_ok(), ok, "{days}");
        }
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

    /// 并发导入（`IMPORT_PARALLEL`）：同一批里内容相同的两张只入库一张，另一张记重复且不留孤儿文件；
    /// 回执与输入同序，进度最后一声报满。
    #[tokio::test]
    async fn parallel_import_keeps_order_and_dedupes_within_one_batch() {
        let (pool, dir) = test_pool().await;
        let style = save_style(&pool, None, "王一博", "", "").await.unwrap();
        let category = find_category(&pool, "正面图").await.unwrap().id;
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        let mut paths = Vec::new();
        for k in 0..9u8 {
            let p = src.join(format!("图{k}.png"));
            std::fs::write(&p, png(k)).unwrap();
            paths.push(p.to_string_lossy().to_string());
        }
        // 与第 0 张内容相同、名字不同：只能靠内容哈希认出来。
        let twin = src.join("图0-副本.png");
        std::fs::write(&twin, png(0)).unwrap();
        paths.push(twin.to_string_lossy().to_string());

        let dirs = DataDirs::new(dir.path().join("data"));
        let last = std::sync::Mutex::new((0i64, 0i64));
        let report = import_files(
            &pool,
            &dirs,
            style,
            category,
            &paths,
            &|done, total, _, _| {
                *last.lock().unwrap() = (done, total);
            },
        )
        .await
        .unwrap();

        assert_eq!(report.imported.len(), 9);
        assert_eq!(report.duplicates.len(), 1);
        assert!(report.failed.is_empty());
        let names: Vec<_> = report.imported.iter().map(|v| v.name.clone()).collect();
        let want: Vec<_> = (0..9).map(|k| format!("图{k}")).collect();
        assert_eq!(names, want, "回执须与输入同序");
        assert_eq!(*last.lock().unwrap(), (10, 10));
        let files = std::fs::read_dir(dirs.refs().join("library"))
            .unwrap()
            .count();
        assert_eq!(files, 9, "撞重复的那张写过的文件要清掉");
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
    async fn a_pack_degrades_per_category_exports_upright_copies_and_records_once() {
        let (pool, dir) = test_pool().await;
        let (_, front, ids) = seed(&pool, dir.path(), 4).await;
        let now = crate::db::now_unix();
        // 三张两天前用过，一张从没用过；正面图冷却 7 天。
        mark_used(&pool, &ids[..3], now - 2 * DAY).await.unwrap();
        // 原文件丢了的图自动屏蔽，不能被抽中。
        let gone = list_images(&pool, None, Some(front), false, now)
            .await
            .unwrap()
            .into_iter()
            .find(|v| v.id == ids[2])
            .unwrap();
        std::fs::remove_file(&gone.file_path).unwrap();
        let spec = PackSpec {
            style: "王一博".into(),
            product: String::new(),
            slots: vec![PackSlotSpec {
                category: "正面图".into(),
                count: Some(2),
                style: None,
                product: None,
            }],
        };
        let preview = draw_pack(&pool, &spec, false, None, now, &mut |_| 0)
            .await
            .unwrap();
        assert!(!preview.marked_used);
        let used: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ref_library_usage")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(used, 3, "预览不记账");

        let out = dir.path().join("导出");
        let pack = draw_pack(&pool, &spec, false, Some(&out), now, &mut |_| 0)
            .await
            .unwrap();
        let got: Vec<(i64, &str, i64)> = pack
            .picks
            .iter()
            .map(|p| (p.image.id, p.tier.as_str(), p.cooldown_days))
            .collect();
        assert_eq!(got, vec![(ids[3], "fresh", 7), (ids[0], "cooling", 2)]);
        assert!(pack.notes[0].contains("降级到 2 天"), "{:?}", pack.notes);
        assert!(pack.marked_used, "导出即记为使用");
        for p in &pack.picks {
            assert!(out.join(p.exported_file.as_ref().unwrap()).is_file());
        }
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(out.join(PACK_MANIFEST)).unwrap()).unwrap();
        assert_eq!(manifest["packId"], pack.pack_id.as_str());
        assert_eq!(manifest["images"][1]["cooldownDays"], 2);
        let rows: Vec<(i64, Option<i64>)> = sqlx::query_as(
            "SELECT image_id, cooldown_days FROM ref_library_usage WHERE job_id = ?1 ORDER BY slot",
        )
        .bind(&pack.pack_id)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(rows, vec![(ids[3], Some(7)), (ids[0], Some(2))]);
        // 同一目录不能再导一包。
        assert!(draw_pack(&pool, &spec, false, Some(&out), now, &mut |_| 0)
            .await
            .is_err());

        // 只剩 3 张能用（一张文件丢了），要 4 张不重复：整包报错，什么都不记。
        let greedy = PackSpec {
            slots: vec![PackSlotSpec {
                count: Some(4),
                ..spec.slots[0].clone()
            }],
            ..spec.clone()
        };
        let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ref_library_usage")
            .fetch_one(&pool)
            .await
            .unwrap();
        let err = draw_pack(&pool, &greedy, true, None, now, &mut |_| 0)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("只有 3 张"), "{err}");
        let after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ref_library_usage")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(before, after);
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
