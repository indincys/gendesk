//! 回执对账（发布模块执行计划 §5.1 reconcile / 需求 §6）。
//!
//! 三分支：已发布 → 台账 + 归档；失败 → 六类处置（timeout 次日补排、risk 当日熔断该账号）；
//! 超时未回写 → 疑似已发（绝不自动重发，硬性 §6.4）。关单 + 日报。
//! 只对**待执行**任务应用回执；疑似只能由 resolve_suspect 人工定态。

use serde::{Deserialize, Serialize};
use specta::Type;
use sqlx::SqlitePool;

use crate::db::repo::{ledger, planning, texts};
use crate::error::{AppError, AppResult};
use crate::publish::xlsx::reader::{parse_rpa, ReceiptRow};

/// 失败六类分类（纯函数）：按执行器回写文案关键字归类。
pub fn classify_fail(text: &str) -> &'static str {
    let t = text;
    if t.contains("登录") {
        "login"
    } else if t.contains("风控") || t.contains("限流") || t.contains("频率") {
        "risk"
    } else if t.contains("素材") || t.contains("不合规") || t.contains("违规") || t.contains("审核")
    {
        "content"
    } else if t.contains("页面") || t.contains("变更") {
        "page"
    } else if t.contains("超时") || t.contains("网络") {
        "timeout"
    } else {
        "other"
    }
}

/// 对账结果汇总。
#[derive(Debug, Clone, Default, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ReconcileResult {
    pub published: i64,
    pub failed: i64,
    /// 风控熔断连带取消的任务数。
    pub canceled_by_risk: i64,
    pub matched: i64,
    pub unmatched: i64,
    pub closed: bool,
}

/// 解析回执时间为 Unix 秒；失败回退 now。
fn parse_time_or(now: i64, s: Option<&str>) -> i64 {
    use chrono::NaiveDateTime;
    let Some(s) = s else { return now };
    for fmt in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M", "%Y/%m/%d %H:%M"] {
        if let Ok(dt) = NaiveDateTime::parse_from_str(s.trim(), fmt) {
            return dt.and_utc().timestamp();
        }
    }
    now
}

/// 应用一组回执到某任务单（只处理待执行任务）。
pub async fn apply_receipts(
    pool: &SqlitePool,
    sheet_id: i64,
    receipts: &[ReceiptRow],
) -> AppResult<ReconcileResult> {
    let sheet = planning::get_sheet(pool, sheet_id)
        .await?
        .ok_or_else(|| AppError::InvalidInput("任务单不存在".into()))?;
    let now = crate::db::now_unix();
    let mut res = ReconcileResult::default();

    for rc in receipts {
        let Some(task) = planning::find_task_by_code(pool, &rc.task_code).await? else {
            res.unmatched += 1;
            continue;
        };
        if task.sheet_id != sheet_id || task.status != "pending" {
            // 非本单、或已定态/疑似（疑似只能人工定态）→ 跳过。
            continue;
        }
        res.matched += 1;
        match rc.status_zh.as_str() {
            "已发布" => {
                let rpa = parse_rpa(&rc.rpa_info);
                let pub_at = parse_time_or(now, rpa.time.as_deref());
                mark_published(
                    pool,
                    &task,
                    &sheet.date,
                    rpa.url.as_deref(),
                    &rc.rpa_info,
                    pub_at,
                    Some(&rc.screenshot)
                        .filter(|s| !s.is_empty())
                        .map(|s| s.as_str()),
                )
                .await?;
                res.published += 1;
            }
            "失败" => {
                let reason = parse_rpa(&rc.rpa_info)
                    .reason
                    .unwrap_or_else(|| rc.rpa_info.clone());
                let kind = classify_fail(&reason);
                let mut conn = pool.begin().await?;
                planning::update_task_result(
                    &mut conn,
                    task.id,
                    "failed",
                    Some(kind),
                    None,
                    Some(&reason),
                    Some(now),
                    None,
                )
                .await?;
                // 风控 → 当日熔断该账号剩余待执行。
                if kind == "risk" {
                    let n =
                        planning::cancel_pending_of_account(&mut conn, sheet_id, task.account_id)
                            .await?;
                    res.canceled_by_risk += n as i64;
                }
                conn.commit().await?;
                res.failed += 1;
            }
            // 空 / 待执行 → 尚未回写，留待超时扫描。
            _ => {}
        }
    }
    Ok(res)
}

/// 记已发布：台账 + 任务定态 + 文本使用计数（单事务）。
async fn mark_published(
    pool: &SqlitePool,
    task: &planning::PublishTaskRow,
    date: &str,
    url: Option<&str>,
    msg: &str,
    published_at: i64,
    screenshot: Option<&str>,
) -> AppResult<()> {
    let set = planning::get_daily_set(pool, task.set_id)
        .await?
        .ok_or_else(|| AppError::Internal("套装丢失".into()))?;
    let mut conn = pool.begin().await?;
    planning::update_task_result(
        &mut conn,
        task.id,
        "published",
        None,
        url,
        Some(msg),
        Some(published_at),
        screenshot,
    )
    .await?;
    ledger::insert_conn(
        &mut conn,
        &ledger::NewLedger {
            date: date.to_string(),
            sku_id: set.sku_id,
            pack_id: set.pack_id,
            title_id: set.title_id,
            body_id: set.body_id,
            platform: task.platform.clone(),
            account_id: task.account_id,
            task_code: task.task_code.clone(),
            published_at,
            url: url.map(str::to_string),
        },
    )
    .await?;
    texts::bump_use_count(&mut conn, set.title_id).await?;
    if let Some(bid) = set.body_id {
        texts::bump_use_count(&mut conn, bid).await?;
    }
    conn.commit().await?;
    Ok(())
}

/// 疑似已发人工定态结果。
#[derive(Debug, Clone, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SuspectOutcome {
    /// 已发布（补录链接）。
    Published { url: Option<String> },
    /// 未发出，定为失败。
    Failed {
        #[serde(rename = "failKind")]
        fail_kind: String,
    },
}

/// 人工定态一个疑似已发任务（§6.4：唯一改动 suspect 的路径）。
pub async fn resolve_suspect(
    pool: &SqlitePool,
    task_id: i64,
    outcome: SuspectOutcome,
) -> AppResult<()> {
    let task = planning::get_task(pool, task_id)
        .await?
        .ok_or_else(|| AppError::InvalidInput("任务不存在".into()))?;
    if task.status != "suspect" {
        return Err(AppError::InvalidInput("只有疑似已发可人工定态".into()));
    }
    let sheet = planning::get_sheet(pool, task.sheet_id)
        .await?
        .ok_or_else(|| AppError::Internal("任务单丢失".into()))?;
    let now = crate::db::now_unix();
    match outcome {
        SuspectOutcome::Published { url } => {
            mark_published(
                pool,
                &task,
                &sheet.date,
                url.as_deref(),
                "人工核实已发布",
                now,
                None,
            )
            .await?;
        }
        SuspectOutcome::Failed { fail_kind } => {
            let kind = if matches!(
                fail_kind.as_str(),
                "login" | "risk" | "content" | "page" | "timeout" | "other"
            ) {
                fail_kind
            } else {
                "other".to_string()
            };
            let mut conn = pool.begin().await?;
            planning::update_task_result(
                &mut conn,
                task_id,
                "failed",
                Some(&kind),
                None,
                Some("人工核实未发出"),
                Some(now),
                None,
            )
            .await?;
            conn.commit().await?;
        }
    }
    Ok(())
}

/// 超时扫描：已导出单中「待执行」且超过回执超时的任务 → 疑似已发。返回标记数。
/// 基准：任务日期+定时发布时间 + 超时小时；无定时以导出时间 + 超时小时。
pub async fn timeout_scan(pool: &SqlitePool, now: i64, timeout_hours: i64) -> AppResult<i64> {
    use chrono::NaiveDateTime;
    let window = timeout_hours.max(0) * 3600;
    let mut marked = 0i64;
    for sheet in planning::exported_with_pending(pool).await? {
        let export_at = sheet.exported_at.unwrap_or(now);
        for task in planning::list_tasks_by_sheet(pool, sheet.id).await? {
            if task.status != "pending" {
                continue;
            }
            let base = match &task.planned_time {
                Some(hm) => {
                    let s = format!("{} {}", sheet.date, hm);
                    NaiveDateTime::parse_from_str(&s, "%Y-%m-%d %H:%M")
                        .map(|dt| dt.and_utc().timestamp())
                        .unwrap_or(export_at)
                }
                None => export_at,
            };
            if now >= base + window {
                let mut conn = pool.begin().await?;
                planning::update_task_result(
                    &mut conn,
                    task.id,
                    "suspect",
                    None,
                    None,
                    Some("超时未回写，疑似已发"),
                    Some(now),
                    None,
                )
                .await?;
                conn.commit().await?;
                marked += 1;
            }
        }
    }
    Ok(marked)
}

/// 日报视图。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ReportView {
    pub date: String,
    pub plan: i64,
    pub published: i64,
    pub failed: i64,
    pub canceled: i64,
    /// 成功率（0–100 整数）。
    pub success_rate: i64,
    pub fails: Vec<ReportFail>,
    pub shortage: Vec<String>,
    pub tips: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ReportFail {
    pub task_code: String,
    pub sku_code: String,
    pub kind: String,
}

/// 若全部行到达终态（无 pending/suspect）→ 关单 + 生成日报。返回是否已关闭。
pub async fn maybe_close(pool: &SqlitePool, sheet_id: i64) -> AppResult<bool> {
    if !planning::all_terminal(pool, sheet_id).await? {
        // 仍有待执行/疑似 → 置回收中（部分回执）。
        let sheet = planning::get_sheet(pool, sheet_id).await?;
        if let Some(s) = sheet {
            if s.status == "exported" {
                let has_result: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM publish_tasks WHERE sheet_id=?1 AND status NOT IN ('pending')",
                )
                .bind(sheet_id)
                .fetch_one(pool)
                .await?;
                if has_result > 0 {
                    let mut conn = pool.acquire().await?;
                    planning::set_sheet_status(&mut conn, sheet_id, "reconciling").await?;
                }
            }
        }
        return Ok(false);
    }

    // 全终态 → 关单 + 日报。
    let sheet = planning::get_sheet(pool, sheet_id)
        .await?
        .ok_or_else(|| AppError::InvalidInput("任务单不存在".into()))?;
    let rows = planning::sheet_rows(pool, sheet_id).await?;
    let count = |st: &str| rows.iter().filter(|r| r.status == st).count() as i64;
    let published = count("published");
    let failed = count("failed");
    let canceled = count("canceled");
    let plan = rows.len() as i64;
    let done = published + failed;
    let success_rate = if done > 0 { published * 100 / done } else { 0 };
    let fails: Vec<ReportFail> = rows
        .iter()
        .filter(|r| r.status == "failed")
        .map(|r| ReportFail {
            task_code: r.task_code.clone(),
            sku_code: r.sku_code.clone(),
            kind: r.fail_kind.clone().unwrap_or_else(|| "other".into()),
        })
        .collect();
    let shortage: Vec<String> =
        serde_json::from_str::<Vec<serde_json::Value>>(&sheet.shortage_json)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|v| v.get("code").and_then(|c| c.as_str()).map(str::to_string))
            .collect();
    let has_timeout = fails.iter().any(|f| f.kind == "timeout");
    let tips = if has_timeout {
        "存在网络超时失败，明日将自动补排相关 SKU×账号".to_string()
    } else if !fails.is_empty() {
        "失败任务需人工跟进（风控/素材/登录等）".to_string()
    } else {
        "全部成功，无需跟进".to_string()
    };
    let report = ReportView {
        date: sheet.date.clone(),
        plan,
        published,
        failed,
        canceled,
        success_rate,
        fails,
        shortage,
        tips,
    };
    let json = serde_json::to_string(&report)?;
    let mut conn = pool.acquire().await?;
    planning::set_report(&mut conn, sheet_id, &json).await?;
    planning::set_sheet_status(&mut conn, sheet_id, "closed").await?;
    Ok(true)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即测试失败，是期望行为
mod e2e {
    use super::*;
    use crate::commands::publish_settings::{ensure_partitions, PublishSettings};
    use crate::db::repo::{accounts, assets, planning as prepo, skus, texts};
    use crate::db::test_support::test_pool;
    use crate::publish::paths::RelPath;
    use crate::publish::planner;
    use crate::publish::xlsx::reader;
    use crate::publish::xlsx::writer::{write_sheet, XlsxRow};

    async fn seed(pool: &sqlx::SqlitePool, root: &std::path::Path) {
        let sku = skus::insert(
            pool,
            &skus::NewSku {
                code: "SF-1".into(),
                style_name: "款".into(),
                product_name: "商品".into(),
                tier: "hot".into(),
                topics_json: "[]".into(),
                platforms_json: Some("[\"xhs\"]".into()),
                note: String::new(),
            },
        )
        .await
        .unwrap();
        let pdir = RelPath::new("资产库/SF-1/v1").to_local(root);
        std::fs::create_dir_all(&pdir).unwrap();
        std::fs::write(pdir.join("video.mp4"), b"v").unwrap();
        let pack = assets::insert(
            pool,
            &assets::NewPack {
                sku_id: sku,
                kind: "video".into(),
                dir_rel: "资产库/SF-1/v1".into(),
                files_json: r#"[{"name":"video.mp4","origName":"a.mp4","bytes":1}]"#.into(),
                cover: None,
                source: "inbox".into(),
            },
        )
        .await
        .unwrap();
        assets::set_lifecycle(pool, pack, "active").await.unwrap();
        texts::insert(
            pool,
            &texts::NewTextItem {
                sku_id: sku,
                kind: "title".into(),
                text: "标题".into(),
                platform: "general".into(),
                source: "manual".into(),
            },
        )
        .await
        .unwrap();
        // 两个 xhs 账号 → 2 行。
        accounts::insert(
            pool,
            &accounts::NewAccount {
                platform: "xhs".into(),
                name: "号A".into(),
                daily_limit: 3,
                slots_json: None,
            },
        )
        .await
        .unwrap();
        accounts::insert(
            pool,
            &accounts::NewAccount {
                platform: "xhs".into(),
                name: "号B".into(),
                daily_limit: 3,
                slots_json: None,
            },
        )
        .await
        .unwrap();
    }

    fn settings(root: &std::path::Path) -> PublishSettings {
        PublishSettings {
            root_local: root.to_string_lossy().to_string(),
            root_exec: "D:\\发布".into(),
            path_style: "windows".into(),
            time_slots: vec!["11:30-13:00".into()],
            ..PublishSettings::default()
        }
    }

    /// 回写任务包 xlsx 模拟执行器（整文件重写，只填 20–22 列）。
    fn write_receipts(root: &std::path::Path, date: &str, receipts: &[(&str, &str, &str)]) {
        let yy: String = date.chars().filter(|c| c.is_ascii_digit()).collect();
        let xlsx = RelPath::from_parts(["任务包", &yy, "任务单.xlsx"]).to_local(root);
        std::fs::create_dir_all(xlsx.parent().unwrap()).unwrap();
        let rows: Vec<XlsxRow> = receipts
            .iter()
            .map(|(code, st, rpa)| XlsxRow {
                task_id: (*code).into(),
                status_zh: (*st).into(),
                rpa_info: (*rpa).into(),
                ..Default::default()
            })
            .collect();
        write_sheet(&xlsx, &rows).unwrap();
    }

    #[tokio::test]
    async fn e2e_export_reconcile_close_report() {
        let (pool, dir) = test_pool().await;
        let root = dir.path();
        ensure_partitions(root).unwrap();
        seed(&pool, root).await;
        let s = settings(root);

        let sheet_id = planner::generate_sheet(&pool, "2026-07-15", &s)
            .await
            .unwrap();
        let mut conn = pool.acquire().await.unwrap();
        prepo::set_sheet_status(&mut conn, sheet_id, "confirmed")
            .await
            .unwrap();
        drop(conn);
        crate::publish::exporter::export_package(&pool, sheet_id, &s)
            .await
            .unwrap();

        let rows = prepo::list_tasks_by_sheet(&pool, sheet_id).await.unwrap();
        assert_eq!(rows.len(), 2);
        // 执行器：行1 已发布，行2 失败（风控）。
        write_receipts(
            root,
            "2026-07-15",
            &[
                (
                    &rows[0].task_code,
                    "已发布",
                    "https://xhs.com/x｜｜2026-07-15 12:30",
                ),
                (&rows[1].task_code, "失败", "风控拦截｜2026-07-15 20:00"),
            ],
        );
        let xlsx = RelPath::from_parts(["任务包", "20260715", "任务单.xlsx"]).to_local(root);
        let receipts = reader::read_receipts(&xlsx).unwrap();
        let res = apply_receipts(&pool, sheet_id, &receipts).await.unwrap();
        assert_eq!(res.published, 1);
        assert_eq!(res.failed, 1);

        // 台账入 1 条；标题 use_count +1。
        let set = prepo::get_daily_set(&pool, rows[0].set_id)
            .await
            .unwrap()
            .unwrap();
        let hist = ledger::history_by_sku(&pool, set.sku_id, 10, 0)
            .await
            .unwrap();
        assert_eq!(hist.len(), 1);
        assert_eq!(hist[0].url.as_deref(), Some("https://xhs.com/x"));

        // 全终态 → 关单 + 日报。
        let closed = maybe_close(&pool, sheet_id).await.unwrap();
        assert!(closed);
        let sheet = prepo::get_sheet(&pool, sheet_id).await.unwrap().unwrap();
        assert_eq!(sheet.status, "closed");
        let report: ReportView = serde_json::from_str(&sheet.report_json.unwrap()).unwrap();
        assert_eq!(report.published, 1);
        assert_eq!(report.failed, 1);
        assert_eq!(report.fails[0].kind, "risk");
    }

    #[tokio::test]
    async fn suspect_never_auto_changed_only_manual() {
        let (pool, dir) = test_pool().await;
        let root = dir.path();
        ensure_partitions(root).unwrap();
        seed(&pool, root).await;
        let s = settings(root);
        let sheet_id = planner::generate_sheet(&pool, "2026-07-15", &s)
            .await
            .unwrap();
        let mut conn = pool.acquire().await.unwrap();
        prepo::set_sheet_status(&mut conn, sheet_id, "exported")
            .await
            .unwrap();
        drop(conn);

        // 超时扫描 → 两行都疑似已发。
        let now = crate::db::now_unix() + 100 * 3600;
        let n = timeout_scan(&pool, now, 4).await.unwrap();
        assert_eq!(n, 2);
        let rows = prepo::list_tasks_by_sheet(&pool, sheet_id).await.unwrap();
        assert!(rows.iter().all(|r| r.status == "suspect"));

        // 负向断言：即便执行器回写「已发布」，apply_receipts 也不改动疑似任务。
        write_receipts(
            root,
            "2026-07-15",
            &[(&rows[0].task_code, "已发布", "https://x｜｜")],
        );
        let xlsx = RelPath::from_parts(["任务包", "20260715", "任务单.xlsx"]).to_local(root);
        let receipts = reader::read_receipts(&xlsx).unwrap();
        let res = apply_receipts(&pool, sheet_id, &receipts).await.unwrap();
        assert_eq!(res.published, 0, "疑似任务不被回执自动改动");
        let still = prepo::get_task(&pool, rows[0].id).await.unwrap().unwrap();
        assert_eq!(still.status, "suspect");

        // 负向断言：重生成（自动路径）不改动疑似任务。
        let _ = planner::generate_sheet(&pool, "2026-07-15", &s).await; // 已是 exported → 报错，不动
                                                                        // 疑似仍在 → 阻塞关单。
        assert!(!maybe_close(&pool, sheet_id).await.unwrap());

        // 唯一路径：人工定态。
        resolve_suspect(
            &pool,
            rows[0].id,
            SuspectOutcome::Published {
                url: Some("https://y".into()),
            },
        )
        .await
        .unwrap();
        resolve_suspect(
            &pool,
            rows[1].id,
            SuspectOutcome::Failed {
                fail_kind: "other".into(),
            },
        )
        .await
        .unwrap();
        let r0 = prepo::get_task(&pool, rows[0].id).await.unwrap().unwrap();
        assert_eq!(r0.status, "published");
        // 全终态 → 可关单。
        assert!(maybe_close(&pool, sheet_id).await.unwrap());
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即测试失败，是期望行为
mod tests {
    use super::*;

    #[test]
    fn classify_all_six() {
        assert_eq!(classify_fail("登录失效"), "login");
        assert_eq!(classify_fail("风控拦截"), "risk");
        assert_eq!(classify_fail("素材不合规"), "content");
        assert_eq!(classify_fail("页面变更"), "page");
        assert_eq!(classify_fail("网络超时"), "timeout");
        assert_eq!(classify_fail("莫名其妙"), "other");
    }

    #[test]
    fn parse_time_formats() {
        let now = 999;
        assert_ne!(parse_time_or(now, Some("2026-07-15 12:30")), now);
        assert_ne!(parse_time_or(now, Some("2026-07-15 12:30:45")), now);
        assert_eq!(parse_time_or(now, Some("乱码")), now);
        assert_eq!(parse_time_or(now, None), now);
    }
}
