//! 导出前逐平台校验纯函数。

use std::collections::HashSet;

use chrono::{Duration, NaiveDateTime};

use super::sheet_json::ExportTask;

fn chars(value: &str) -> usize {
    value.chars().count()
}

fn fail(task: &ExportTask, field: &str, message: &str) -> String {
    format!("{} · {}：{}", task.task_id, field, message)
}

pub fn validate_tasks(
    tasks: &[ExportTask],
    now: NaiveDateTime,
    missing_paths: &HashSet<String>,
) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    if tasks.is_empty() {
        errors.push("任务单 · tasks：至少需要一个平台任务".into());
    }
    for task in tasks {
        let platform = task.platform.as_str();
        let title_limit = match platform {
            "抖音" | "小红书" => Some(20),
            "视频号" => Some(22),
            "快手" => None,
            _ => {
                errors.push(fail(task, "platform", "未知平台"));
                continue;
            }
        };
        match (title_limit, task.title.as_deref()) {
            (Some(limit), Some(title)) if !title.is_empty() && chars(title) <= limit => {}
            (Some(limit), _) => {
                errors.push(fail(task, "title", &format!("必填且不能超过 {limit} 字")))
            }
            (None, None) => {}
            (None, Some(_)) => errors.push(fail(task, "title", "快手图文标题必须为 null")),
        }

        let description_limit = if platform == "快手" { 500 } else { 1000 };
        if (platform == "抖音" || platform == "快手") && task.description.trim().is_empty() {
            errors.push(fail(task, "description", "必填"));
        } else if chars(&task.description) > description_limit {
            errors.push(fail(
                task,
                "description",
                &format!("不能超过 {description_limit} 字"),
            ));
        }

        if task.image_paths.is_empty() || task.image_paths.len() > 18 {
            errors.push(fail(task, "imagePaths", "必须有 1 到 18 张图片"));
        }
        for path in &task.image_paths {
            if missing_paths.contains(path) {
                errors.push(fail(task, "imagePaths", &format!("文件不存在：{path}")));
            }
        }

        if platform == "抖音" {
            match &task.cover_path {
                Some(path) if !missing_paths.contains(path) => {}
                Some(path) => errors.push(fail(task, "coverPath", &format!("文件不存在：{path}"))),
                None => errors.push(fail(task, "coverPath", "抖音必须指定封面")),
            }
            match &task.douyin {
                Some(options) if !task.cart => {
                    if chars(&options.short_title) > 10 {
                        errors.push(fail(task, "douyin.shortTitle", "不能超过 10 字"));
                    }
                }
                Some(options) => {
                    if options.product_url.trim().is_empty()
                        || options.short_title.trim().is_empty()
                    {
                        errors.push(fail(task, "douyin", "挂车开启时商品链接与短标题必填"));
                    }
                    if chars(&options.short_title) > 10 {
                        errors.push(fail(task, "douyin.shortTitle", "不能超过 10 字"));
                    }
                }
                None => errors.push(fail(task, "douyin", "抖音任务必须包含平台设置")),
            }
        } else if task.cover_path.is_some() || task.douyin.is_some() {
            errors.push(fail(
                task,
                "platformOptions",
                "非抖音任务不得含 coverPath 或 douyin",
            ));
        }
        if matches!(platform, "小红书" | "快手") && task.music_keyword.is_some() {
            errors.push(fail(task, "musicKeyword", "该平台不得包含音乐关键词"));
        }
        if platform == "小红书" && task.xhs.is_none() {
            errors.push(fail(task, "xhs", "小红书任务必须包含平台设置"));
        }
        if platform == "视频号" && task.shipinhao.is_none() {
            errors.push(fail(task, "shipinhao", "视频号任务必须包含平台设置"));
        }
        if platform != "小红书" && task.xhs.is_some() {
            errors.push(fail(task, "xhs", "只允许出现在小红书任务"));
        }
        if platform != "视频号" && task.shipinhao.is_some() {
            errors.push(fail(task, "shipinhao", "只允许出现在视频号任务"));
        }

        match NaiveDateTime::parse_from_str(&task.scheduled_at, "%Y-%m-%d %H:%M") {
            Ok(at) if at >= now + Duration::hours(2) && at <= now + Duration::days(14) => {}
            Ok(_) => errors.push(fail(task, "scheduledAt", "必须落在导出时刻 +2h 到 +14d 内")),
            Err(_) => errors.push(fail(task, "scheduledAt", "格式必须为 yyyy-MM-dd HH:mm")),
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // 测试断言失败即测试失败
mod tests {
    use super::*;
    use crate::publish::sheet_json::{DouyinOptions, ExportTask};

    fn now() -> NaiveDateTime {
        NaiveDateTime::parse_from_str("2026-08-01 10:00", "%Y-%m-%d %H:%M").unwrap()
    }

    fn douyin() -> ExportTask {
        ExportTask {
            task_id: "T1".into(),
            content_id: "C1".into(),
            platform: "抖音".into(),
            mode: "图文".into(),
            scheduled_at: "2026-08-01 12:00".into(),
            title: Some("标题".into()),
            description: "正文".into(),
            topics: vec![],
            image_paths: vec!["/a.jpg".into()],
            cover_path: Some("/a.jpg".into()),
            music_keyword: None,
            cart: true,
            douyin: Some(DouyinOptions {
                product_url: "https://example.com".into(),
                short_title: "短标题".into(),
                visibility: "公开".into(),
                allow_save: false,
            }),
            xhs: None,
            shipinhao: None,
            note: String::new(),
        }
    }

    #[test]
    fn catches_title_and_time_with_task_id() {
        let mut task = douyin();
        task.title = Some("一二三四五六七八九十一二三四五六七八九十一".into());
        task.scheduled_at = "2026-08-01 11:00".into();
        let errors = validate_tasks(&[task], now(), &HashSet::new()).unwrap_err();
        assert!(errors.iter().any(|x| x.contains("T1 · title")));
        assert!(errors.iter().any(|x| x.contains("T1 · scheduledAt")));
    }

    #[test]
    fn shipinhao_rejects_23_character_title_and_one_hour_schedule() {
        let mut task = douyin();
        task.task_id = "T-视频号-01".into();
        task.platform = "视频号".into();
        task.title = Some("一二三四五六七八九十一二三四五六七八九十一二三".into());
        task.scheduled_at = "2026-08-01 11:00".into();
        task.cover_path = None;
        task.douyin = None;
        task.cart = false;
        task.shipinhao = Some(crate::publish::sheet_json::ShipinhaoOptions {
            location: "不显示位置".into(),
        });

        let errors = validate_tasks(&[task], now(), &HashSet::new()).unwrap_err();
        assert!(errors
            .iter()
            .any(|error| error.contains("T-视频号-01 · title")));
        assert!(errors
            .iter()
            .any(|error| error.contains("T-视频号-01 · scheduledAt")));
    }

    #[test]
    fn rejects_empty_task_sheet() {
        let errors = validate_tasks(&[], now(), &HashSet::new()).unwrap_err();
        assert_eq!(errors, vec!["任务单 · tasks：至少需要一个平台任务"]);
    }
}
