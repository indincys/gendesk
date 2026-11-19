//! 本地图片和视频的预览、打开与原件导出。

use std::path::{Path, PathBuf};

use serde::Serialize;
use specta::Type;
use tauri::{AppHandle, Manager};
use tauri_plugin_opener::OpenerExt;

use crate::error::{AppError, AppResult};
use crate::files::copy::copy_new;

fn media_path(path: &str) -> AppResult<&Path> {
    let path = Path::new(path);
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default();
    if !path.is_absolute()
        || !matches!(
            extension.to_ascii_lowercase().as_str(),
            "jpg"
                | "jpeg"
                | "png"
                | "webp"
                | "bmp"
                | "gif"
                | "heic"
                | "tif"
                | "tiff"
                | "mp4"
                | "mov"
                | "m4v"
                | "webm"
        )
    {
        return Err(AppError::InvalidInput("无法识别该图片或视频路径".into()));
    }
    Ok(path)
}

#[tauri::command]
#[specta::specta]
pub async fn open_media(app: AppHandle, path: String) -> AppResult<()> {
    let path = media_path(&path)?;
    if !path.is_file() {
        return Err(AppError::InvalidInput("原文件不存在或无法读取".into()));
    }
    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(|error| AppError::Io(error.to_string()))
}

#[tauri::command]
#[specta::specta]
pub async fn preview_library_original(
    app: AppHandle,
    state: tauri::State<'_, crate::state::AppState>,
    id: i64,
) -> AppResult<String> {
    let path: Option<String> = sqlx::query_scalar(
        "SELECT r.file_path FROM ref_library_images i JOIN ref_images r ON r.id=i.ref_image_id WHERE i.id=?",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?;
    let path = path.ok_or_else(|| AppError::InvalidInput("参考图库条目不存在".into()))?;
    if !media_path(&path)?.is_file() {
        return Err(AppError::InvalidInput("原文件已移动或无法读取".into()));
    }
    app.asset_protocol_scope()
        .allow_file(&path)
        .map_err(|error| AppError::Io(error.to_string()))?;
    Ok(path)
}

#[derive(Debug, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MediaExportItem {
    pub source_path: String,
    pub output_path: Option<String>,
    pub error: Option<String>,
}

pub(crate) fn export_to_unique(source: &Path, dest: &Path, name: &str) -> std::io::Result<PathBuf> {
    let base = Path::new(name);
    let stem = base.file_stem().and_then(|x| x.to_str()).unwrap_or("asset");
    let extension = base
        .extension()
        .and_then(|x| x.to_str())
        .unwrap_or_default();
    for n in 0..100_000 {
        let candidate = if n == 0 {
            dest.join(name)
        } else {
            dest.join(format!("{stem}_{n}.{extension}"))
        };
        match copy_new(source, &candidate) {
            Ok(_) => return Ok(candidate),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "同名文件过多",
    ))
}

#[tauri::command]
#[specta::specta]
pub async fn export_media_files(
    paths: Vec<String>,
    dest_dir: String,
) -> AppResult<Vec<MediaExportItem>> {
    tokio::task::spawn_blocking(move || {
        let dest = Path::new(&dest_dir);
        if !dest.is_absolute() || !dest.is_dir() {
            return Err(AppError::InvalidInput("导出文件夹不存在或无法访问".into()));
        }
        Ok(paths
            .into_iter()
            .map(|path| {
                let outcome = (|| {
                    let source = media_path(&path)?;
                    if !source.is_file() {
                        return Err(AppError::InvalidInput("源文件不存在或无法访问".into()));
                    }
                    let name = source
                        .file_name()
                        .and_then(|x| x.to_str())
                        .ok_or_else(|| AppError::InvalidInput("文件名无法读取".into()))?;
                    export_to_unique(source, dest, name).map_err(AppError::from)
                })();
                let (output_path, error) = match outcome {
                    Ok(output) => (Some(output.to_string_lossy().into_owned()), None),
                    Err(error) => (None, Some(error.to_string())),
                };
                MediaExportItem {
                    source_path: path,
                    output_path,
                    error,
                }
            })
            .collect())
    })
    .await
    .map_err(|error| AppError::Io(error.to_string()))?
}
