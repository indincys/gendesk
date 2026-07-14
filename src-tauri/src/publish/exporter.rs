//! 任务包导出（发布模块执行计划 §5.1 exporter / 需求 §4.6）。
//!
//! 素材按 SKU 复制（每 SKU 一份）+ body.txt + 执行说明.md + 任务单.xlsx + READY.txt（最后写）。
//! 第 11/12/14 列按执行机根路径 + 风格拼接绝对路径（导出是唯一转换点）。

use std::path::Path;

use serde::Serialize;
use specta::Type;
use sqlx::SqlitePool;

use crate::commands::publish_settings::PublishSettings;
use crate::db::repo::{accounts, planning};
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
    /// 源文件缺失清单（预检通过后正常应为空；冗余上报便于排查）。
    pub missing_files: Vec<String>,
}

/// 导出预检报告：errors 非空即阻断导出。
#[derive(Debug, Clone, Default, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PreflightReport {
    /// 阻断级问题（导出会被拒绝）。
    pub errors: Vec<String>,
    /// 提醒级问题（导出照常，但值得看一眼）。
    pub warnings: Vec<String>,
    pub row_count: i64,
    pub sku_count: i64,
}

impl PreflightReport {
    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// 导出预检（纯读）：素材齐备 · 路径长度 · 账号在用 · 执行机根路径 · **重导出回执保护**。
///
/// 最后一项是关键：xlsx 是双侧唯一契约，执行器只写 20–22 列。一旦它开始回写，
/// 整包覆盖会把回执抹掉，且无从恢复——所以只要包内 xlsx 有任何一行不是「待执行」，
/// 就拒绝重导出，先走对账。
pub async fn preflight(
    pool: &SqlitePool,
    sheet_id: i64,
    s: &PublishSettings,
) -> AppResult<PreflightReport> {
    let mut rep = PreflightReport::default();
    let sheet = planning::get_sheet(pool, sheet_id)
        .await?
        .ok_or_else(|| AppError::InvalidInput("任务单不存在".into()))?;
    if sheet.status != "confirmed" && sheet.status != "exported" {
        rep.errors.push(format!(
            "任务单当前为「{}」，只有已确认 / 已导出的可以导出",
            sheet.status
        ));
        return Ok(rep);
    }
    if s.root_local.is_empty() {
        rep.errors
            .push("尚未配置本机根目录（设置 › 发布与同步）".into());
        return Ok(rep);
    }
    let local_root = Path::new(&s.root_local);
    let style = PathStyle::from_str_or_default(&s.path_style);

    // 执行机根路径：未配置则回退本机根（同机模式），风格与根路径首字符不一致时提醒。
    if s.root_exec.is_empty() {
        rep.warnings
            .push("未配置执行机根目录，将按本机根目录拼接路径（同机模式）".into());
    } else {
        let looks_windows = s.root_exec.chars().nth(1) == Some(':');
        let looks_unix = s.root_exec.starts_with('/');
        let mismatch = (looks_windows && style == PathStyle::Unix)
            || (looks_unix && style == PathStyle::Windows);
        if mismatch {
            rep.warnings.push(format!(
                "执行机根目录「{}」与路径风格「{}」看起来不匹配，导出的路径可能在执行机上打不开",
                s.root_exec, s.path_style
            ));
        }
    }

    let exec_root = if s.root_exec.is_empty() {
        &s.root_local
    } else {
        &s.root_exec
    };
    let yyyymmdd: String = sheet.date.chars().filter(|c| c.is_ascii_digit()).collect();
    let pkg_rel = RelPath::from_parts([paths::TASK_PACKAGES, &yyyymmdd]);

    // 重导出：执行器已回写 → 禁止覆盖。
    if sheet.status == "exported" {
        let xlsx = pkg_rel.join(paths::TASK_XLSX).to_local(local_root);
        if xlsx.exists() {
            // 读不动（被 Excel 占用/写坏）时不阻断——真有回执的话对账那边也读不到，
            // 这里报 warning 让人先去看一眼。
            match crate::publish::xlsx::reader::read_receipts(&xlsx) {
                Ok(receipts) => {
                    let written = receipts
                        .iter()
                        .filter(|r| !r.status_zh.trim().is_empty() && r.status_zh != "待执行")
                        .count();
                    if written > 0 {
                        rep.errors.push(format!(
                            "检测到执行器已回写 {written} 行回执，禁止覆盖任务包；\
                             请先「导入回执」对账，再决定是否重导出"
                        ));
                    }
                }
                Err(e) => rep.warnings.push(format!(
                    "包内任务单.xlsx 读取失败（{e}），无法确认是否已有回执"
                )),
            }
        }
    }

    let rows = planning::sheet_rows(pool, sheet_id).await?;
    rep.row_count = rows.len() as i64;
    if rows.is_empty() {
        rep.errors.push("任务单没有任何任务行".into());
    }

    // 账号在用。
    let accts = accounts::list(pool).await?;
    let mut seen_skus = std::collections::HashSet::new();
    let mut bad_accounts: Vec<String> = Vec::new();
    for r in &rows {
        if let Some(a) = accts.iter().find(|a| a.id == r.account_id) {
            if a.status != "active" && !bad_accounts.contains(&a.name) {
                bad_accounts.push(a.name.clone());
            }
        } else {
            rep.errors
                .push(format!("任务 {} 引用的账号已不存在", r.task_code));
        }

        if !seen_skus.insert(r.sku_id) {
            continue;
        }
        // 素材文件齐备（存在且非空）。
        let src_dir = RelPath::new(&r.dir_rel).to_local(local_root);
        let names = pack_file_names(&r.files_json);
        if names.is_empty() {
            rep.errors
                .push(format!("{} 的素材包没有任何文件", r.sku_code));
        }
        for name in &names {
            let src = src_dir.join(name);
            match std::fs::metadata(&src) {
                Ok(m) if m.len() > 0 => {}
                Ok(_) => rep
                    .errors
                    .push(format!("{}/{} 是空文件（0 字节）", r.sku_code, name)),
                Err(_) => rep.errors.push(format!(
                    "{}/{} 不存在（{}）",
                    r.sku_code,
                    name,
                    src.display()
                )),
            }
        }
        if r.pack_kind == "gallery" && r.body_text.is_none() {
            rep.errors
                .push(format!("{} 是图集任务但没有正文", r.sku_code));
        }
        // 执行机路径长度（含最长文件名，不能只算到 SKU 目录层）。
        let mat_rel = pkg_rel.join(paths::MATERIALS_DIR).join(&r.sku_code);
        let longest = names.iter().map(String::as_str).max_by_key(|n| n.len());
        let probe = match longest {
            Some(n) => mat_rel.join(n),
            None => mat_rel.clone(),
        };
        let exec_path = paths::exec_join(exec_root, &probe, style);
        if paths::exceeds_path_limit(&exec_path) {
            rep.warnings.push(format!(
                "{} 的执行机路径长 {} 字符，超过 Windows {} 上限，可能被截断",
                r.sku_code,
                exec_path.chars().count(),
                paths::PATH_LIMIT
            ));
        }
    }
    for name in bad_accounts {
        rep.errors
            .push(format!("账号「{name}」已停用，但任务单里仍有它的任务行"));
    }
    rep.sku_count = seen_skus.len() as i64;
    Ok(rep)
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

/// 导出进度回调（每复制完一个文件调一次）。
pub type ProgressFn = std::sync::Arc<dyn Fn(i64, i64) + Send + Sync>;

/// 导出某任务单为任务包。sheet 必须为 confirmed 或 exported（重导出=整包覆盖）。
///
/// 文件搬运（复制视频可达数百 MB × 多 SKU）走 `spawn_blocking`：留在 async 线程上
/// 同步复制会把整个 Tauri 运行时堵死，导出期间 UI 与其它 IPC 全部卡住。
pub async fn export_package(
    pool: &SqlitePool,
    sheet_id: i64,
    s: &PublishSettings,
    progress: Option<ProgressFn>,
) -> AppResult<ExportResult> {
    // 预检是导出的唯一入口条件：素材缺失/回执已回写等问题在这里拦下，
    // 而不是等到执行机上才发现（或把回执覆盖掉）。
    let pre = preflight(pool, sheet_id, s).await?;
    if !pre.ok() {
        return Err(AppError::InvalidInput(format!(
            "导出预检未通过：\n{}",
            pre.errors.join("\n")
        )));
    }

    let sheet = planning::get_sheet(pool, sheet_id)
        .await?
        .ok_or_else(|| AppError::InvalidInput("任务单不存在".into()))?;
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

    let rows = planning::sheet_rows(pool, sheet_id).await?;

    // ── 计划阶段（纯数据，无 IO）：算出要复制哪些文件、写哪些 body.txt ──
    let mut copied_skus = std::collections::HashSet::new();
    let mut long_path_warn = false;
    let mut copy_jobs: Vec<(std::path::PathBuf, std::path::PathBuf, String)> = Vec::new(); // (src, dest, 展示名)
    let mut body_jobs: Vec<(std::path::PathBuf, String)> = Vec::new();
    let mut sku_dirs: Vec<std::path::PathBuf> = Vec::new();
    for r in &rows {
        if !copied_skus.insert(r.sku_id) {
            continue;
        }
        let dest_rel = pkg_rel.join(paths::MATERIALS_DIR).join(&r.sku_code);
        let dest_abs = dest_rel.to_local(local_root);
        sku_dirs.push(dest_abs.clone());
        let src_dir = RelPath::new(&r.dir_rel).to_local(local_root);
        for name in pack_file_names(&r.files_json) {
            copy_jobs.push((
                src_dir.join(&name),
                dest_abs.join(&name),
                format!("{}/{}", r.sku_code, name),
            ));
        }
        if r.pack_kind == "gallery" {
            if let Some(body) = &r.body_text {
                body_jobs.push((dest_abs.join(paths::BODY_TXT), body.clone()));
            }
        }
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

    // ── 落盘阶段：全部文件 IO 挪到阻塞线程池，async 运行时不被数百 MB 的复制堵死 ──
    let ready_path = pkg_rel.join(paths::READY).to_local(local_root);
    let receipts_dir = pkg_rel.join(paths::RECEIPTS_DIR).to_local(local_root);
    let xlsx_path = pkg_rel.join(paths::TASK_XLSX).to_local(local_root);
    let guide_path = pkg_rel.join(paths::EXEC_GUIDE).to_local(local_root);
    let ready_body = format!("READY {}\n{} 行任务\n", sheet.date, rows.len());
    let total = (copy_jobs.len() + body_jobs.len() + 2) as i64;

    let written = tauri::async_runtime::spawn_blocking(move || -> AppResult<(i64, Vec<String>)> {
        // 重导出：先删 READY.txt——执行器以它为「包已就绪」的信号，
        // 必须在其余文件全部落盘后才重新出现。
        let _ = std::fs::remove_file(&ready_path);
        std::fs::create_dir_all(&pkg_abs)?;
        std::fs::create_dir_all(&receipts_dir)?;
        for d in &sku_dirs {
            std::fs::create_dir_all(d)?;
        }

        let mut file_count = 0i64;
        let mut done = 0i64;
        let mut missing: Vec<String> = Vec::new();
        let tick = |done: i64| {
            if let Some(p) = &progress {
                p(done, total);
            }
        };

        for (src, dest, label) in copy_jobs {
            // 预检已保证素材齐备；到这一步还缺，只可能是预检之后被删——记下来上报，不静默跳过。
            if src.exists() {
                std::fs::copy(&src, &dest)?;
                file_count += 1;
            } else {
                missing.push(label);
            }
            done += 1;
            tick(done);
        }
        for (path, body) in body_jobs {
            std::fs::write(&path, body)?;
            file_count += 1;
            done += 1;
            tick(done);
        }

        write_sheet(&xlsx_path, &xrows)?;
        file_count += 1;
        done += 1;
        tick(done);

        std::fs::write(&guide_path, EXEC_GUIDE_TEMPLATE)?;
        file_count += 1;
        done += 1;
        tick(done);

        // READY.txt 最后写（就绪标志，mtime 最新）。
        std::fs::write(&ready_path, ready_body)?;
        Ok((file_count, missing))
    })
    .await
    .map_err(|e| AppError::Internal(format!("导出任务异常终止：{e}")))?;
    let (file_count, missing_files) = written?;

    // 置任务单为已导出。
    let mut conn = pool.acquire().await?;
    planning::set_sheet_status(&mut conn, sheet_id, "exported").await?;

    Ok(ExportResult {
        pkg_dir_rel: pkg_rel.as_str().to_string(),
        row_count: rows.len() as i64,
        sku_count: copied_skus.len() as i64,
        file_count,
        long_path_warn,
        missing_files,
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

        let res = export_package(&pool, sheet_id, &s, None).await.unwrap();
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
        assert!(res.missing_files.is_empty());

        // ── B1 预检 ──────────────────────────────────────────────────
        // 已导出但执行器尚未回写 → 允许重导出。
        assert!(
            preflight(&pool, sheet_id, &s).await.unwrap().ok(),
            "无回执时重导出是允许的"
        );

        // 执行器回写一行「已发布」→ 重导出被拒（整包覆盖会抹掉回执，§6.1 单写方约定）。
        let xlsx = pkg.join("任务单.xlsx");
        let rows = prepo::list_tasks_by_sheet(&pool, sheet_id).await.unwrap();
        crate::publish::xlsx::writer::write_sheet(
            &xlsx,
            &[crate::publish::xlsx::writer::XlsxRow {
                task_id: rows[0].task_code.clone(),
                status_zh: "已发布".into(),
                rpa_info: "https://x｜｜2026-07-15 12:30".into(),
                ..Default::default()
            }],
        )
        .unwrap();
        let rep = preflight(&pool, sheet_id, &s).await.unwrap();
        assert!(!rep.ok(), "已有回执必须拒绝重导出");
        assert!(rep.errors[0].contains("回执"), "{:?}", rep.errors);
        assert!(export_package(&pool, sheet_id, &s, None).await.is_err());

        // 素材文件缺失 → 预检报 error，导出被拒（问题不再推迟到执行机才暴露）。
        std::fs::remove_file(pack_dir.join("img_01.jpg")).unwrap();
        std::fs::remove_file(&xlsx).unwrap(); // 排除上面的回执因素
        let rep = preflight(&pool, sheet_id, &s).await.unwrap();
        assert!(
            rep.errors.iter().any(|e| e.contains("img_01.jpg")),
            "{:?}",
            rep.errors
        );
        assert!(export_package(&pool, sheet_id, &s, None).await.is_err());
    }

    // B1：关单后不可再导出（状态机保证；补断言防回归）。
    #[tokio::test]
    async fn closed_sheet_cannot_be_exported() {
        let (pool, dir) = test_pool().await;
        let root = dir.path();
        ensure_partitions(root).unwrap();
        let s = PublishSettings {
            root_local: root.to_string_lossy().to_string(),
            ..PublishSettings::default()
        };
        let mut conn = pool.acquire().await.unwrap();
        let sheet_id = prepo::create_sheet(&mut conn, "2026-07-15").await.unwrap();
        prepo::set_sheet_status(&mut conn, sheet_id, "closed")
            .await
            .unwrap();
        drop(conn);
        let rep = preflight(&pool, sheet_id, &s).await.unwrap();
        assert!(!rep.ok());
        assert!(export_package(&pool, sheet_id, &s, None).await.is_err());
    }
}
