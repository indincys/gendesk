//! 任务单编排域命令（发布模块执行计划 4.1 planning 域）。

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, State};
use tauri_specta::Event;

use crate::commands::publish_settings;
use crate::db::repo::{accounts, assets, ledger, planning, texts};
use crate::error::{AppError, AppResult};
use crate::publish::events::SheetChangedEvent;
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
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SheetDetail {
    pub id: i64,
    pub date: String,
    pub status: String,
    pub shortage: Vec<ShortageItem>,
    pub rows: Vec<TaskRowView>,
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
            }
        })
        .collect();
    let shortage: Vec<ShortageItem> =
        serde_json::from_str(&sheet.shortage_json).unwrap_or_default();
    Ok(SheetDetail {
        id: sheet.id,
        date: sheet.date,
        status: sheet.status,
        shortage,
        rows,
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
    let mut out = Vec::new();
    for s in planning::list_sheets(&state.db).await? {
        out.push(build_summary(&state.db, s).await?);
    }
    Ok(out)
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
        planning::update_task_time(&state.db, id, pt.as_deref()).await?;
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn cancel_task_row(state: State<'_, AppState>, app: AppHandle, id: i64) -> AppResult<()> {
    let task = planning::get_task(&state.db, id)
        .await?
        .ok_or_else(|| AppError::InvalidInput("任务行不存在".into()))?;
    planning::set_task_status(&state.db, id, "canceled").await?;
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
    let sheet = planning::get_sheet(&state.db, input.sheet_id)
        .await?
        .ok_or_else(|| AppError::InvalidInput("任务单不存在".into()))?;
    let account = accounts::list(&state.db)
        .await?
        .into_iter()
        .find(|a| a.id == input.account_id)
        .ok_or_else(|| AppError::InvalidInput("账号不存在".into()))?;

    // 找该 SKU 当日套装；无则即时选取并落套装。
    let existing_set =
        sqlx::query_scalar::<_, i64>("SELECT id FROM daily_sets WHERE date = ?1 AND sku_id = ?2")
            .bind(&sheet.date)
            .bind(input.sku_id)
            .fetch_optional(&state.db)
            .await?;

    let (set_id, content_kind) = match existing_set {
        Some(sid) => {
            let ds = planning::get_daily_set(&state.db, sid)
                .await?
                .ok_or_else(|| AppError::Internal("套装读取失败".into()))?;
            let pack = assets::get(&state.db, ds.pack_id).await?;
            let kind = pack.map(|p| p.kind).unwrap_or_else(|| "video".into());
            (sid, kind)
        }
        None => {
            let pick = pick_set_for(&state, input.sku_id, &account.platform).await?;
            let mut conn = state.db.acquire().await?;
            let sid = planning::insert_daily_set(
                &mut conn,
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
    };

    let yy: String = sheet
        .date
        .chars()
        .filter(|c| c.is_ascii_digit())
        .skip(2)
        .collect();
    let next = planning::max_task_seq(&state.db, input.sheet_id, &yy).await? + 1;
    let task_code = format!("T{yy}-{next:03}");
    let mut conn = state.db.acquire().await?;
    planning::insert_publish_task(
        &mut conn,
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
    drop(conn);
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
    sqlx::query("UPDATE daily_sets SET pack_id = ?2, title_id = ?3, body_id = ?4 WHERE id = ?1")
        .bind(set_id)
        .bind(pick.pack_id)
        .bind(pick.title_id)
        .bind(pick.body_id)
        .execute(&state.db)
        .await?;
    // content_kind 可能随包类型变（video↔gallery）→ 同步任务行。
    sqlx::query("UPDATE publish_tasks SET content_kind = ?2, updated_at = ?3 WHERE set_id = ?1")
        .bind(set_id)
        .bind(&pick.content_kind)
        .bind(crate::db::now_unix())
        .execute(&state.db)
        .await?;
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
