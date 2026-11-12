//! 参考图库命令（薄壳）：规则在 `crate::ref_library`，CLI 控制通道调用同一组函数。

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, State};
use tauri_specta::Event;

use crate::error::AppResult;
use crate::ref_library::{
    self as lib, LibraryImagePatch, LibraryImageView, LibraryImportReport, LibraryOverview,
    LibraryPickResult, LibraryUsageView,
};
use crate::state::AppState;

/// 参考图库在应用界面之外变了（Agent 经 CLI 导入、改图、抽图记账，或工单收录抽中图）。
///
/// 只是一声「该重读了」：载荷不带数据，图库页收到后自己节流重读，避免把上万行塞进事件。
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct LibraryChanged {
    pub at: i64,
}

pub fn emit_changed(app: &AppHandle) {
    if let Err(error) = (LibraryChanged {
        at: crate::db::now_unix(),
    })
    .emit(app)
    {
        tracing::warn!(%error, "发送参考图库变化事件失败");
    }
}

#[tauri::command]
#[specta::specta]
pub async fn library_overview(state: State<'_, AppState>) -> AppResult<LibraryOverview> {
    lib::overview(&state.db, crate::db::now_unix()).await
}

#[tauri::command]
#[specta::specta]
pub async fn list_library_images(
    state: State<'_, AppState>,
    style_id: Option<i64>,
    category_id: Option<i64>,
    include_removed: bool,
) -> AppResult<Vec<LibraryImageView>> {
    lib::list_images(
        &state.db,
        style_id,
        category_id,
        include_removed,
        crate::db::now_unix(),
    )
    .await
}

#[tauri::command]
#[specta::specta]
pub async fn save_library_style(
    state: State<'_, AppState>,
    id: Option<i64>,
    name: String,
    product: String,
    note: String,
) -> AppResult<i64> {
    lib::save_style(&state.db, id, &name, &product, &note).await
}

#[tauri::command]
#[specta::specta]
pub async fn delete_library_style(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    lib::delete_style(&state.db, id).await
}

#[tauri::command]
#[specta::specta]
pub async fn save_library_category(
    state: State<'_, AppState>,
    id: Option<i64>,
    name: String,
    cooldown_days: i64,
    sort_order: Option<i64>,
) -> AppResult<i64> {
    lib::save_category(&state.db, id, &name, cooldown_days, sort_order).await
}

#[tauri::command]
#[specta::specta]
pub async fn delete_library_category(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    lib::delete_category(&state.db, id).await
}

/// 导入本地图片到某款式某分类；逐张推 `refs://import-progress`（与生成页上传同一事件）。
#[tauri::command]
#[specta::specta]
pub async fn import_library_images(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    style_id: i64,
    category_id: i64,
    paths: Vec<String>,
) -> AppResult<LibraryImportReport> {
    let emitter = app.clone();
    let progress = move |done: i64, total: i64, name: &str, failed: i64| {
        crate::commands::refs::emit_progress(&emitter, done, total, name, "running", failed);
    };
    let report = lib::import_files(
        &state.db,
        &state.dirs,
        style_id,
        category_id,
        &paths,
        &progress,
    )
    .await;
    let (total, failed) = report
        .as_ref()
        .map(|r| (paths.len() as i64, r.failed.len() as i64))
        .unwrap_or((paths.len() as i64, 0));
    crate::commands::refs::emit_progress(&app, total, total, "", "done", failed);
    report
}

#[tauri::command]
#[specta::specta]
pub async fn update_library_images(
    state: State<'_, AppState>,
    ids: Vec<i64>,
    patch: LibraryImagePatch,
) -> AppResult<i64> {
    lib::update_images(&state.db, &ids, &patch).await
}

#[tauri::command]
#[specta::specta]
pub async fn pick_library_images(
    state: State<'_, AppState>,
    style_id: i64,
    category_id: i64,
    count: i64,
    mark_used: bool,
) -> AppResult<LibraryPickResult> {
    lib::pick(
        &state.db,
        style_id,
        category_id,
        usize::try_from(count).unwrap_or(0),
        mark_used,
        crate::db::now_unix(),
        &mut lib::os_rng,
    )
    .await
}

#[tauri::command]
#[specta::specta]
pub async fn mark_library_used(state: State<'_, AppState>, ids: Vec<i64>) -> AppResult<i64> {
    lib::mark_used(&state.db, &ids, crate::db::now_unix()).await
}

#[tauri::command]
#[specta::specta]
pub async fn library_image_usage(
    state: State<'_, AppState>,
    id: i64,
) -> AppResult<Vec<LibraryUsageView>> {
    lib::usage_of_image(&state.db, id).await
}
