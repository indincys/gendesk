//! 洞察类命令：排期预演（F4）· 发布月历（F5）· 开屏晨报（F6）。
//!
//! 三者都是**只读**的：预演不落库、不选套装（改了频率就能立刻看到分布变化）；
//! 月历与晨报只是把既有数据换个角度拼出来。

use serde::Serialize;
use specta::Type;
use tauri::State;

use crate::commands::publish_settings;
use crate::db::repo::{accounts, inbox, planning, skus};
use crate::error::AppResult;
use crate::publish::planner::frequency::{due_skus, FreqRules, SkuFreq};
use crate::publish::platform::Platform;
use crate::state::AppState;

// ─────────────────────────────────────────────── F4 排期预演

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PreviewEntry {
    pub sku_id: i64,
    pub sku_code: String,
    pub style_name: String,
    pub tier: String,
    /// 该 SKU 当日会展开到的平台（中文）。
    pub platforms: Vec<String>,
    /// 展开行数（平台 × 该平台在用账号数，日限裁剪前）。
    pub rows: i64,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PreviewDay {
    pub date: String,
    pub entries: Vec<PreviewEntry>,
    pub total_rows: i64,
    /// 超出账号日限、会被裁掉的行数。
    pub trimmed: i64,
}

/// 未来 N 天排期预演（F4）。**不选套装、不落库**——只按频率 × 平台 × 账号推演分布，
/// 所以改了 warmWeekly / 平台矩阵 / 账号，点一下就能看到影响，不必真去生成任务单。
///
/// 因为不选套装，它也**不反映缺料**：一个没素材的 SKU 照样出现在预演里。
#[tauri::command]
#[specta::specta]
pub async fn preview_schedule(state: State<'_, AppState>, days: i64) -> AppResult<Vec<PreviewDay>> {
    let days = days.clamp(1, 14);
    let s = publish_settings::load(&state.db).await?;
    let rows = skus::list_agg(&state.db).await?;
    let sched: Vec<&skus::SkuAggRow> = rows
        .iter()
        .filter(|r| r.is_general == 0 && r.status == "active")
        .collect();
    let accts = accounts::list(&state.db).await?;
    let active: Vec<&crate::db::repo::accounts::AccountRow> =
        accts.iter().filter(|a| a.status == "active").collect();

    let rules = FreqRules {
        hot_daily: s.tier_rules.hot_daily,
        warm_weekly: s.tier_rules.warm_weekly,
        cold_weekly_rotate: s.tier_rules.cold_weekly_rotate,
    };
    let freq: Vec<SkuFreq> = sched
        .iter()
        .map(|r| SkuFreq {
            id: r.id,
            tier: r.tier.clone(),
        })
        .collect();

    let today = chrono::Local::now().date_naive();
    let mut out = Vec::new();
    for off in 0..days {
        let d = today + chrono::Duration::days(off);
        let date = d.format("%Y-%m-%d").to_string();
        let due = due_skus(&date, &freq, &rules);

        let mut entries = Vec::new();
        let mut per_account: std::collections::HashMap<i64, i64> = std::collections::HashMap::new();
        for r in sched.iter().filter(|r| due.contains(&r.id)) {
            let platforms = enabled_platforms(r, &s);
            let mut rows_n = 0i64;
            let mut zh: Vec<String> = Vec::new();
            for p in &platforms {
                let n = active.iter().filter(|a| &a.platform == p).count() as i64;
                if n == 0 {
                    continue; // 没账号的平台展不出行
                }
                rows_n += n;
                zh.push(
                    Platform::from_code(p)
                        .map(|x| x.zh().to_string())
                        .unwrap_or_else(|| p.clone()),
                );
                for a in active.iter().filter(|a| &a.platform == p) {
                    *per_account.entry(a.id).or_default() += 1;
                }
            }
            if rows_n == 0 {
                continue;
            }
            entries.push(PreviewEntry {
                sku_id: r.id,
                sku_code: r.code.clone(),
                style_name: r.style_name.clone(),
                tier: r.tier.clone(),
                platforms: zh,
                rows: rows_n,
            });
        }
        let total_rows: i64 = entries.iter().map(|e| e.rows).sum();
        // 日限裁剪量：每个账号超出上限的部分。
        let trimmed: i64 = per_account
            .iter()
            .map(|(aid, n)| {
                let limit = active
                    .iter()
                    .find(|a| a.id == *aid)
                    .map(|a| a.daily_limit.max(0))
                    .unwrap_or(0);
                (n - limit).max(0)
            })
            .sum();
        out.push(PreviewDay {
            date,
            entries,
            total_rows,
            trimmed,
        });
    }
    Ok(out)
}

fn enabled_platforms(r: &skus::SkuAggRow, s: &publish_settings::PublishSettings) -> Vec<String> {
    if let Some(json) = r.platforms_json.as_deref() {
        if let Ok(v) = serde_json::from_str::<Vec<String>>(json) {
            return v;
        }
    }
    let m = &s.platform_matrix;
    Platform::ALL
        .into_iter()
        .filter(|p| match p {
            Platform::Douyin => m.douyin,
            Platform::Xhs => m.xhs,
            Platform::Kuaishou => m.kuaishou,
            Platform::Shipinhao => m.shipinhao,
            Platform::Bilibili => m.bilibili,
        })
        .map(|p| p.code().to_string())
        .collect()
}

// ─────────────────────────────────────────────── F5 发布月历

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CalendarDay {
    pub date: String,
    /// 实发数（台账）。
    pub published: i64,
    /// 计划数（任务单）。
    pub planned: i64,
    pub failed: i64,
    pub sheet_id: Option<i64>,
    /// 当日涉及的 SKU 编码（去重，最多 6 个，供格子悬停展示）。
    pub skus: Vec<String>,
}

/// 某月的发布月历（F5）。`yyyy_mm` 形如 `2026-07`。
#[tauri::command]
#[specta::specta]
pub async fn calendar_month(
    state: State<'_, AppState>,
    yyyy_mm: String,
) -> AppResult<Vec<CalendarDay>> {
    let like = format!("{yyyy_mm}-%");

    // 一条 GROUP BY 取实发；一条取计划。
    let published = sqlx::query_as::<_, (String, i64)>(
        "SELECT date, COUNT(*) FROM usage_ledger WHERE date LIKE ?1 GROUP BY date",
    )
    .bind(&like)
    .fetch_all(&state.db)
    .await?;
    let planned = sqlx::query_as::<_, (String, i64, i64, i64)>(
        "SELECT s.date, s.id, COUNT(pt.id),
                SUM(CASE WHEN pt.status='failed' THEN 1 ELSE 0 END)
         FROM task_sheets s LEFT JOIN publish_tasks pt ON pt.sheet_id = s.id
         WHERE s.date LIKE ?1 GROUP BY s.date, s.id",
    )
    .bind(&like)
    .fetch_all(&state.db)
    .await?;
    let sku_rows = sqlx::query_as::<_, (String, String)>(
        "SELECT DISTINCT l.date, sk.code FROM usage_ledger l
         JOIN skus sk ON sk.id = l.sku_id WHERE l.date LIKE ?1",
    )
    .bind(&like)
    .fetch_all(&state.db)
    .await?;

    use std::collections::BTreeMap;
    fn day<'a>(m: &'a mut BTreeMap<String, CalendarDay>, d: &str) -> &'a mut CalendarDay {
        m.entry(d.to_string()).or_insert_with(|| CalendarDay {
            date: d.to_string(),
            published: 0,
            planned: 0,
            failed: 0,
            sheet_id: None,
            skus: Vec::new(),
        })
    }

    let mut days: BTreeMap<String, CalendarDay> = BTreeMap::new();
    for (date, n) in published {
        day(&mut days, &date).published = n;
    }
    for (date, sheet_id, n, failed) in planned {
        let e = day(&mut days, &date);
        e.planned = n;
        e.failed = failed;
        e.sheet_id = Some(sheet_id);
    }
    for (date, code) in sku_rows {
        let e = day(&mut days, &date);
        if e.skus.len() < 6 {
            e.skus.push(code);
        }
    }
    Ok(days.into_values().collect())
}

// ─────────────────────────────────────────────── F6 开屏晨报

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct BriefView {
    pub today: String,
    /// 昨日。
    pub yesterday_published: i64,
    pub yesterday_failed: i64,
    pub yesterday_success_rate: Option<i64>,
    /// 今日。
    pub today_planned: i64,
    pub today_suspect: i64,
    pub today_shortage: i64,
    pub today_sheet_id: Option<i64>,
    /// 待认领（收件箱）。
    pub unclaimed: i64,
    /// 跑道告警的 SKU 数（素材 ≤ 7 天见底）。
    pub runway_warn: i64,
}

/// 开屏晨报（F6）：昨天怎么样、今天要做什么、有什么卡住了。全部现有查询拼装。
#[tauri::command]
#[specta::specta]
pub async fn daily_brief(state: State<'_, AppState>) -> AppResult<BriefView> {
    use crate::publish::reconcile::ReportView;

    let today_d = chrono::Local::now().date_naive();
    let today = today_d.format("%Y-%m-%d").to_string();
    let yesterday = (today_d - chrono::Duration::days(1))
        .format("%Y-%m-%d")
        .to_string();

    let y_sheet = planning::get_sheet_by_date(&state.db, &yesterday).await?;
    let y_report = y_sheet
        .as_ref()
        .and_then(|s| s.report_json.as_deref())
        .and_then(|j| serde_json::from_str::<ReportView>(j).ok());
    let y_rows = match &y_sheet {
        Some(s) => planning::list_tasks_by_sheet(&state.db, s.id).await?,
        None => Vec::new(),
    };
    let y_count = |st: &str| y_rows.iter().filter(|r| r.status == st).count() as i64;

    let t_sheet = planning::get_sheet_by_date(&state.db, &today).await?;
    let t_rows = match &t_sheet {
        Some(s) => planning::list_tasks_by_sheet(&state.db, s.id).await?,
        None => Vec::new(),
    };
    let t_shortage = t_sheet
        .as_ref()
        .map(|s| {
            serde_json::from_str::<Vec<serde_json::Value>>(&s.shortage_json)
                .unwrap_or_default()
                .iter()
                .filter(|v| {
                    !matches!(
                        v.get("reason").and_then(|r| r.as_str()),
                        Some("timeout_backfill")
                    )
                })
                .count() as i64
        })
        .unwrap_or(0);

    // 跑道告警：素材 7 天内见底的 SKU（F3 依赖；list_skus 已算好）。
    let runway_warn = crate::commands::publish_skus::list_skus(
        state.clone(),
        crate::commands::publish_skus::SkuFilter {
            tier: None,
            warn_only: None,
            status: Some("active".into()),
            query: None,
        },
    )
    .await?
    .iter()
    .filter(|s| s.material_days.is_some_and(|d| d <= 7))
    .count() as i64;

    Ok(BriefView {
        today,
        yesterday_published: y_count("published"),
        yesterday_failed: y_count("failed"),
        yesterday_success_rate: y_report.map(|r| r.success_rate),
        today_planned: t_rows.len() as i64,
        today_suspect: t_rows.iter().filter(|r| r.status == "suspect").count() as i64,
        today_shortage: t_shortage,
        today_sheet_id: t_sheet.map(|s| s.id),
        unclaimed: inbox::count_pending(&state.db).await?,
        runway_warn,
    })
}
