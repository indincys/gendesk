//! SKU 域命令（发布模块执行计划 4.1 skus 域）。
//!
//! 列表聚合一次出三池余量 + 最近发布 + 预警标记；预警阈值读发布设置。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;

use crate::commands::publish_settings;
use crate::db::repo::{inbox, ledger, skus as repo};
use crate::error::{AppError, AppResult};
use crate::publish::platform::Platform;
use crate::publish::{paths, sku_mapping};
use crate::state::AppState;

/// SKU 列表/详情视图。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SkuView {
    pub id: i64,
    pub code: String,
    pub style_name: String,
    pub product_name: String,
    pub tier: String,
    pub topics: Vec<String>,
    /// 平台覆盖（NULL=跟随全局矩阵）。
    pub platforms: Option<Vec<String>>,
    pub status: String,
    pub is_general: bool,
    pub note: String,
    /// 收件箱文件夹别名（空串=无别名）。
    pub folder_alias: String,
    pub material_count: i64,
    pub title_count: i64,
    pub body_count: i64,
    pub has_gallery: bool,
    pub last_published: Option<i64>,
    /// 各池预警（低于阈值）。
    pub warn_material: bool,
    pub warn_title: bool,
    pub warn_body: bool,
    /// 任一池预警。
    pub warn: bool,
}

/// 一条发布历史（读台账）。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct HistoryItem {
    pub date: String,
    pub platform: String,
    pub task_code: String,
    pub url: Option<String>,
    pub published_at: i64,
}

/// SKU 详情：档案视图 + 发布历史（池明细由 assets/texts 域命令单独取）。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SkuDetail {
    pub sku: SkuView,
    pub history: Vec<HistoryItem>,
}

/// 列表过滤条件。
#[derive(Debug, Clone, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SkuFilter {
    pub tier: Option<String>,
    pub warn_only: Option<bool>,
    pub status: Option<String>,
    pub query: Option<String>,
}

/// 新建 SKU 输入。
#[derive(Debug, Clone, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CreateSkuInput {
    pub code: String,
    pub style_name: String,
    pub product_name: Option<String>,
    pub tier: Option<String>,
    pub topics: Option<Vec<String>>,
    pub platforms: Option<Vec<String>>,
    pub note: Option<String>,
    /// 收件箱文件夹别名（可选，中文亦可）。
    pub folder_alias: Option<String>,
}

/// 编辑补丁。
#[derive(Debug, Clone, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SkuPatch {
    pub style_name: Option<String>,
    pub product_name: Option<String>,
    pub tier: Option<String>,
    pub topics: Option<Vec<String>>,
    /// `Some(None)` = 清除覆盖（跟随全局矩阵）；`Some(Some(..))` = 设置覆盖。
    pub platforms: Option<Option<Vec<String>>>,
    pub note: Option<String>,
    /// 收件箱文件夹别名（`Some("")`=清除别名）。
    pub folder_alias: Option<String>,
}

fn parse_topics(json: &str) -> Vec<String> {
    serde_json::from_str(json).unwrap_or_default()
}

fn parse_platforms(json: Option<&str>) -> Option<Vec<String>> {
    json.and_then(|s| serde_json::from_str(s).ok())
}

fn valid_tier(t: &str) -> bool {
    matches!(t, "hot" | "warm" | "cold")
}

/// 设置/清除文件夹别名（空串=清除）；非空别名若已被别的 SKU 占用则报错。
async fn apply_alias(db: &sqlx::SqlitePool, id: i64, alias: &str) -> AppResult<()> {
    let alias = alias.trim();
    if !alias.is_empty() {
        if let Some(existing) = repo::find_by_alias(db, alias).await? {
            if existing.id != id {
                return Err(AppError::InvalidInput(format!(
                    "文件夹别名已被 SKU {} 占用：{alias}",
                    existing.code
                )));
            }
        }
    }
    repo::set_alias(db, id, alias).await?;
    Ok(())
}

fn to_view(row: &repo::SkuAggRow, s: &publish_settings::PublishSettings) -> SkuView {
    let has_gallery = row.gallery_count > 0;
    let warn_material = row.material_count < s.warn_material;
    let warn_title = row.title_count < s.warn_title;
    // 正文预警仅对有图集包（需要图文）的 SKU 生效。
    let warn_body = has_gallery && row.body_count < s.warn_body;
    // 通用分组不参与排期，不做预警。
    let is_general = row.is_general != 0;
    let warn = !is_general && (warn_material || warn_title || warn_body);
    SkuView {
        id: row.id,
        code: row.code.clone(),
        style_name: row.style_name.clone(),
        product_name: row.product_name.clone(),
        tier: row.tier.clone(),
        topics: parse_topics(&row.topics_json),
        platforms: parse_platforms(row.platforms_json.as_deref()),
        status: row.status.clone(),
        is_general,
        note: row.note.clone(),
        folder_alias: row.folder_alias.clone(),
        material_count: row.material_count,
        title_count: row.title_count,
        body_count: row.body_count,
        has_gallery,
        last_published: row.last_published,
        warn_material: !is_general && warn_material,
        warn_title: !is_general && warn_title,
        warn_body: !is_general && warn_body,
        warn,
    }
}

/// 校验平台 code 列表（覆盖矩阵）；非法平台报错。
fn validate_platforms(platforms: &[String]) -> AppResult<()> {
    for p in platforms {
        if Platform::from_code(p).is_none() {
            return Err(AppError::InvalidInput(format!("未知平台：{p}")));
        }
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn list_skus(state: State<'_, AppState>, filter: SkuFilter) -> AppResult<Vec<SkuView>> {
    let settings = publish_settings::load(&state.db).await?;
    let rows = repo::list_agg(&state.db).await?;
    let q = filter.query.as_deref().map(str::to_lowercase);
    let views = rows
        .iter()
        .map(|r| to_view(r, &settings))
        .filter(|v| {
            if let Some(t) = &filter.tier {
                if !v.is_general && &v.tier != t {
                    return false;
                }
            }
            if let Some(st) = &filter.status {
                if &v.status != st {
                    return false;
                }
            }
            if filter.warn_only == Some(true) && !v.warn {
                return false;
            }
            if let Some(q) = &q {
                let hay = format!("{} {} {}", v.code, v.style_name, v.product_name).to_lowercase();
                if !hay.contains(q) {
                    return false;
                }
            }
            true
        })
        .collect();
    Ok(views)
}

#[tauri::command]
#[specta::specta]
pub async fn create_sku(state: State<'_, AppState>, input: CreateSkuInput) -> AppResult<i64> {
    let code = input.code.trim().to_string();
    if !paths::is_valid_sku_code(&code) {
        return Err(AppError::InvalidInput(format!(
            "SKU 编码只能包含字母、数字与 - _ .（无空格，不超过 {} 字符），\
             且不能是 . / .. 或 Windows 保留名（CON/PRN/AUX/NUL/COM1–9/LPT1–9）",
            paths::SKU_CODE_MAX
        )));
    }
    // NOCASE 查重：Windows 上 `sf-1` 与 `SF-1` 会争抢资产库同一个目录。
    if let Some(existing) = repo::find_by_code(&state.db, &code).await? {
        return Err(AppError::InvalidInput(format!(
            "SKU 编码已存在：{}（编码不区分大小写）",
            existing.code
        )));
    }
    let tier = input
        .tier
        .filter(|t| valid_tier(t))
        .unwrap_or_else(|| "warm".into());
    if let Some(ps) = &input.platforms {
        validate_platforms(ps)?;
    }
    let topics_json = serde_json::to_string(&input.topics.unwrap_or_default())?;
    let platforms_json = match input.platforms {
        Some(ps) => Some(serde_json::to_string(&ps)?),
        None => None,
    };
    let id = repo::insert(
        &state.db,
        &repo::NewSku {
            code,
            style_name: input.style_name.trim().to_string(),
            product_name: input.product_name.unwrap_or_default(),
            tier,
            topics_json,
            platforms_json,
            note: input.note.unwrap_or_default(),
        },
    )
    .await?;
    if let Some(alias) = &input.folder_alias {
        apply_alias(&state.db, id, alias).await?;
    }
    Ok(id)
}

#[tauri::command]
#[specta::specta]
pub async fn update_sku(state: State<'_, AppState>, id: i64, patch: SkuPatch) -> AppResult<()> {
    if let Some(t) = &patch.tier {
        if !valid_tier(t) {
            return Err(AppError::InvalidInput(format!("非法分层：{t}")));
        }
    }
    let topics_json = match &patch.topics {
        Some(ts) => Some(serde_json::to_string(ts)?),
        None => None,
    };
    // platforms: Some(None)=清除；Some(Some)=设置；None=不动
    let platforms_arg: Option<Option<String>> = match &patch.platforms {
        None => None,
        Some(None) => Some(None),
        Some(Some(ps)) => {
            validate_platforms(ps)?;
            Some(Some(serde_json::to_string(ps)?))
        }
    };
    repo::update_fields(
        &state.db,
        id,
        patch.style_name.as_deref(),
        patch.product_name.as_deref(),
        patch.tier.as_deref(),
        topics_json.as_deref(),
        platforms_arg.as_ref().map(|o| o.as_deref()),
        patch.note.as_deref(),
    )
    .await?;
    if let Some(alias) = &patch.folder_alias {
        apply_alias(&state.db, id, alias).await?;
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn set_sku_status(state: State<'_, AppState>, id: i64, status: String) -> AppResult<()> {
    if status != "active" && status != "paused" {
        return Err(AppError::InvalidInput(format!("非法状态：{status}")));
    }
    repo::set_status(&state.db, id, &status).await?;
    Ok(())
}

/// 发布模块导航徽章计数（资产库 = 待认领 + 预警；发布计划 = 待确认任务单 + 待核对）。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PublishBadges {
    /// 待认领 + 解析失败。
    pub unclaimed: i64,
    /// 余量预警 SKU 数。
    pub warn: i64,
    /// 待确认任务单数（P2 起）。
    pub pending_sheets: i64,
    /// 待核对数（P3 起）。
    pub pending_reconcile: i64,
}

/// 徽章计数（命令与 watcher 事件共用，单点避免漂移）。
pub async fn badge_counts(pool: &sqlx::SqlitePool) -> AppResult<PublishBadges> {
    let settings = publish_settings::load(pool).await?;
    let rows = repo::list_agg(pool).await?;
    let warn = rows.iter().filter(|r| to_view(r, &settings).warn).count() as i64;
    let unclaimed = inbox::count_pending(pool).await?;
    let pending_sheets: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM task_sheets WHERE status IN ('draft','confirmed')",
    )
    .fetch_one(pool)
    .await?;
    let pending_reconcile: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM publish_tasks WHERE status = 'suspect'")
            .fetch_one(pool)
            .await?;
    Ok(PublishBadges {
        unclaimed,
        warn,
        pending_sheets,
        pending_reconcile,
    })
}

#[tauri::command]
#[specta::specta]
pub async fn get_publish_badges(state: State<'_, AppState>) -> AppResult<PublishBadges> {
    badge_counts(&state.db).await
}

#[tauri::command]
#[specta::specta]
pub async fn get_sku_detail(state: State<'_, AppState>, id: i64) -> AppResult<SkuDetail> {
    let settings = publish_settings::load(&state.db).await?;
    let row = repo::get_agg(&state.db, id)
        .await?
        .ok_or_else(|| AppError::InvalidInput(format!("SKU 不存在：{id}")))?;
    let sku = to_view(&row, &settings);
    let history = ledger::history_by_sku(&state.db, id, 50, 0)
        .await?
        .into_iter()
        .map(|l| HistoryItem {
            date: l.date,
            platform: Platform::from_code(&l.platform)
                .map(|p| p.zh().to_string())
                .unwrap_or(l.platform),
            task_code: l.task_code,
            url: l.url,
            published_at: l.published_at,
        })
        .collect();
    Ok(SkuDetail { sku, history })
}

/// 批量映射导入结果（`dryRun` 时为预检，不落库）。
#[derive(Debug, Clone, Default, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MappingImportReport {
    /// 本次是否只预检不落库。
    pub dry_run: bool,
    /// 探测到的文件编码（xlsx 为 `XLSX`）。
    pub encoding: String,
    /// 是否识别到表头行（否则按位置 `编码,别名,话题` 解析）。
    pub had_header: bool,
    /// 可导入的数据行数。
    pub rows: i64,
    /// 将新建 / 已新建的 SKU 数。
    pub created: i64,
    /// 有字段变更的既有 SKU 数。
    pub updated: i64,
    /// 与库内完全一致、无需改动的行。
    pub unchanged: i64,
    pub alias_set: i64,
    pub topics_set: i64,
    /// 新建 SKU 的编码（预览用，最多 200 个）。
    pub created_codes: Vec<String>,
    /// 冲突：仅该格被跳过，行内其余字段照常导入。
    pub conflicts: Vec<String>,
    /// 无法导入的行，以及被忽略的单元格。
    pub errors: Vec<String>,
}

/// 一行的待写字段（`None` = 不动）。
#[derive(Debug, Default)]
struct UpdateFields {
    style_name: Option<String>,
    product_name: Option<String>,
    tier: Option<String>,
    topics_json: Option<String>,
    /// `Some(None)` = 清除平台覆盖；`Some(Some(..))` = 设置覆盖。
    platforms_json: Option<Option<String>>,
    note: Option<String>,
}

impl UpdateFields {
    fn is_empty(&self) -> bool {
        self.style_name.is_none()
            && self.product_name.is_none()
            && self.tier.is_none()
            && self.topics_json.is_none()
            && self.platforms_json.is_none()
            && self.note.is_none()
    }
}

/// 一行的执行计划（预检与落库共用，保证「预览说什么、执行就做什么」）。
enum RowPlan {
    Create {
        new: repo::NewSku,
        alias: Option<String>,
        status: Option<String>,
    },
    Update {
        id: i64,
        fields: UpdateFields,
        alias: Option<String>,
        status: Option<String>,
    },
}

/// 解析结果 + 库内现状 → 逐行计划 + 报告（纯读，不写库）。
async fn plan_mappings(
    db: &sqlx::SqlitePool,
    parsed: &sku_mapping::ParsedMapping,
) -> AppResult<(Vec<RowPlan>, MappingImportReport)> {
    let mut report = MappingImportReport {
        encoding: parsed.encoding.clone(),
        had_header: parsed.had_header,
        errors: parsed.errors.clone(),
        ..Default::default()
    };
    let existing = repo::list_agg(db).await?;
    // 别名归属与各 SKU 当前别名：随计划推进而更新，故「先改走 A 的别名、再把 A 给 B」不会误报冲突。
    // 别名 → 持有者编码（原样，供报错文案）；编码键（小写）→ 当前别名。
    let mut alias_owner: HashMap<String, String> = existing
        .iter()
        .filter(|r| !r.folder_alias.is_empty())
        .map(|r| (r.folder_alias.clone(), r.code.clone()))
        .collect();
    let mut alias_of: HashMap<String, String> = existing
        .iter()
        .map(|r| (r.code.to_ascii_lowercase(), r.folder_alias.clone()))
        .collect();
    // 编码键一律小写：库内编码大小写唯一（idx_skus_code_nocase），映射表里的
    // `sf-1` 必须命中库里的 `SF-1` 走更新，而不是新建出一个撞索引的行。
    let by_code: HashMap<String, &repo::SkuAggRow> = existing
        .iter()
        .map(|r| (r.code.to_ascii_lowercase(), r))
        .collect();

    let mut plans: Vec<RowPlan> = Vec::new();
    for row in &parsed.rows {
        let line = row.line;
        let cur = by_code.get(&row.code.to_ascii_lowercase()).copied();
        if cur.map(|c| c.is_general != 0) == Some(true) {
            report
                .errors
                .push(format!("第 {line} 行：内置「通用」分组不可通过映射表修改"));
            continue;
        }
        report.rows += 1;

        // 别名：占用冲突则只跳过这一格；与现值相同则不算变更。
        let code_key = row.code.to_ascii_lowercase();
        let mut alias_to_set: Option<String> = None;
        if let Some(a) = &row.alias {
            match alias_owner.get(a) {
                Some(owner) if !owner.eq_ignore_ascii_case(&row.code) => {
                    report.conflicts.push(format!(
                        "第 {line} 行：别名「{a}」已被 SKU {owner} 占用，本行别名未写入"
                    ));
                }
                _ => {
                    let same = alias_of.get(&code_key).map(String::as_str) == Some(a.as_str());
                    if !same {
                        if let Some(old) = alias_of.get(&code_key) {
                            alias_owner.remove(old);
                        }
                        alias_owner.insert(a.clone(), row.code.clone());
                        alias_of.insert(code_key.clone(), a.clone());
                        alias_to_set = Some(a.clone());
                        report.alias_set += 1;
                    }
                }
            }
        }

        match cur {
            // 编码不存在 → 新建（款式名缺省取别名，再缺省取编码；分层缺省温款）。
            None => {
                let style_name = row
                    .style_name
                    .clone()
                    .or_else(|| row.alias.clone())
                    .unwrap_or_else(|| row.code.clone());
                let topics = row.topics.clone().unwrap_or_default();
                if !topics.is_empty() {
                    report.topics_set += 1;
                }
                let platforms_json = match &row.platforms {
                    Some(Some(ps)) => Some(serde_json::to_string(ps)?),
                    _ => None,
                };
                report.created += 1;
                if report.created_codes.len() < 200 {
                    report.created_codes.push(row.code.clone());
                }
                plans.push(RowPlan::Create {
                    new: repo::NewSku {
                        code: row.code.clone(),
                        style_name,
                        product_name: row.product_name.clone().unwrap_or_default(),
                        tier: row.tier.clone().unwrap_or_else(|| "warm".into()),
                        topics_json: serde_json::to_string(&topics)?,
                        platforms_json,
                        note: row.note.clone().unwrap_or_default(),
                    },
                    alias: alias_to_set,
                    status: row.status.clone().filter(|s| s == "paused"),
                });
            }
            // 编码已存在 → 就地更新，只写「有值且与现值不同」的格子。
            Some(c) => {
                let mut f = UpdateFields::default();
                let diff = |new: &Option<String>, old: &str| -> Option<String> {
                    new.clone().filter(|v| v != old)
                };
                f.style_name = diff(&row.style_name, &c.style_name);
                f.product_name = diff(&row.product_name, &c.product_name);
                f.tier = diff(&row.tier, &c.tier);
                f.note = diff(&row.note, &c.note);
                if let Some(ts) = &row.topics {
                    if ts != &parse_topics(&c.topics_json) {
                        f.topics_json = Some(serde_json::to_string(ts)?);
                        report.topics_set += 1;
                    }
                }
                if let Some(target) = &row.platforms {
                    let old = parse_platforms(c.platforms_json.as_deref());
                    if target != &old {
                        f.platforms_json = Some(match target {
                            Some(ps) => Some(serde_json::to_string(ps)?),
                            None => None,
                        });
                    }
                }
                let status = row.status.clone().filter(|s| s != &c.status);
                let touched = !f.is_empty() || alias_to_set.is_some() || status.is_some();
                if touched {
                    report.updated += 1;
                } else {
                    report.unchanged += 1;
                }
                plans.push(RowPlan::Update {
                    id: c.id,
                    fields: f,
                    alias: alias_to_set,
                    status,
                });
            }
        }
    }
    Ok((plans, report))
}

/// 批量导入 SKU 映射表：**编码不存在则新建，存在则就地更新，空单元格一律不动**。
///
/// 接受 `.xlsx/.csv/.tsv/.txt`（UTF-8/GBK 自动探测）；表头可选，见 [`sku_mapping`]。
/// `dry_run=true` 只返回预检报告不落库——前端先预览、用户确认后再以 `false` 落库。
#[tauri::command]
#[specta::specta]
pub async fn import_sku_mappings(
    state: State<'_, AppState>,
    path: String,
    dry_run: bool,
) -> AppResult<MappingImportReport> {
    let parsed = sku_mapping::parse_mapping_file(std::path::Path::new(&path))?;
    let (plans, mut report) = plan_mappings(&state.db, &parsed).await?;
    report.dry_run = dry_run;
    if !dry_run {
        execute_plans(&state.db, plans).await?;
    }
    Ok(report)
}

/// 落库：按行序执行计划（别名换绑依赖行序，故不可并发）。
async fn execute_plans(db: &sqlx::SqlitePool, plans: Vec<RowPlan>) -> AppResult<()> {
    for plan in plans {
        match plan {
            RowPlan::Create { new, alias, status } => {
                let id = repo::insert(db, &new).await?;
                if let Some(a) = alias {
                    repo::set_alias(db, id, &a).await?;
                }
                if let Some(s) = status {
                    repo::set_status(db, id, &s).await?;
                }
            }
            RowPlan::Update {
                id,
                fields,
                alias,
                status,
            } => {
                if !fields.is_empty() {
                    repo::update_fields(
                        db,
                        id,
                        fields.style_name.as_deref(),
                        fields.product_name.as_deref(),
                        fields.tier.as_deref(),
                        fields.topics_json.as_deref(),
                        fields.platforms_json.as_ref().map(|o| o.as_deref()),
                        fields.note.as_deref(),
                    )
                    .await?;
                }
                if let Some(a) = alias {
                    repo::set_alias(db, id, &a).await?;
                }
                if let Some(s) = status {
                    repo::set_status(db, id, &s).await?;
                }
            }
        }
    }
    Ok(())
}

/// 选择映射表文件（xlsx / csv / tsv / txt）。
#[tauri::command]
#[specta::specta]
pub async fn pick_mapping_file(app: AppHandle) -> AppResult<Option<String>> {
    let picked = app
        .dialog()
        .file()
        .add_filter("映射表", &["xlsx", "xlsm", "csv", "tsv", "txt"])
        .blocking_pick_file();
    Ok(picked
        .and_then(|p| p.into_path().ok())
        .map(|p| p.to_string_lossy().to_string()))
}

/// 导出映射表模板 CSV（UTF-8 BOM，Excel 双击即用）。返回落盘路径；用户取消则 `None`。
#[tauri::command]
#[specta::specta]
pub async fn save_sku_mapping_template(app: AppHandle) -> AppResult<Option<String>> {
    let picked = app
        .dialog()
        .file()
        .set_file_name("SKU映射表模板.csv")
        .add_filter("CSV", &["csv"])
        .blocking_save_file();
    let Some(path) = picked.and_then(|p| p.into_path().ok()) else {
        return Ok(None);
    };
    std::fs::write(&path, sku_mapping::template_csv())?;
    Ok(Some(path.to_string_lossy().to_string()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即测试失败，是期望行为
mod tests {
    use super::*;
    use crate::db::test_support::test_pool;

    /// 跑一遍导入：先预检、再落库，并断言两者报告一致（预览即执行）。
    async fn import(
        pool: &sqlx::SqlitePool,
        csv: &str,
        dir: &std::path::Path,
    ) -> MappingImportReport {
        let path = dir.join("m.csv");
        std::fs::write(&path, csv).unwrap();
        let parsed = sku_mapping::parse_mapping_file(&path).unwrap();
        let (_, preview) = plan_mappings(pool, &parsed).await.unwrap();
        let (plans, report) = plan_mappings(pool, &parsed).await.unwrap();
        assert_eq!(
            (preview.created, preview.updated, preview.unchanged),
            (report.created, report.updated, report.unchanged),
            "预检与执行的计划必须一致"
        );
        execute_plans(pool, plans).await.unwrap();
        report
    }

    #[tokio::test]
    async fn creates_missing_skus_instead_of_skipping() {
        let (pool, dir) = test_pool().await;
        let csv = "SKU编码,款式名,文件夹别名,话题,分层\n\
                   NFC-W-01,敖瑞鹏01,A-敖瑞鹏-01,沙发 家居,热款\n\
                   NFC-W-02,,B-敖瑞鹏-02,,\n";
        let r = import(&pool, csv, dir.path()).await;
        assert_eq!((r.created, r.updated, r.rows), (2, 0, 2));
        assert_eq!(r.created_codes, ["NFC-W-01", "NFC-W-02"]);
        assert!(r.errors.is_empty() && r.conflicts.is_empty());

        let a = repo::find_by_code(&pool, "NFC-W-01")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(a.style_name, "敖瑞鹏01");
        assert_eq!(a.folder_alias, "A-敖瑞鹏-01");
        assert_eq!(a.tier, "hot");
        assert_eq!(parse_topics(&a.topics_json), ["沙发", "家居"]);
        // 款式名留空 → 取别名兜底；分层留空 → 温款。
        let b = repo::find_by_code(&pool, "NFC-W-02")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(b.style_name, "B-敖瑞鹏-02");
        assert_eq!(b.tier, "warm");
        assert_eq!(b.status, "active");
    }

    #[tokio::test]
    async fn empty_cells_never_overwrite_existing_values() {
        let (pool, dir) = test_pool().await;
        import(
            &pool,
            "SKU编码,款式名,文件夹别名,话题,备注\nNFC-W-01,原名,A-01,沙发,原备注\n",
            dir.path(),
        )
        .await;
        // 第二次只写话题，其余留空。
        let r = import(
            &pool,
            "SKU编码,款式名,文件夹别名,话题,备注\nNFC-W-01,,,客厅 家居,\n",
            dir.path(),
        )
        .await;
        assert_eq!((r.created, r.updated, r.topics_set), (0, 1, 1));
        let s = repo::find_by_code(&pool, "NFC-W-01")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(s.style_name, "原名", "留空不得清空款式名");
        assert_eq!(s.folder_alias, "A-01", "留空不得清空别名");
        assert_eq!(s.note, "原备注");
        assert_eq!(parse_topics(&s.topics_json), ["客厅", "家居"]);
    }

    #[tokio::test]
    async fn identical_rows_count_as_unchanged() {
        let (pool, dir) = test_pool().await;
        let csv = "SKU编码,款式名,文件夹别名,分层\nNFC-W-01,敖瑞鹏01,A-01,热款\n";
        import(&pool, csv, dir.path()).await;
        let r = import(&pool, csv, dir.path()).await;
        assert_eq!((r.created, r.updated, r.unchanged), (0, 0, 1));
        assert_eq!(r.alias_set, 0, "别名与现值相同不算改动");
    }

    #[tokio::test]
    async fn alias_taken_by_another_sku_is_a_conflict_rest_of_row_still_imports() {
        let (pool, dir) = test_pool().await;
        import(&pool, "SKU编码,文件夹别名\nNFC-W-01,A-01\n", dir.path()).await;
        let r = import(
            &pool,
            "SKU编码,文件夹别名,款式名\nNFC-W-02,A-01,新款\n",
            dir.path(),
        )
        .await;
        assert_eq!(r.conflicts.len(), 1);
        assert!(r.conflicts[0].contains("已被 SKU NFC-W-01 占用"));
        let two = repo::find_by_code(&pool, "NFC-W-02")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(two.folder_alias, "", "冲突的别名不写入");
        assert_eq!(two.style_name, "新款", "行内其余字段照常导入");
        let one = repo::find_by_code(&pool, "NFC-W-01")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(one.folder_alias, "A-01", "原持有者不受影响");
    }

    #[tokio::test]
    async fn alias_handover_within_one_file_is_not_a_false_conflict() {
        let (pool, dir) = test_pool().await;
        import(
            &pool,
            "SKU编码,文件夹别名\nNFC-W-01,A-01\nNFC-W-02,B-01\n",
            dir.path(),
        )
        .await;
        // 先把 A-01 让给 NFC-W-02（行序在前的 NFC-W-01 同时改走 C-01）。
        let r = import(
            &pool,
            "SKU编码,文件夹别名\nNFC-W-01,C-01\nNFC-W-02,A-01\n",
            dir.path(),
        )
        .await;
        assert!(r.conflicts.is_empty(), "换绑不是冲突：{:?}", r.conflicts);
        assert_eq!(r.alias_set, 2);
        let one = repo::find_by_code(&pool, "NFC-W-01")
            .await
            .unwrap()
            .unwrap();
        let two = repo::find_by_code(&pool, "NFC-W-02")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (one.folder_alias.as_str(), two.folder_alias.as_str()),
            ("C-01", "A-01")
        );
    }

    #[tokio::test]
    async fn dry_run_writes_nothing() {
        let (pool, dir) = test_pool().await;
        let path = dir.path().join("m.csv");
        std::fs::write(&path, "SKU编码,文件夹别名\nNFC-W-01,A-01\n").unwrap();
        let parsed = sku_mapping::parse_mapping_file(&path).unwrap();
        let (_plans, report) = plan_mappings(&pool, &parsed).await.unwrap();
        assert_eq!(report.created, 1);
        assert!(
            repo::find_by_code(&pool, "NFC-W-01")
                .await
                .unwrap()
                .is_none(),
            "预检不得落库"
        );
    }

    // A5：编码大小写唯一 —— 映射表里的 `sf-1` 必须命中库里的 `SF-1` 走更新，
    // 而不是新建出一个撞 idx_skus_code_nocase 唯一索引的行。
    #[tokio::test]
    async fn code_lookup_is_case_insensitive() {
        let (pool, dir) = test_pool().await;
        import(&pool, "SKU编码,款式名\nSF-1,原名\n", dir.path()).await;
        let r = import(&pool, "SKU编码,款式名\nsf-1,新名\n", dir.path()).await;
        assert_eq!((r.created, r.updated), (0, 1), "大小写不同视为同一个 SKU");
        let all = repo::list_agg(&pool).await.unwrap();
        assert_eq!(
            all.iter().filter(|s| s.is_general == 0).count(),
            1,
            "不得建出第二个 SKU"
        );
        let s = repo::find_by_code(&pool, "SF-1").await.unwrap().unwrap();
        assert_eq!(s.style_name, "新名");
    }

    #[tokio::test]
    async fn general_group_is_protected() {
        let (pool, dir) = test_pool().await;
        let id = repo::general_id(&pool).await.unwrap();
        let general = repo::get(&pool, id).await.unwrap().unwrap();
        let csv = format!("SKU编码,款式名\n{},改名\n", general.code);
        let r = import(&pool, &csv, dir.path()).await;
        assert_eq!(r.rows, 0);
        assert_eq!(r.errors.len(), 1);
        assert!(r.errors[0].contains("通用"));
        let after = repo::get(&pool, id).await.unwrap().unwrap();
        assert_eq!(after.style_name, general.style_name);
    }
}
