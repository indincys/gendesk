//! 应用内定时 + 启动补跑（发布模块执行计划 §5.1 ticker / 前置事实 17）。
//!
//! 启动补跑三查：① 今日任务单不存在→生成今日；② 已过每日生成时间且明日任务单不存在→生成明日；
//! ③ 超时扫描（P3 接入）。运行期每 5 分钟跑一轮同一补跑逻辑（对时钟跳变/休眠健壮）。
//! 「到点触发」用注入的 now 决策，纯逻辑可测（无需真实时钟）。

use chrono::{Duration, NaiveDate, NaiveTime, Timelike};
use sqlx::SqlitePool;
use tauri::AppHandle;

use crate::commands::publish_settings::{self, PublishSettings};
use crate::db::repo::planning;
use crate::publish::planner;

/// 解析 `HH:MM` → (时, 分)；非法回退 22:00。
fn parse_hm(s: &str) -> (u32, u32) {
    s.split_once(':')
        .and_then(|(h, m)| Some((h.trim().parse().ok()?, m.trim().parse().ok()?)))
        .filter(|(h, m): &(u32, u32)| *h < 24 && *m < 60)
        .unwrap_or((22, 0))
}

/// 现在是否已过每日生成时刻。
fn past_autogen(now: NaiveTime, autogen: (u32, u32)) -> bool {
    now.hour() * 60 + now.minute() >= autogen.0 * 60 + autogen.1
}

/// 补跑决策：返回需要生成草稿的日期（YYYY-MM-DD），不含已存在的。纯逻辑。
/// `today` / `now_time` 注入以便测试。`exists` 判断某日任务单是否已存在。
pub fn catchup_dates(
    today: NaiveDate,
    now_time: NaiveTime,
    autogen: (u32, u32),
    exists: &dyn Fn(&str) -> bool,
) -> Vec<String> {
    let mut out = Vec::new();
    let today_s = today.format("%Y-%m-%d").to_string();
    // ① 今日缺 → 生成今日。
    if !exists(&today_s) {
        out.push(today_s);
    }
    // ② 已过生成时刻且明日缺 → 生成明日。
    if past_autogen(now_time, autogen) {
        let tomorrow_s = (today + Duration::days(1)).format("%Y-%m-%d").to_string();
        if !exists(&tomorrow_s) {
            out.push(tomorrow_s);
        }
    }
    out
}

/// 执行一轮补跑（生成缺失的今日/明日草稿）。返回实际生成的日期。
pub async fn run_catchup(pool: &SqlitePool, app: &AppHandle) -> Vec<String> {
    let settings: PublishSettings = match publish_settings::load(pool).await {
        Ok(s) if !s.root_local.is_empty() => s,
        _ => return Vec::new(),
    };
    let now = chrono::Local::now();
    let today = now.date_naive();
    let autogen = parse_hm(&settings.autogen_time);

    // 预取存在性（同步闭包不能 await，故先查两日）。
    let today_s = today.format("%Y-%m-%d").to_string();
    let tomorrow_s = (today + Duration::days(1)).format("%Y-%m-%d").to_string();
    let today_exists = planning::get_sheet_by_date(pool, &today_s)
        .await
        .ok()
        .flatten()
        .is_some();
    let tomorrow_exists = planning::get_sheet_by_date(pool, &tomorrow_s)
        .await
        .ok()
        .flatten()
        .is_some();
    let exists = |d: &str| (d == today_s && today_exists) || (d == tomorrow_s && tomorrow_exists);
    let dates = catchup_dates(today, now.time(), autogen, &exists);

    let mut generated = Vec::new();
    for date in dates {
        match planner::generate_sheet(pool, &date, &settings).await {
            Ok(sheet_id) => {
                crate::publish::inbox::watcher::emit_badges(pool, app).await;
                let _ = sheet_id;
                generated.push(date);
            }
            Err(e) => tracing::warn!(error = %e, date, "补跑生成任务单失败"),
        }
    }

    // ③ 超时扫描 → 疑似已发（前置事实 17）。
    match crate::publish::reconcile::timeout_scan(
        pool,
        now.timestamp(),
        settings.receipt_timeout_hours,
    )
    .await
    {
        Ok(n) if n > 0 => crate::publish::inbox::watcher::emit_badges(pool, app).await,
        Ok(_) => {}
        Err(e) => tracing::warn!(error = %e, "超时扫描失败"),
    }
    // 顺带跑一轮对账（回执可能已回写但未触发 watcher）。
    crate::commands::publish_reconcile::reconcile_run(pool, app).await;

    generated
}

/// 启动定时循环：立即补跑一次，此后每 5 分钟一轮（P3 会在此加超时扫描）。
/// 用 tauri::async_runtime::spawn 委托 Tauri 全局运行时，可从 setup（非 Tokio 运行时上下文）安全调用。
pub fn spawn(pool: SqlitePool, app: AppHandle) -> tauri::async_runtime::JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        loop {
            run_catchup(&pool, &app).await;
            tokio::time::sleep(std::time::Duration::from_secs(300)).await;
        }
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即测试失败，是期望行为
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }
    fn t(h: u32, m: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(h, m, 0).unwrap()
    }

    #[test]
    fn generates_today_when_missing() {
        let none = |_: &str| false;
        let out = catchup_dates(d(2026, 7, 15), t(10, 0), (22, 0), &none);
        // 未到 22:00 → 只补今日。
        assert_eq!(out, vec!["2026-07-15"]);
    }

    #[test]
    fn generates_tomorrow_after_autogen() {
        let none = |_: &str| false;
        let out = catchup_dates(d(2026, 7, 15), t(22, 30), (22, 0), &none);
        assert_eq!(out, vec!["2026-07-15", "2026-07-16"]);
    }

    #[test]
    fn no_duplicate_when_exists() {
        // 今日已存在，明日未到点 → 不生成。
        let today_only = |x: &str| x == "2026-07-15";
        let out = catchup_dates(d(2026, 7, 15), t(10, 0), (22, 0), &today_only);
        assert!(out.is_empty());
    }

    #[test]
    fn tomorrow_not_generated_before_autogen() {
        let today_exists = |x: &str| x == "2026-07-15";
        let out = catchup_dates(d(2026, 7, 15), t(21, 59), (22, 0), &today_exists);
        assert!(out.is_empty(), "未到生成时刻不排明日");
    }

    #[test]
    fn parse_hm_fallback() {
        assert_eq!(parse_hm("22:00"), (22, 0));
        assert_eq!(parse_hm("07:30"), (7, 30));
        assert_eq!(parse_hm("bad"), (22, 0));
        assert_eq!(parse_hm("99:99"), (22, 0));
    }
}
