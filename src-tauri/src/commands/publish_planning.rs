//! 任务单编排域命令（发布模块执行计划 4.1 planning 域）。

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, State};
use tauri_specta::Event;

use crate::commands::publish_settings;
use crate::db::repo::{accounts, assets, ledger, planning, texts};
use crate::error::{AppError, AppResult};
use crate::publish::events::{ExportProgressEvent, SheetChangedEvent};
use crate::publish::planner::{self, set_picker, ShortageItem};
use crate::publish::platform::Platform;
use crate::state::AppState;

// ─────────────────────────────────────────────── 视图类型

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SheetSummary {
    pub id: i64,
    pub date: String,
    pub status: String,
    pub task_count: i64,
    pub shortage_count: i64,
    /// 各状态计数（待执行/已发布/失败/疑似/已取消）。
    pub pending: i64,
    pub published: i64,
    pub failed: i64,
    pub suspect: i64,
    pub canceled: i64,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TaskRowView {
    pub id: i64,
    pub task_code: String,
    pub sku_id: i64,
    pub sku_code: String,
    pub style_name: String,
    pub product_name: String,
    pub title: String,
    pub topics: Vec<String>,
    /// 封面绝对本地路径（前端 convertFileSrc）；无封面为 null。
    pub cover_path: Option<String>,
    pub platform: String,
    pub platform_zh: String,
    pub account_name: String,
    pub content_kind: String,
    pub planned_time: Option<String>,
    pub status: String,
    pub fail_kind: Option<String>,
    pub result_url: Option<String>,
    pub result_msg: Option<String>,
    /// 回执截图绝对本地路径（执行器回传时才有；F2 核对时内嵌展示）。
    pub screenshot_path: Option<String>,
    /// 取消原因：manual（人工）| risk（风控熔断）。
    pub cancel_kind: Option<String>,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SheetDetail {
    pub id: i64,
    pub date: String,
    pub status: String,
    pub shortage: Vec<ShortageItem>,
    pub rows: Vec<TaskRowView>,
    /// 生成之后有过人工调整（改时间/增补行/换套装）。重生成会清掉这些改动，
    /// 前端据此在「重新生成」前弹确认。
    pub edited: bool,
}

// ─────────────────────────────────────────────── 辅助

fn platform_zh(code: &str) -> String {
    Platform::from_code(code)
        .map(|p| p.zh().to_string())
        .unwrap_or_else(|| code.to_string())
}

async fn build_summary(pool: &sqlx::SqlitePool, s: planning::SheetRow) -> AppResult<SheetSummary> {
    let rows = planning::list_tasks_by_sheet(pool, s.id).await?;
    let shortage: Vec<ShortageItem> = serde_json::from_str(&s.shortage_json).unwrap_or_default();
    let count = |st: &str| rows.iter().filter(|r| r.status == st).count() as i64;
    Ok(SheetSummary {
        id: s.id,
        date: s.date,
        status: s.status,
        task_count: rows.len() as i64,
        shortage_count: shortage.len() as i64,
        pending: count("pending"),
        published: count("published"),
        failed: count("failed"),
        suspect: count("suspect"),
        canceled: count("canceled"),
    })
}

async fn build_detail(state: &AppState, sheet_id: i64) -> AppResult<SheetDetail> {
    let sheet = planning::get_sheet(&state.db, sheet_id)
        .await?
        .ok_or_else(|| AppError::InvalidInput("任务单不存在".into()))?;
    let root = publish_settings::root_local(&state.db).await.ok();
    let yyyymmdd: String = sheet.date.chars().filter(|c| c.is_ascii_digit()).collect();
    let joined = planning::sheet_rows(&state.db, sheet_id).await?;
    let rows = joined
        .into_iter()
        .map(|r| {
            let cover_path = match (&root, &r.cover) {
                (Some(root), Some(cover)) => {
                    let rel = crate::publish::paths::RelPath::new(&r.dir_rel).join(cover);
                    Some(rel.to_local(root).to_string_lossy().to_string())
                }
                _ => None,
            };
            // 回执截图：任务包/{date}/回执截图/{文件名}（执行器回写第 22 列时才有）。
            let screenshot_path = match (&root, r.screenshot.as_deref().filter(|s| !s.is_empty())) {
                (Some(root), Some(name)) => Some(
                    crate::publish::paths::RelPath::from_parts([
                        crate::publish::paths::TASK_PACKAGES,
                        &yyyymmdd,
                        crate::publish::paths::RECEIPTS_DIR,
                        name,
                    ])
                    .to_local(root)
                    .to_string_lossy()
                    .to_string(),
                ),
                _ => None,
            };
            let topics: Vec<String> = serde_json::from_str(&r.topics_json).unwrap_or_default();
            TaskRowView {
                id: r.id,
                task_code: r.task_code,
                sku_id: r.sku_id,
                sku_code: r.sku_code,
                style_name: r.style_name,
                product_name: r.product_name,
                title: r.title_text,
                topics: topics.into_iter().take(5).collect(),
                cover_path,
                platform_zh: platform_zh(&r.platform),
                platform: r.platform,
                account_name: r.account_name,
                content_kind: r.content_kind,
                planned_time: r.planned_time,
                status: r.status,
                fail_kind: r.fail_kind,
                result_url: r.result_url,
                result_msg: r.result_msg,
                screenshot_path,
                cancel_kind: r.cancel_kind,
            }
        })
        .collect();
    let shortage: Vec<ShortageItem> =
        serde_json::from_str(&sheet.shortage_json).unwrap_or_default();
    let edited = planning::is_edited(&state.db, sheet_id).await?;
    Ok(SheetDetail {
        id: sheet.id,
        date: sheet.date,
        status: sheet.status,
        shortage,
        rows,
        edited,
    })
}

/// 发 SheetChangedEvent（工作台/看板刷新）。
async fn emit_changed(app: &AppHandle, pool: &sqlx::SqlitePool, sheet_id: i64) {
    if let Ok(Some(s)) = planning::get_sheet(pool, sheet_id).await {
        if let Ok(sum) = build_summary(pool, s.clone()).await {
            let _ = SheetChangedEvent {
                sheet_id,
                date: sum.date,
                status: sum.status,
                pending: sum.pending,
                published: sum.published,
                failed: sum.failed,
                suspect: sum.suspect,
                canceled: sum.canceled,
            }
            .emit(app);
        }
    }
    crate::publish::inbox::watcher::emit_badges(pool, app).await;
}

// ─────────────────────────────────────────────── 命令

/// 生成/重生成某日任务单草稿。
#[tauri::command]
#[specta::specta]
pub async fn generate_sheet(
    state: State<'_, AppState>,
    app: AppHandle,
    date: String,
) -> AppResult<SheetDetail> {
    let settings = publish_settings::load(&state.db).await?;
    let sheet_id = planner::generate_sheet(&state.db, &date, &settings).await?;
    emit_changed(&app, &state.db, sheet_id).await;
    build_detail(&state, sheet_id).await
}

#[tauri::command]
#[specta::specta]
pub async fn list_sheets(state: State<'_, AppState>) -> AppResult<Vec<SheetSummary>> {
    // 两条查询搞定：单据表 + 一条 GROUP BY 的状态计数。
    // 原来是每张单拉全部任务行再在内存里数，单据攒到几十张就明显卡。
    let counts = planning::sheet_status_counts(&state.db).await?;
    Ok(planning::list_sheets(&state.db)
        .await?
        .into_iter()
        .map(|s| {
            let shortage: Vec<ShortageItem> =
                serde_json::from_str(&s.shortage_json).unwrap_or_default();
            let n = |st: &str| counts.get(&(s.id, st.to_string())).copied().unwrap_or(0);
            SheetSummary {
                task_count: n("pending")
                    + n("published")
                    + n("failed")
                    + n("suspect")
                    + n("canceled"),
                shortage_count: shortage
                    .iter()
                    .filter(|i| i.reason != "timeout_backfill")
                    .count() as i64,
                pending: n("pending"),
                published: n("published"),
                failed: n("failed"),
                suspect: n("suspect"),
                canceled: n("canceled"),
                id: s.id,
                date: s.date,
                status: s.status,
            }
        })
        .collect())
}

#[tauri::command]
#[specta::specta]
pub async fn get_sheet(state: State<'_, AppState>, id: i64) -> AppResult<SheetDetail> {
    build_detail(&state, id).await
}

/// 确认任务单（草稿 → 已确认，锁定）。
#[tauri::command]
#[specta::specta]
pub async fn confirm_sheet(
    state: State<'_, AppState>,
    app: AppHandle,
    id: i64,
) -> AppResult<SheetDetail> {
    let sheet = planning::get_sheet(&state.db, id)
        .await?
        .ok_or_else(|| AppError::InvalidInput("任务单不存在".into()))?;
    if sheet.status != "draft" {
        return Err(AppError::InvalidInput("只有草稿可确认".into()));
    }
    let mut conn = state.db.acquire().await?;
    planning::set_sheet_status(&mut conn, id, "confirmed").await?;
    drop(conn);
    emit_changed(&app, &state.db, id).await;
    build_detail(&state, id).await
}

/// 退回草稿（已确认 → 草稿；已导出不可退回）。
#[tauri::command]
#[specta::specta]
pub async fn unlock_sheet(
    state: State<'_, AppState>,
    app: AppHandle,
    id: i64,
) -> AppResult<SheetDetail> {
    let sheet = planning::get_sheet(&state.db, id)
        .await?
        .ok_or_else(|| AppError::InvalidInput("任务单不存在".into()))?;
    if sheet.status != "confirmed" {
        return Err(AppError::InvalidInput(
            "只有已确认（未导出）可退回草稿".into(),
        ));
    }
    let mut conn = state.db.acquire().await?;
    planning::set_sheet_status(&mut conn, id, "draft").await?;
    drop(conn);
    emit_changed(&app, &state.db, id).await;
    build_detail(&state, id).await
}

/// 校验任务单可编辑（仅草稿）。
async fn ensure_draft(pool: &sqlx::SqlitePool, sheet_id: i64) -> AppResult<()> {
    let sheet = planning::get_sheet(pool, sheet_id)
        .await?
        .ok_or_else(|| AppError::InvalidInput("任务单不存在".into()))?;
    if sheet.status != "draft" {
        return Err(AppError::InvalidInput(
            "任务单非草稿，不能编辑；请先退回草稿".into(),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TaskRowPatch {
    /// `Some(None)` = 清空（立即发）；`Some(Some)` = 设为 HH:MM。
    pub planned_time: Option<Option<String>>,
}

/// 校验计划时间：`None` = 清空（立即发）；`Some` 必须是 `HH:MM`。
/// 不校验的话任意字符串会进 xlsx 第 5 列，执行器读不懂；超时扫描也只能静默回退到导出时刻。
fn validate_planned_time(pt: &Option<String>) -> AppResult<()> {
    if let Some(t) = pt {
        if crate::publish::planner::scheduler::parse_hhmm(t).is_none() {
            return Err(AppError::InvalidInput(format!(
                "定时发布时间格式应为 HH:MM（00:00–23:59），收到「{t}」"
            )));
        }
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn update_task_row(
    state: State<'_, AppState>,
    id: i64,
    patch: TaskRowPatch,
) -> AppResult<()> {
    let task = planning::get_task(&state.db, id)
        .await?
        .ok_or_else(|| AppError::InvalidInput("任务行不存在".into()))?;
    ensure_draft(&state.db, task.sheet_id).await?;
    if let Some(pt) = patch.planned_time {
        validate_planned_time(&pt)?;
        planning::update_task_time(&state.db, id, pt.as_deref()).await?;
    }
    Ok(())
}

/// 人工取消一行（只允许待执行）。
///
/// 硬性红线（需求 §6.4）：published/failed/**suspect** 都是已定态的，取消它们等于
/// 绕过 `resolve_suspect` 这条唯一定态路径。取消后立即尝试关单——否则取消掉最后一个
/// 待执行行的单会永远停在「回收中」，不出日报。
#[tauri::command]
#[specta::specta]
pub async fn cancel_task_row(state: State<'_, AppState>, app: AppHandle, id: i64) -> AppResult<()> {
    let task = planning::get_task(&state.db, id)
        .await?
        .ok_or_else(|| AppError::InvalidInput("任务行不存在".into()))?;
    if task.status != "pending" {
        return Err(AppError::InvalidInput(format!(
            "只有待执行的任务可以取消；该任务当前为「{}」{}",
            crate::publish::reconcile::task_status_zh(&task.status),
            if task.status == "suspect" {
                "，请到核对台人工定态"
            } else {
                ""
            }
        )));
    }
    let mut conn = state.db.acquire().await?;
    let n = planning::cancel_task_manual(&mut conn, id).await?;
    drop(conn);
    if n == 0 {
        return Err(AppError::InvalidInput(
            "任务状态已变化，请刷新后重试".into(),
        ));
    }
    crate::publish::reconcile::maybe_close(&state.db, task.sheet_id).await?;
    emit_changed(&app, &state.db, task.sheet_id).await;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn delete_task_row(state: State<'_, AppState>, app: AppHandle, id: i64) -> AppResult<()> {
    let task = planning::get_task(&state.db, id)
        .await?
        .ok_or_else(|| AppError::InvalidInput("任务行不存在".into()))?;
    ensure_draft(&state.db, task.sheet_id).await?;
    planning::delete_task(&state.db, id).await?;
    emit_changed(&app, &state.db, task.sheet_id).await;
    Ok(())
}

#[derive(Debug, Clone, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AddTaskRowInput {
    pub sheet_id: i64,
    pub sku_id: i64,
    pub account_id: i64,
    pub planned_time: Option<String>,
}

/// 增补任务行：使用该 SKU 当日套装（无则即时选取一套）。
#[tauri::command]
#[specta::specta]
pub async fn add_task_row(
    state: State<'_, AppState>,
    app: AppHandle,
    input: AddTaskRowInput,
) -> AppResult<()> {
    ensure_draft(&state.db, input.sheet_id).await?;
    validate_planned_time(&input.planned_time)?;
    let sheet = planning::get_sheet(&state.db, input.sheet_id)
        .await?
        .ok_or_else(|| AppError::InvalidInput("任务单不存在".into()))?;
    let account = accounts::list(&state.db)
        .await?
        .into_iter()
        .find(|a| a.id == input.account_id)
        .ok_or_else(|| AppError::InvalidInput("账号不存在".into()))?;

    // 找该 SKU 当日套装；无则即时选取（选取是纯读，放事务外）。
    let existing_set =
        sqlx::query_scalar::<_, i64>("SELECT id FROM daily_sets WHERE date = ?1 AND sku_id = ?2")
            .bind(&sheet.date)
            .bind(input.sku_id)
            .fetch_optional(&state.db)
            .await?;
    let picked = match existing_set {
        Some(_) => None,
        None => Some(pick_set_for(&state, input.sku_id, &account.platform).await?),
    };

    let yy: String = sheet
        .date
        .chars()
        .filter(|c| c.is_ascii_digit())
        .skip(2)
        .collect();
    let next = planning::max_task_seq(&state.db, input.sheet_id, &yy).await? + 1;
    let task_code = format!("T{yy}-{next:03}");

    // 建套装 + 插任务在同一事务：中途失败不会留下一个没有任务行的孤儿套装。
    let mut tx = state.db.begin().await?;
    let (set_id, content_kind) = match (existing_set, picked) {
        (Some(sid), _) => {
            let ds = planning::get_daily_set(&state.db, sid)
                .await?
                .ok_or_else(|| AppError::Internal("套装读取失败".into()))?;
            let pack = assets::get(&state.db, ds.pack_id).await?;
            let kind = pack.map(|p| p.kind).unwrap_or_else(|| "video".into());
            (sid, kind)
        }
        (None, Some(pick)) => {
            let sid = planning::insert_daily_set(
                &mut tx,
                &planning::NewDailySet {
                    date: sheet.date.clone(),
                    sku_id: input.sku_id,
                    pack_id: pick.pack_id,
                    title_id: pick.title_id,
                    body_id: pick.body_id,
                },
            )
            .await?;
            (sid, pick.content_kind)
        }
        (None, None) => return Err(AppError::Internal("套装选取缺失".into())),
    };
    planning::insert_publish_task(
        &mut tx,
        &planning::NewPublishTask {
            sheet_id: input.sheet_id,
            task_code,
            set_id,
            account_id: input.account_id,
            platform: account.platform,
            content_kind,
            planned_time: input.planned_time,
        },
    )
    .await?;
    tx.commit().await?;

    emit_changed(&app, &state.db, input.sheet_id).await;
    Ok(())
}

/// 整包换该 SKU 当日套装（重选素材/标题/正文）。所有引用该套装的行同步生效。
#[tauri::command]
#[specta::specta]
pub async fn reroll_set(
    state: State<'_, AppState>,
    app: AppHandle,
    sheet_id: i64,
    sku_id: i64,
) -> AppResult<SheetDetail> {
    ensure_draft(&state.db, sheet_id).await?;
    let sheet = planning::get_sheet(&state.db, sheet_id)
        .await?
        .ok_or_else(|| AppError::InvalidInput("任务单不存在".into()))?;
    let set_id =
        sqlx::query_scalar::<_, i64>("SELECT id FROM daily_sets WHERE date = ?1 AND sku_id = ?2")
            .bind(&sheet.date)
            .bind(sku_id)
            .fetch_optional(&state.db)
            .await?
            .ok_or_else(|| AppError::InvalidInput("该 SKU 当日无套装".into()))?;

    // 用新 seed 重选（避免选回同一套）。
    let platform = sqlx::query_scalar::<_, String>(
        "SELECT platform FROM publish_tasks WHERE set_id = ?1 LIMIT 1",
    )
    .bind(set_id)
    .fetch_optional(&state.db)
    .await?
    .unwrap_or_else(|| "general".into());
    let pick = pick_set_for(&state, sku_id, &platform).await?;
    // 两条 UPDATE 收进一个事务：中断在两者之间会留下 content_kind 与套装包类型不一致的行
    // （套装换成了图集包，任务行还写着「视频」）。
    let mut tx = state.db.begin().await?;
    sqlx::query("UPDATE daily_sets SET pack_id = ?2, title_id = ?3, body_id = ?4 WHERE id = ?1")
        .bind(set_id)
        .bind(pick.pack_id)
        .bind(pick.title_id)
        .bind(pick.body_id)
        .execute(&mut *tx)
        .await?;
    // content_kind 可能随包类型变（video↔gallery）→ 同步任务行。
    sqlx::query("UPDATE publish_tasks SET content_kind = ?2, updated_at = ?3 WHERE set_id = ?1")
        .bind(set_id)
        .bind(&pick.content_kind)
        .bind(crate::db::now_unix())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    emit_changed(&app, &state.db, sheet_id).await;
    build_detail(&state, sheet_id).await
}

/// 为一个 SKU 即时选取一套内容（增补/换套装用）。
async fn pick_set_for(
    state: &AppState,
    sku_id: i64,
    platform: &str,
) -> AppResult<set_picker::SetPick> {
    let settings = publish_settings::load(&state.db).await?;
    let packs = assets::list_by_sku(&state.db, sku_id).await?;
    let mut pack_cands = Vec::new();
    for p in &packs {
        let last = ledger::pack_platform_last(&state.db, p.id).await?;
        pack_cands.push(set_picker::PackCand {
            id: p.id,
            kind: p.kind.clone(),
            lifecycle: p.lifecycle.clone(),
            last_pub: last,
        });
    }
    let mut conn = state.db.acquire().await?;
    let titles = texts::list_enabled(&mut conn, sku_id, "title").await?;
    let bodies = texts::list_enabled(&mut conn, sku_id, "body").await?;
    drop(conn);
    // 目标平台以传入平台优先，回退该 SKU 生效平台。
    let target = if platform == "general" {
        vec![]
    } else {
        vec![platform.to_string()]
    };
    let input = set_picker::PickInput {
        packs: pack_cands,
        titles: titles
            .iter()
            .map(|r| set_picker::TextCand {
                id: r.id,
                platform: r.platform.clone(),
                use_count: r.use_count,
            })
            .collect(),
        bodies: bodies
            .iter()
            .map(|r| set_picker::TextCand {
                id: r.id,
                platform: r.platform.clone(),
                use_count: r.use_count,
            })
            .collect(),
        target_platforms: target,
        dedup_days: settings.dedup_days,
        now: crate::db::now_unix(),
        // 秒级 seed 让重选与初次不同。
        seed: (crate::db::now_unix() as u64) ^ (sku_id as u64).wrapping_mul(0x9E37_79B9),
    };
    set_picker::pick(&input)
        .map_err(|e| AppError::InvalidInput(format!("换套装失败：{}", e.label())))
}

/// 导出预检（纯读）：素材齐备 / 路径长度 / 账号在用 / 重导出回执保护。
/// 前端在导出确认弹窗打开时调用，逐条渲染；有 error 时禁用「确认导出」。
#[tauri::command]
#[specta::specta]
pub async fn preflight_export(
    state: State<'_, AppState>,
    sheet_id: i64,
) -> AppResult<crate::publish::exporter::PreflightReport> {
    let settings = publish_settings::load(&state.db).await?;
    crate::publish::exporter::preflight(&state.db, sheet_id, &settings).await
}

/// 导出任务包（confirmed → exported；重导出=整包覆盖，但已有回执时被预检拒绝）。
#[tauri::command]
#[specta::specta]
pub async fn export_package(
    state: State<'_, AppState>,
    app: AppHandle,
    sheet_id: i64,
) -> AppResult<crate::publish::exporter::ExportResult> {
    let settings = publish_settings::load(&state.db).await?;
    // 每复制完一个文件推一次进度（文件数至多数百，无需节流）。
    let handle = app.clone();
    let progress: crate::publish::exporter::ProgressFn = std::sync::Arc::new(move |done, total| {
        let _ = ExportProgressEvent {
            sheet_id,
            done,
            total,
        }
        .emit(&handle);
    });
    let res =
        crate::publish::exporter::export_package(&state.db, sheet_id, &settings, Some(progress))
            .await?;
    emit_changed(&app, &state.db, sheet_id).await;
    Ok(res)
}

/// 在文件管理器中打开任务包目录。
#[tauri::command]
#[specta::specta]
pub async fn open_package_dir(
    state: State<'_, AppState>,
    app: AppHandle,
    sheet_id: i64,
) -> AppResult<()> {
    use tauri_plugin_opener::OpenerExt;
    let sheet = planning::get_sheet(&state.db, sheet_id)
        .await?
        .ok_or_else(|| AppError::InvalidInput("任务单不存在".into()))?;
    let root = publish_settings::root_local(&state.db).await?;
    let yyyymmdd: String = sheet.date.chars().filter(|c| c.is_ascii_digit()).collect();
    let dir = crate::publish::paths::RelPath::from_parts([
        crate::publish::paths::TASK_PACKAGES,
        &yyyymmdd,
    ])
    .to_local(&root);
    std::fs::create_dir_all(&dir)?;
    app.opener()
        .open_path(dir.to_string_lossy().to_string(), None::<&str>)
        .map_err(|e| AppError::Io(e.to_string()))
}

/// 内置「通用」以外的可排期 SKU（增补行选择器用）。
#[tauri::command]
#[specta::specta]
pub async fn list_schedulable_skus(
    state: State<'_, AppState>,
) -> AppResult<Vec<crate::commands::publish_skus::SkuView>> {
    // 复用 list_skus 的视图，过滤在售非通用。
    crate::commands::publish_skus::list_skus(
        state,
        crate::commands::publish_skus::SkuFilter {
            tier: None,
            warn_only: None,
            status: Some("active".into()),
            query: None,
        },
    )
    .await
    .map(|v| v.into_iter().filter(|s| !s.is_general).collect())
}
