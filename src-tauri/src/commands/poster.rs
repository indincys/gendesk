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

/// 某一次提交下还在收图的图文：有图移进废纸篓或生成失败后，这一组可能已经审完，就地组成。
/// 只碰这一批的图文，不顺带重试别的失败批次（生成失败可能一下来很多条）。
pub fn settle_sets(app: AppHandle, gendesk_batch: i64) {
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let _guard = state.poster_lock.lock().await;
        if let Err(error) = poster::settle_sets_of(&state.db, &state.dirs.root, gendesk_batch).await
        {
            tracing::error!(%error, "结算 Poster 图文失败");
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
    let awaiting: bool = {
        let _guard = state.poster_lock.lock().await;
        poster::bind(&state.db, &batch_id, &choices).await?;
        sqlx::query_scalar("SELECT state='awaiting' FROM poster_batches WHERE id=?")
            .bind(&batch_id)
            .fetch_one(&state.db)
            .await?
    };
    emit(&app);
    if !awaiting {
        start_delivery(app);
    }
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
    let _guard = state.poster_lock.lock().await;
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

#[tauri::command]
#[specta::specta]
pub async fn get_poster_draft(
    state: State<'_, AppState>,
    batch_id: String,
) -> AppResult<poster::draft::DeliveryDraft> {
    let _guard = state.poster_lock.lock().await;
    poster::draft::read(&state.db, &batch_id).await
}

#[tauri::command]
#[specta::specta]
pub async fn save_poster_draft(
    state: State<'_, AppState>,
    app: AppHandle,
    batch_id: String,
    revision: i64,
    images: Vec<poster::draft::DraftPlacement>,
    confirm: bool,
) -> AppResult<()> {
    {
        let _review = state.review_lock.lock().await;
        let _poster = state.poster_lock.lock().await;
        poster::draft::save(&state.db, &batch_id, revision, &images, confirm).await?;
    }
    emit(&app);
    if confirm {
        deliver_batch(app, batch_id);
    }
    Ok(())
}

fn deliver_batch(app: AppHandle, batch_id: String) {
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let _guard = state.poster_lock.lock().await;
        if let Err(error) = poster::retry_one(&state.db, &state.dirs.root, &batch_id).await {
            tracing::warn!(%error, batch=%batch_id, "本批 Poster 交付待处理");
        }
        emit(&app);
    });
}

#[tauri::command]
#[specta::specta]
pub async fn retry_poster_batch(
    state: State<'_, AppState>,
    app: AppHandle,
    batch_id: String,
) -> AppResult<()> {
    let _guard = state.poster_lock.lock().await;
    let result = poster::retry_one(&state.db, &state.dirs.root, &batch_id).await;
    emit(&app);
    result
}

#[tauri::command]
#[specta::specta]
pub async fn export_poster_draft(
    state: State<'_, AppState>,
    batch_id: String,
    dest_dir: String,
    static_only: bool,
    revision: i64,
) -> AppResult<poster::draft::DraftExport> {
    let draft = {
        let _review = state.review_lock.lock().await;
        let _guard = state.poster_lock.lock().await;
        let draft = poster::draft::read(&state.db, &batch_id).await?;
        if draft.revision != revision {
            return Err(AppError::InvalidInput(
                "草稿内容已变化，请重新读取后再导出".into(),
            ));
        }
        draft
    };
    tokio::task::spawn_blocking(move || {
        poster::draft::export(draft, std::path::Path::new(&dest_dir), static_only)
    })
    .await
    .map_err(|error| AppError::Io(error.to_string()))?
}
