//! 每日自动组稿：到设置时刻后，按配置的 same/next 目标日各生成一次。

use std::sync::Arc;
use std::time::Duration;

use chrono::{Duration as ChronoDuration, Local};
use sqlx::SqlitePool;
use tauri::AppHandle;

pub fn spawn(pool: SqlitePool, app: AppHandle) {
    let last_run = Arc::new(tokio::sync::Mutex::new(String::new()));
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            let settings = match crate::commands::publish_settings::load(&pool).await {
                Ok(value) => value,
                Err(err) => {
                    tracing::warn!(error=%err, "读取自动组稿设置失败");
                    continue;
                }
            };
            if settings.schedule_paused {
                continue;
            }
            let now = Local::now();
            let Some(autogen_time) = crate::publish::schedule::parse_hhmm(&settings.autogen_time)
            else {
                tracing::warn!(value=%settings.autogen_time, "自动组稿时间非法");
                continue;
            };
            if now.time() < autogen_time {
                continue;
            }
            let run_key = now.format("%Y-%m-%d").to_string();
            let mut guard = last_run.lock().await;
            if *guard == run_key {
                continue;
            }
            let same = now.date_naive().format("%Y-%m-%d").to_string();
            let next = (now.date_naive() + ChronoDuration::days(1))
                .format("%Y-%m-%d")
                .to_string();
            for (date, target) in [(same, "same"), (next, "next")] {
                match crate::commands::publish_sheets::generate_for_target_day(
                    &pool,
                    &date,
                    now.naive_local(),
                    target,
                )
                .await
                {
                    Ok(_) => {}
                    Err(crate::error::AppError::InvalidInput(message))
                        if message == "没有启用的任务单配置" => {}
                    Err(err) => tracing::warn!(target_day=target,error=%err,"自动组稿失败"),
                }
            }
            *guard = run_key;
            crate::publish::inbox::watcher::emit_badges(&pool, &app).await;
        }
    });
}
