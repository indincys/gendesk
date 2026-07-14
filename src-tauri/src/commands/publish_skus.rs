//! SKU 域命令（发布模块执行计划 4.1 skus 域）。
//!
//! 列表聚合一次出三池余量 + 最近发布 + 预警标记；预警阈值读发布设置。

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::State;

use crate::commands::publish_settings;
use crate::db::repo::{inbox, ledger, skus as repo};
use crate::error::{AppError, AppResult};
use crate::publish::paths;
use crate::publish::platform::Platform;
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
        return Err(AppError::InvalidInput(
            "SKU 编码只能包含字母、数字与 - _ .（无空格）".into(),
        ));
    }
    if repo::find_by_code(&state.db, &code).await?.is_some() {
        return Err(AppError::InvalidInput(format!("SKU 编码已存在：{code}")));
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

/// 批量映射导入结果。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MappingImportReport {
    /// 至少设置了别名或话题的 SKU 行数。
    pub updated: i64,
    pub alias_set: i64,
    pub topics_set: i64,
    /// 跳过/出错行的说明（编码不存在、别名冲突等）。
    pub skipped: Vec<String>,
}

/// 一行按 Tab 优先、否则逗号切列并去空白。
fn split_cols(line: &str) -> Vec<String> {
    let parts: Vec<&str> = if line.contains('\t') {
        line.split('\t').collect()
    } else {
        line.split(',').collect()
    };
    parts.iter().map(|s| s.trim().to_string()).collect()
}

/// 解析话题列：空白分隔、去 `#`、去重、最多 5 个。
fn parse_topic_field(s: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for tok in s.split_whitespace() {
        let t = tok.trim_start_matches('#').trim();
        if !t.is_empty() && !out.iter().any(|x| x == t) {
            out.push(t.to_string());
        }
        if out.len() >= 5 {
            break;
        }
    }
    out
}

/// 批量导入 SKU 映射：每行 `编码[<Tab/逗号>别名][<Tab/逗号>话题]`。
/// 别名一对一（唯一），话题为显式设置/替换（区别于收件箱的「绝不覆盖」）。SKU 需已存在。
#[tauri::command]
#[specta::specta]
pub async fn import_sku_mappings(
    state: State<'_, AppState>,
    path: String,
) -> AppResult<MappingImportReport> {
    let content = std::fs::read(&path)?;
    let text = String::from_utf8_lossy(&content);
    let mut report = MappingImportReport {
        updated: 0,
        alias_set: 0,
        topics_set: 0,
        skipped: Vec::new(),
    };
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim().trim_start_matches('\u{feff}');
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        let cols = split_cols(line);
        let code = cols.first().map(String::as_str).unwrap_or("").trim();
        if code.is_empty() {
            continue;
        }
        let Some(sku) = repo::find_by_code(&state.db, code).await? else {
            report
                .skipped
                .push(format!("第 {} 行：SKU 编码不存在「{code}」", i + 1));
            continue;
        };
        let alias = cols.get(1).map(String::as_str).unwrap_or("").trim();
        let topics_field = cols.get(2).map(String::as_str).unwrap_or("").trim();
        let mut touched = false;
        // 别名（唯一性冲突则跳过别名、仍尝试话题）。
        if !alias.is_empty() {
            match repo::find_by_alias(&state.db, alias).await? {
                Some(other) if other.id != sku.id => {
                    report.skipped.push(format!(
                        "第 {} 行：别名「{alias}」已被 SKU {} 占用",
                        i + 1,
                        other.code
                    ));
                }
                _ => {
                    repo::set_alias(&state.db, sku.id, alias).await?;
                    report.alias_set += 1;
                    touched = true;
                }
            }
        }
        // 话题（显式设置/替换）。
        if !topics_field.is_empty() {
            let topics = parse_topic_field(topics_field);
            let json = serde_json::to_string(&topics)?;
            repo::set_topics(&state.db, sku.id, &json).await?;
            report.topics_set += 1;
            touched = true;
        }
        if touched {
            report.updated += 1;
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_cols_tab_then_comma() {
        assert_eq!(
            split_cols("NFC-W-01\tA-敖瑞鹏-01\t#a #b"),
            ["NFC-W-01", "A-敖瑞鹏-01", "#a #b"]
        );
        assert_eq!(
            split_cols("NFC-W-01, A-敖瑞鹏-01 , 沙发 家居"),
            ["NFC-W-01", "A-敖瑞鹏-01", "沙发 家居"]
        );
        assert_eq!(split_cols("NFC-W-01"), ["NFC-W-01"]);
    }

    #[test]
    fn parse_topic_field_strips_hash_dedupes_caps_5() {
        assert_eq!(
            parse_topic_field("#沙发 #家居 沙发 #新品"),
            ["沙发", "家居", "新品"]
        );
        assert_eq!(parse_topic_field("a b c d e f g").len(), 5);
        assert!(parse_topic_field("   ").is_empty());
    }
}
