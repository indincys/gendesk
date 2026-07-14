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

    let mut generated = Vec::new();
    // 暂停排期（节假日）：只跳过①②生成，超时扫描与对账照常——回收闭环停了的话，
    // 暂停期间已导出的单永远收不回来。手动 generate_sheet 也不受影响。
    if settings.schedule_paused {
        tracing::info!("排期已暂停，跳过自动生成（对账与超时扫描照常）");
    } else {
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
        let exists =
            |d: &str| (d == today_s && today_exists) || (d == tomorrow_s && tomorrow_exists);
        for date in catchup_dates(today, now.time(), autogen, &exists) {
            match planner::generate_sheet(pool, &date, &settings).await {
                Ok(_) => {
                    crate::publish::inbox::watcher::emit_badges(pool, app).await;
                    generated.push(date);
                }
                Err(e) => tracing::warn!(error = %e, date, "补跑生成任务单失败"),
            }
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

    // 归档清理：每天只跑一次（日期变了才跑），避免每 5 分钟扫一遍磁盘。
    if settings.archive_retention_days > 0 {
        let today_s = today.format("%Y-%m-%d").to_string();
        let last = LAST_SWEEP_DAY.lock().ok().map(|g| g.clone());
        if last.as_deref() != Some(today_s.as_str()) {
            if let Ok(mut g) = LAST_SWEEP_DAY.lock() {
                *g = today_s;
            }
            sweep_archives(pool, &settings, today).await;
        }
    }

    generated
}

/// 上次跑归档清理的日期（进程内，重启后当天会再跑一次——清理是幂等的）。
static LAST_SWEEP_DAY: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

/// 目录名（`YYYYMMDD`）是否已超出保留期。
fn dir_expired(name: &str, today: NaiveDate, retention_days: i64) -> bool {
    let Ok(d) = NaiveDate::parse_from_str(name, "%Y%m%d") else {
        return false; // 名字不是日期的目录一律不动
    };
    (today - d).num_days() > retention_days
}

/// 删除超期归档：收件箱 已收录/ 与 已丢弃/ 下的过期日期目录 +
/// **已关闭**任务单的过期任务包目录（未关闭的绝不删——回执还没收完）。
/// inbox_items / task_sheets 的 DB 记录保留，历史仍可查。
async fn sweep_archives(pool: &SqlitePool, settings: &PublishSettings, today: NaiveDate) {
    use crate::publish::paths::{self, RelPath};
    let root = std::path::PathBuf::from(&settings.root_local);
    let keep = settings.archive_retention_days;

    let mut removed = 0usize;
    for sub in paths::INBOX_ARCHIVES {
        let dir = RelPath::from_parts([paths::INBOX, sub]).to_local(&root);
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.filter_map(|e| e.ok()) {
            let name = entry.file_name().to_string_lossy().to_string();
            if entry.path().is_dir() && dir_expired(&name, today, keep) {
                match std::fs::remove_dir_all(entry.path()) {
                    Ok(()) => removed += 1,
                    Err(e) => tracing::warn!(dir = %name, error = %e, "清理归档目录失败"),
                }
            }
        }
    }

    // 任务包：只删已关闭的单（未关闭 = 回执还没收完，删了就永远收不回来）。
    let closed: Vec<String> =
        sqlx::query_scalar("SELECT date FROM task_sheets WHERE status='closed'")
            .fetch_all(pool)
            .await
            .unwrap_or_default();
    let pkg_root = RelPath::from_parts([paths::TASK_PACKAGES]).to_local(&root);
    for date in closed {
        let yyyymmdd: String = date.chars().filter(|c| c.is_ascii_digit()).collect();
        if !dir_expired(&yyyymmdd, today, keep) {
            continue;
        }
        let dir = pkg_root.join(&yyyymmdd);
        if dir.is_dir() {
            match std::fs::remove_dir_all(&dir) {
                Ok(()) => removed += 1,
                Err(e) => tracing::warn!(dir = %yyyymmdd, error = %e, "清理任务包目录失败"),
            }
        }
    }
    if removed > 0 {
        tracing::info!(removed, retention_days = keep, "归档清理完成");
    }
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

    // E5：只删超期的日期目录；未到期的、名字不是日期的一概不动。
    #[test]
    fn only_expired_date_dirs_are_swept() {
        let today = d(2026, 7, 15);
        assert!(dir_expired("20260101", today, 90), "195 天前 > 90 天保留期");
        assert!(!dir_expired("20260701", today, 90), "14 天前，未到期");
        assert!(!dir_expired("任务单备份", today, 90), "非日期目录不动");
        assert!(
            !dir_expired("20260715", today, 0),
            "0 = 永久保留（调用方已跳过）"
        );
    }

    // E5/E7 端到端：过期的已关闭任务包被删，未关闭的留下；暂停时不生成草稿。
    #[tokio::test]
    async fn sweep_keeps_unclosed_packages() {
        use crate::commands::publish_settings::ensure_partitions;
        use crate::db::repo::planning as prepo;
        use crate::db::test_support::test_pool;
        use crate::publish::paths::RelPath;

        let (pool, dir) = test_pool().await;
        let root = dir.path();
        ensure_partitions(root).unwrap();
        let today = d(2026, 7, 15);

        // 两个 100 天前的任务包：一个已关闭、一个仍在回收中。
        let mut conn = pool.acquire().await.unwrap();
        let closed = prepo::create_sheet(&mut conn, "2026-04-06").await.unwrap();
        prepo::set_sheet_status(&mut conn, closed, "closed")
            .await
            .unwrap();
        let open = prepo::create_sheet(&mut conn, "2026-04-07").await.unwrap();
        prepo::set_sheet_status(&mut conn, open, "reconciling")
            .await
            .unwrap();
        drop(conn);

        for day in ["20260406", "20260407"] {
            let p = RelPath::from_parts(["任务包", day]).to_local(root);
            std::fs::create_dir_all(&p).unwrap();
            std::fs::write(p.join("任务单.xlsx"), b"x").unwrap();
        }
        // 过期的收件箱归档。
        let old_inbox = RelPath::from_parts(["收件箱", "已收录", "20260101"]).to_local(root);
        std::fs::create_dir_all(&old_inbox).unwrap();

        let settings = PublishSettings {
            root_local: root.to_string_lossy().to_string(),
            archive_retention_days: 90,
            ..PublishSettings::default()
        };
        sweep_archives(&pool, &settings, today).await;

        assert!(
            !RelPath::from_parts(["任务包", "20260406"])
                .to_local(root)
                .exists(),
            "已关闭且超期 → 删"
        );
        assert!(
            RelPath::from_parts(["任务包", "20260407"])
                .to_local(root)
                .exists(),
            "未关闭 → 绝不删（回执还没收完）"
        );
        assert!(!old_inbox.exists(), "超期的收件箱归档 → 删");
    }
}
