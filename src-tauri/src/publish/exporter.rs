//! 任务包导出（发布模块执行计划 §5.1 exporter / 需求 §4.6）。
//!
//! 素材按 SKU 复制（每 SKU 一份）+ body.txt + 执行说明.md + 任务单.xlsx + READY.txt（最后写）。
//! 第 11/12/14 列按执行机根路径 + 风格拼接绝对路径（导出是唯一转换点）。

use std::path::Path;

use serde::Serialize;
use specta::Type;
use sqlx::SqlitePool;

use crate::commands::publish_settings::PublishSettings;
use crate::db::repo::planning;
use crate::error::{AppError, AppResult};
use crate::publish::paths::{self, PathStyle, RelPath};
use crate::publish::platform::Platform;
use crate::publish::xlsx::writer::{write_sheet, XlsxRow};

const EXEC_GUIDE_TEMPLATE: &str = include_str!("exec_guide.md");

/// 导出结果。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ExportResult {
    /// 包目录相对路径（任务包/YYYYMMDD）。
    pub pkg_dir_rel: String,
    pub row_count: i64,
    pub sku_count: i64,
    pub file_count: i64,
    /// 是否有超 Windows 长度上限的拼接路径（告警）。
    pub long_path_warn: bool,
}

fn content_kind_zh(kind: &str) -> &'static str {
    if kind == "gallery" {
        "图文"
    } else {
        "视频"
    }
}

fn pack_file_names(files_json: &str) -> Vec<String> {
    serde_json::from_str::<Vec<serde_json::Value>>(files_json)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| v.get("name").and_then(|x| x.as_str()).map(str::to_string))
        .collect()
}

/// 导出某任务单为任务包。sheet 必须为 confirmed 或 exported（重导出=整包覆盖）。
pub async fn export_package(
    pool: &SqlitePool,
    sheet_id: i64,
    s: &PublishSettings,
) -> AppResult<ExportResult> {
    let sheet = planning::get_sheet(pool, sheet_id)
        .await?
        .ok_or_else(|| AppError::InvalidInput("任务单不存在".into()))?;
    if sheet.status != "confirmed" && sheet.status != "exported" {
        return Err(AppError::InvalidInput(
            "只有已确认 / 已导出的任务单可导出".into(),
        ));
    }
    if s.root_local.is_empty() {
        return Err(AppError::InvalidInput("尚未配置本机根目录".into()));
    }
    let local_root = Path::new(&s.root_local);
    let exec_root = if s.root_exec.is_empty() {
        &s.root_local
    } else {
        &s.root_exec
    };
    let style = PathStyle::from_str_or_default(&s.path_style);

    let yyyymmdd: String = sheet.date.chars().filter(|c| c.is_ascii_digit()).collect();
    let pkg_rel = RelPath::from_parts([paths::TASK_PACKAGES, &yyyymmdd]);
    let pkg_abs = pkg_rel.to_local(local_root);

    // 重导出：先删 READY.txt（其他文件整包覆盖）。
    let ready_path = pkg_rel.join(paths::READY).to_local(local_root);
    let _ = std::fs::remove_file(&ready_path);
    std::fs::create_dir_all(&pkg_abs)?;
    std::fs::create_dir_all(pkg_rel.join(paths::RECEIPTS_DIR).to_local(local_root))?;

    let rows = planning::sheet_rows(pool, sheet_id).await?;

    // 复制素材（每 distinct SKU 一份）。
    let mut copied_skus = std::collections::HashSet::new();
    let mut file_count = 0i64;
    let mut long_path_warn = false;
    for r in &rows {
        if !copied_skus.insert(r.sku_id) {
            continue;
        }
        let dest_rel = pkg_rel.join(paths::MATERIALS_DIR).join(&r.sku_code);
        let dest_abs = dest_rel.to_local(local_root);
        std::fs::create_dir_all(&dest_abs)?;
        // 复制包内文件（img_NN / video / cover）。
        let src_dir = RelPath::new(&r.dir_rel).to_local(local_root);
        for name in pack_file_names(&r.files_json) {
            let src = src_dir.join(&name);
            if src.exists() {
                std::fs::copy(&src, dest_abs.join(&name))?;
                file_count += 1;
            }
        }
        // 图文正文物化 body.txt。
        if r.pack_kind == "gallery" {
            if let Some(body) = &r.body_text {
                std::fs::write(dest_abs.join(paths::BODY_TXT), body)?;
                file_count += 1;
            }
        }
        // 长度告警。
        let exec_path = paths::exec_join(exec_root, &dest_rel, style);
        if paths::exceeds_path_limit(&exec_path) {
            long_path_warn = true;
        }
    }

    // 构建 xlsx 行。
    let mut xrows = Vec::with_capacity(rows.len());
    for r in &rows {
        let mat_rel = pkg_rel.join(paths::MATERIALS_DIR).join(&r.sku_code);
        let material_path = paths::exec_join(exec_root, &mat_rel, style);
        let names = pack_file_names(&r.files_json);
        let media_names: Vec<&String> = names.iter().filter(|n| !n.starts_with("cover.")).collect();
        let media_filename = if r.pack_kind == "gallery" {
            match (media_names.first(), media_names.last()) {
                (Some(a), Some(b)) if media_names.len() > 1 => format!("{a}…{b}"),
                (Some(a), _) => (*a).clone(),
                _ => String::new(),
            }
        } else {
            media_names
                .first()
                .map(|s| (*s).clone())
                .unwrap_or_default()
        };
        let cover_name = names.iter().find(|n| n.starts_with("cover."));
        let cover_path = cover_name
            .map(|c| paths::exec_join(exec_root, &mat_rel.join(c), style))
            .unwrap_or_default();
        let body_path = if r.pack_kind == "gallery" && r.body_text.is_some() {
            paths::exec_join(exec_root, &mat_rel.join(paths::BODY_TXT), style)
        } else {
            String::new()
        };
        let topics_vec: Vec<String> = serde_json::from_str(&r.topics_json).unwrap_or_default();
        let mut topics: [String; 5] = Default::default();
        for (i, t) in topics_vec.into_iter().take(5).enumerate() {
            topics[i] = t;
        }
        let platform_zh = Platform::from_code(&r.platform)
            .map(|p| p.zh().to_string())
            .unwrap_or_else(|| r.platform.clone());

        xrows.push(XlsxRow {
            task_id: r.task_code.clone(),
            task_date: sheet.date.clone(),
            planned_time: r.planned_time.clone().unwrap_or_default(),
            platform_zh,
            account_name: r.account_name.clone(),
            style_name: r.style_name.clone(),
            sku_code: r.sku_code.clone(),
            product_name: r.product_name.clone(),
            content_kind_zh: content_kind_zh(&r.content_kind).to_string(),
            media_filename,
            material_path,
            cover_path,
            title: r.title_text.clone(),
            body_path,
            topics,
            status_zh: "待执行".to_string(),
            rpa_info: String::new(),
            screenshot: String::new(),
        });
    }

    // 写 xlsx + 执行说明.md。
    write_sheet(&pkg_rel.join(paths::TASK_XLSX).to_local(local_root), &xrows)?;
    file_count += 1;
    std::fs::write(
        pkg_rel.join(paths::EXEC_GUIDE).to_local(local_root),
        EXEC_GUIDE_TEMPLATE,
    )?;
    file_count += 1;

    // 最后写 READY.txt（就绪标志，mtime 最新）。
    std::fs::write(
        &ready_path,
        format!("READY {}\n{} 行任务\n", sheet.date, rows.len()),
    )?;

    // 置任务单为已导出。
    let mut conn = pool.acquire().await?;
    planning::set_sheet_status(&mut conn, sheet_id, "exported").await?;

    Ok(ExportResult {
        pkg_dir_rel: pkg_rel.as_str().to_string(),
        row_count: rows.len() as i64,
        sku_count: copied_skus.len() as i64,
        file_count,
        long_path_warn,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即测试失败，是期望行为
mod tests {
    use super::*;
    use crate::commands::publish_settings::ensure_partitions;
    use crate::db::repo::{accounts, assets, planning as prepo, skus, texts};
    use crate::db::test_support::test_pool;
    use crate::publish::planner;

    #[tokio::test]
    async fn export_writes_package_with_ready_last() {
        let (pool, dir) = test_pool().await;
        let root = dir.path();
        ensure_partitions(root).unwrap();

        // 造 SKU + 图集包（含真实文件）+ 标题 + 正文 + 账号。
        let sku = skus::insert(
            &pool,
            &skus::NewSku {
                code: "SF-1".into(),
                style_name: "款".into(),
                product_name: "商品".into(),
                tier: "hot".into(),
                topics_json: "[\"沙发\",\"家居\"]".into(),
                platforms_json: Some("[\"xhs\"]".into()),
                note: String::new(),
            },
        )
        .await
        .unwrap();
        // 资产库/SF-1/g1 内放 img_01.jpg + cover.jpg
        let pack_dir = RelPath::new("资产库/SF-1/g1").to_local(root);
        std::fs::create_dir_all(&pack_dir).unwrap();
        std::fs::write(pack_dir.join("img_01.jpg"), b"img").unwrap();
        std::fs::write(pack_dir.join("cover.jpg"), b"cov").unwrap();
        let pack = assets::insert(&pool, &assets::NewPack {
            sku_id: sku, kind: "gallery".into(), dir_rel: "资产库/SF-1/g1".into(),
            files_json: r#"[{"name":"img_01.jpg","origName":"a.jpg","bytes":3},{"name":"cover.jpg","origName":"c.jpg","bytes":3}]"#.into(),
            cover: Some("cover.jpg".into()), source: "inbox".into(),
        }).await.unwrap();
        assets::set_lifecycle(&pool, pack, "active").await.unwrap();
        texts::insert(
            &pool,
            &texts::NewTextItem {
                sku_id: sku,
                kind: "title".into(),
                text: "标题一".into(),
                platform: "general".into(),
                source: "manual".into(),
            },
        )
        .await
        .unwrap();
        texts::insert(
            &pool,
            &texts::NewTextItem {
                sku_id: sku,
                kind: "body".into(),
                text: "正文内容".into(),
                platform: "general".into(),
                source: "manual".into(),
            },
        )
        .await
        .unwrap();
        accounts::insert(
            &pool,
            &accounts::NewAccount {
                platform: "xhs".into(),
                name: "主号".into(),
                daily_limit: 3,
                slots_json: None,
            },
        )
        .await
        .unwrap();

        let mut s = PublishSettings {
            root_local: root.to_string_lossy().to_string(),
            root_exec: "D:\\发布".into(),
            path_style: "windows".into(),
            ..PublishSettings::default()
        };
        s.time_slots = vec!["11:30-13:00".into()];

        let sheet_id = planner::generate_sheet(&pool, "2026-07-15", &s)
            .await
            .unwrap();
        // 确认后导出。
        let mut conn = pool.acquire().await.unwrap();
        prepo::set_sheet_status(&mut conn, sheet_id, "confirmed")
            .await
            .unwrap();
        drop(conn);

        let res = export_package(&pool, sheet_id, &s).await.unwrap();
        assert_eq!(res.row_count, 1);
        assert_eq!(res.sku_count, 1);

        let pkg = RelPath::new("任务包/20260715").to_local(root);
        assert!(pkg.join("任务单.xlsx").exists());
        assert!(pkg.join("执行说明.md").exists());
        assert!(pkg.join("READY.txt").exists());
        assert!(pkg.join("回执截图").is_dir());
        // 素材按 SKU 复制 + body.txt。
        assert!(pkg.join("素材/SF-1/img_01.jpg").exists());
        assert!(pkg.join("素材/SF-1/cover.jpg").exists());
        assert_eq!(
            std::fs::read_to_string(pkg.join("素材/SF-1/body.txt")).unwrap(),
            "正文内容"
        );

        // READY.txt 为最后写入（mtime 最新，>= 其他文件）。
        let ready_mt = std::fs::metadata(pkg.join("READY.txt"))
            .unwrap()
            .modified()
            .unwrap();
        let xlsx_mt = std::fs::metadata(pkg.join("任务单.xlsx"))
            .unwrap()
            .modified()
            .unwrap();
        assert!(ready_mt >= xlsx_mt, "READY 应最后写入");

        // 任务单状态置 exported。
        let sheet = prepo::get_sheet(&pool, sheet_id).await.unwrap().unwrap();
        assert_eq!(sheet.status, "exported");
    }
}
