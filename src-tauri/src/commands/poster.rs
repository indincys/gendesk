use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, Manager, State};
use tauri_specta::Event;

use crate::error::{AppError, AppResult};
use crate::poster::{self, PosterBatch, PosterChoice, PosterPreview, PosterSettings};
use crate::state::AppState;

#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct PosterChanged {
    pub at: i64,
}

pub fn emit(app: &AppHandle) {
    if let Err(error) = (PosterChanged {
        at: crate::db::now_unix(),
    })
    .emit(app)
    {
        tracing::warn!(%error, "发送 Poster 图片交接状态失败");
    }
}

pub fn start_delivery(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let _guard = state.poster_lock.lock().await;
        if let Err(error) = poster::deliver_pending(&state.db, &state.dirs.root).await {
            tracing::error!(%error, "处理 Poster 图片交接失败");
        }
        emit(&app);
    });
}

#[tauri::command]
#[specta::specta]
pub async fn get_poster_settings(state: State<'_, AppState>) -> AppResult<PosterSettings> {
    poster::load_settings(&state.db).await
}

#[tauri::command]
#[specta::specta]
pub async fn update_poster_settings(
    state: State<'_, AppState>,
    app: AppHandle,
    settings: PosterSettings,
) -> AppResult<PosterSettings> {
    let _guard = state.review_lock.lock().await;
    if !std::path::Path::new(&settings.executable).is_absolute() {
        return Err(AppError::InvalidInput("Poster 应用必须使用绝对路径".into()));
    }
    crate::db::repo::settings::set_by_key(&state.db, "poster", &serde_json::to_string(&settings)?)
        .await?;
    emit(&app);
    if settings.auto_import {
        start_delivery(app);
    }
    Ok(settings)
}

#[tauri::command]
#[specta::specta]
pub async fn list_poster_batches(state: State<'_, AppState>) -> AppResult<Vec<PosterBatch>> {
    poster::batches(&state.db).await
}

/// 失败批次的款式与可选商品（「指定商品」用）。
#[tauri::command]
#[specta::specta]
pub async fn preview_poster_batch(
    state: State<'_, AppState>,
    batch_id: String,
) -> AppResult<PosterPreview> {
    poster::preview(&state.db, &batch_id).await
}

/// 为匹配不上的款式指定商品：保存绑定、重新排队并立即续导。
#[tauri::command]
#[specta::specta]
pub async fn bind_poster_batch(
    state: State<'_, AppState>,
    app: AppHandle,
    batch_id: String,
    choices: Vec<PosterChoice>,
) -> AppResult<()> {
    {
        let _guard = state.poster_lock.lock().await;
        poster::bind(&state.db, &batch_id, &choices).await?;
    }
    emit(&app);
    start_delivery(app);
    Ok(())
}

/// 放弃导入一个失败批次。
#[tauri::command]
#[specta::specta]
pub async fn skip_poster_batch(
    state: State<'_, AppState>,
    app: AppHandle,
    batch_id: String,
) -> AppResult<()> {
    poster::skip(&state.db, &batch_id).await?;
    emit(&app);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn retry_poster_imports(app: AppHandle) -> AppResult<()> {
    start_delivery(app);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn pick_poster_app(app: AppHandle) -> AppResult<Option<String>> {
    use tauri_plugin_dialog::DialogExt;
    let picked = app
        .dialog()
        .file()
        .set_title("选择 Poster 应用")
        .blocking_pick_file();
    Ok(picked.and_then(|p| p.into_path().ok()).map(|path| {
        let path = if path.extension().is_some_and(|ext| ext == "app") {
            path.join("Contents/MacOS/Poster")
        } else {
            path
        };
        path.to_string_lossy().into()
    }))
}
