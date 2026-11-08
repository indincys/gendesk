//! 将已验收原图交给本机实况图工具，逐张运行并成对归档。

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use specta::Type;
use sqlx::{Sqlite, SqlitePool, Transaction};
use tauri::{AppHandle, Manager, State};
use tauri_plugin_opener::OpenerExt;
use tauri_specta::Event;
use tokio::process::Command;

use crate::db::repo::settings;
use crate::error::{AppError, AppResult};
use crate::library::Organization;
use crate::state::AppState;

const ENABLED_KEY: &str = "livePhotoEnabled";
const FOLDER: &str = "实况照片";

#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LivePhotoStatus {
    supported: bool,
    enabled: bool,
    pending: i64,
    failed: i64,
    error: Option<String>,
}

#[derive(Clone, Serialize, Type, Event)]
pub struct LivePhotoChanged {
    error: Option<String>,
}

#[derive(sqlx::FromRow)]
struct Job {
    id: i64,
    image_path: String,
    output_dir: String,
    batch_id: Option<i64>,
    batch_note: Option<String>,
    organization_json: Option<String>,
    group_name: Option<String>,
}

impl Job {
    fn directory(&self) -> AppResult<PathBuf> {
        let out = Path::new(&self.output_dir);
        let Some(batch_id) = self.batch_id else {
            return Ok(out.to_path_buf());
        };
        let live_root = Path::new("成品").join(FOLDER);
        let direct_root = out.parent().filter(|parent| parent.ends_with(&live_root));
        // 最早的队列按产品归属保存路径，通过原图和旧落点的公共「成品」找到实况根。
        let legacy_root = || {
            let image_dir = Path::new(&self.image_path).parent()?;
            out.ancestors()
                .zip(image_dir.ancestors())
                .find(|(candidate, source)| {
                    candidate.ends_with(&live_root)
                        && source.ends_with(Path::new("成品").join("生成图片"))
                        && candidate.parent() == source.parent()
                })
                .map(|(candidate, _)| candidate)
        };
        let suffix = format!("_{batch_id}");
        let named = out
            .file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|name| name.ends_with(&suffix) && name != format!("生成批次{suffix}"));
        let Some(root) = direct_root.or_else(legacy_root) else {
            return Ok(out.to_path_buf());
        };
        // 已落盘的目录以批次编号认领，避免多次验收、改名或旧队列恢复时拆成两个目录。
        let entries = match std::fs::read_dir(root) {
            Ok(entries) => Some(entries),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        let mut existing = None;
        for entry in entries.into_iter().flatten() {
            let entry = entry?;
            if entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.ends_with(&suffix))
                && entry.file_type()?.is_dir()
                && existing.replace(entry.path()).is_some()
            {
                return Err(AppError::Io(format!(
                    "找到多个实况图目录对应批次 {batch_id}"
                )));
            }
        }
        if let Some(path) = existing {
            return Ok(path);
        }
        if direct_root.is_some() && named {
            return Ok(out.to_path_buf());
        }
        let organization = self
            .organization_json
            .as_deref()
            .and_then(|json| serde_json::from_str(json).ok());
        let title = submission_title(
            self.batch_note.as_deref(),
            [(organization, self.group_name.as_deref().unwrap_or_default())],
        );
        Ok(root.join(batch_name(batch_id, &title)))
    }
}

fn pair_paths(dir: &Path, stem: &OsStr) -> [PathBuf; 2] {
    [".JPG", ".MOV"].map(|ext| {
        let mut filename = stem.to_os_string();
        filename.push(ext);
        dir.join(filename)
    })
}

fn has_pair(paths: &[PathBuf; 2]) -> bool {
    paths.iter().all(|path| {
        path.symlink_metadata()
            .is_ok_and(|file| file.is_file() && file.len() > 0)
    })
}

fn publish_pair(staged: [PathBuf; 2], published: [PathBuf; 2]) -> AppResult<()> {
    let mut created = crate::publish::paths::CreatedFilesGuard::new([]);
    for (source, target) in staged.into_iter().zip(published) {
        match std::fs::hard_link(&source, &target) {
            Ok(()) => created.track(target),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if !target.symlink_metadata()?.is_file()
                    || std::fs::read(&source)? != std::fs::read(&target)?
                {
                    return Err(AppError::Io(format!(
                        "实况照片文件已被占用 {}",
                        target.display()
                    )));
                }
            }
            Err(error) => return Err(error.into()),
        }
    }
    created.preserve_all();
    Ok(())
}

struct Tool {
    python: PathBuf,
    script: PathBuf,
    path: std::ffi::OsString,
}

impl Tool {
    fn local() -> AppResult<Self> {
        if !cfg!(target_os = "macos") {
            return Err(AppError::InvalidInput("本机实况图工具仅支持 macOS".into()));
        }
        let dir = crate::poster::home()
            .join("Workspace/产品/明星日历_2027/图文带货创作/制作工具/实况图工具");
        let python = dir.join(".venv/bin/python");
        let script = dir.join("make_live.py");
        for path in [&python, &script] {
            if !path.is_file() {
                return Err(AppError::InvalidInput(format!(
                    "找不到实况图工具文件 {}",
                    path.display()
                )));
            }
        }

        // Finder 启动时的 PATH 通常不含 Homebrew，脚本靠 PATH 调用 ffmpeg。
        let inherited = std::env::var_os("PATH").unwrap_or_default();
        let dirs: Vec<_> = std::env::split_paths(&inherited)
            .filter(|path| path.is_absolute())
            .chain(
                [
                    "/opt/homebrew/bin",
                    "/usr/local/bin",
                    "/opt/local/bin",
                    "/usr/bin",
                    "/bin",
                ]
                .map(PathBuf::from),
            )
            .collect();
        if !dirs.iter().any(|dir| dir.join("ffmpeg").is_file()) {
            return Err(AppError::InvalidInput(
                "找不到实况图工具所需的 ffmpeg".into(),
            ));
        }
        let path = std::env::join_paths(dirs).map_err(|e| AppError::Io(e.to_string()))?;
        Ok(Self {
            python,
            script,
            path,
        })
    }

    async fn convert(&self, image: &Path, out: &Path) -> AppResult<()> {
        let stem = image
            .file_stem()
            .ok_or_else(|| AppError::InvalidInput("原图缺少文件名".into()))?;
        let published = pair_paths(out, stem);
        let mut ready_name = std::ffi::OsString::from(".实况待发布-");
        ready_name.push(stem);
        let ready = out.join(ready_name);
        let staged = pair_paths(&ready, stem);
        // 隐藏目录整体转为待发布。若退出时只链接了一半，重启时补上同一套配对。
        if has_pair(&staged) {
            publish_pair(staged, published)?;
            std::fs::remove_dir_all(ready)?;
            return Ok(());
        }
        if has_pair(&published) {
            return Ok(());
        }
        if let Some(conflict) = published
            .iter()
            .find(|path| path.symlink_metadata().is_ok())
        {
            return Err(AppError::Io(format!(
                "实况照片文件已被占用 {}",
                conflict.display()
            )));
        }
        if !image.is_file() {
            return Err(AppError::Io(format!(
                "找不到已验收原图 {}",
                image.display()
            )));
        }
        std::fs::create_dir_all(out)?;
        let staging = tempfile::Builder::new()
            .prefix(".实况转换-")
            .tempdir_in(out)?;
        let output = tokio::time::timeout(
            Duration::from_secs(600),
            Command::new(&self.python)
                .arg(&self.script)
                .arg(staging.path())
                .arg(image)
                .env("PATH", &self.path)
                .kill_on_drop(true)
                .output(),
        )
        .await
        .map_err(|_| AppError::Timeout("实况图转换超过 10 分钟".into()))??;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let detail = stderr.lines().rev().find(|line| !line.trim().is_empty());
            tracing::warn!(image = %image.display(), %stderr, "实况图脚本退出失败");
            return Err(AppError::Io(match detail {
                Some(line) => line.chars().take(300).collect(),
                None => format!("实况图脚本退出 {}", output.status),
            }));
        }
        if !has_pair(&pair_paths(staging.path(), stem)) {
            return Err(AppError::Io(
                "实况图脚本没有生成完整的 JPG 与 MOV 配对".into(),
            ));
        }
        std::fs::rename(staging.path(), &ready)?;
        publish_pair(staged, published)?;
        std::fs::remove_dir_all(ready)?;
        Ok(())
    }
}

pub async fn enabled(pool: &SqlitePool) -> AppResult<bool> {
    Ok(cfg!(target_os = "macos")
        && settings::get_by_key(pool, ENABLED_KEY).await?.as_deref() == Some("true"))
}

pub async fn enqueue(
    tx: &mut Transaction<'_, Sqlite>,
    image: &Path,
    output_dir: &Path,
) -> AppResult<()> {
    sqlx::query("INSERT INTO live_photo_jobs(image_path, output_dir) VALUES (?, ?)")
        .bind(image.to_string_lossy().as_ref())
        .bind(output_dir.to_string_lossy().as_ref())
        .execute(&mut **tx)
        .await?;
    Ok(())
}

pub fn root(library_root: &Path) -> PathBuf {
    library_root.join("成品").join(FOLDER)
}

fn readable_title(value: &str) -> Option<&str> {
    let name = value.trim();
    let is_hex = |value: &str| value.bytes().all(|b| b.is_ascii_hexdigit());
    let cli_id = name
        .get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("cli-"))
        && name.len() == 36
        && is_hex(&name[4..]);
    let (stem, ext) = name.split_once('.').unwrap_or((name, ""));
    let hash = (40..=64).contains(&stem.len())
        && is_hex(stem)
        && ext.bytes().all(|b| b.is_ascii_alphanumeric());
    (!name.is_empty() && !cli_id && !hash).then_some(name)
}

// 与验收页「按提交」的标题同一规则，使用全批任务，避免标题随已验收图片消失而变化。
fn submission_title<'a>(
    note: Option<&str>,
    items: impl IntoIterator<Item = (Option<Organization>, &'a str)>,
) -> String {
    if let Some(note) = note.and_then(readable_title) {
        return note.to_owned();
    }
    let mut labels = BTreeSet::new();
    let mut groups = BTreeSet::new();
    for (organization, group) in items {
        if let Some(org) = organization {
            let label = [org.product, org.requirement]
                .into_iter()
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
                .join(" · ");
            if !label.is_empty() {
                labels.insert(label);
            }
        }
        groups.extend(readable_title(group));
    }
    match labels.len() {
        0 if groups.len() == 1 => groups.into_iter().next().unwrap_or_default().to_owned(),
        0 => "未命名提交".into(),
        1 => labels.into_iter().next().unwrap_or_default(),
        n => format!("含 {n} 项内容的提交"),
    }
}

fn batch_name(batch_id: i64, title: &str) -> String {
    let cleaned: String = crate::files::sanitize_filename(title)
        .replace('·', "_")
        .chars()
        .filter(|c| !c.is_whitespace() && !matches!(c, '.' | '。' | '(' | ')' | '（' | '）'))
        .take(48)
        .collect();
    let cleaned = cleaned.trim_matches('_');
    let name = if cleaned.is_empty() {
        "未命名提交"
    } else {
        cleaned
    };
    format!("{name}_{batch_id}")
}

pub async fn batch_dir(
    pool: &SqlitePool,
    library_root: &Path,
    batch_id: i64,
    note: Option<&str>,
) -> AppResult<PathBuf> {
    let title = match note.and_then(readable_title) {
        Some(title) => title.to_owned(),
        None => {
            let rows: Vec<(String, String)> = sqlx::query_as(
                "SELECT t.params_json, COALESCE(g.name,'') FROM tasks t
                LEFT JOIN prompts p ON p.id = t.prompt_id
                LEFT JOIN prompt_groups g ON g.id = p.group_id WHERE t.batch_id = ?",
            )
            .bind(batch_id)
            .fetch_all(pool)
            .await?;
            submission_title(
                None,
                rows.iter().map(|(params, group)| {
                    (
                        Organization::from_params(params).ok().flatten(),
                        group.as_str(),
                    )
                }),
            )
        }
    };
    Ok(root(library_root).join(batch_name(batch_id, &title)))
}

pub fn notify(state: &AppState, app: &AppHandle) {
    state.live_photo_wake.notify_one();
    let _ = LivePhotoChanged { error: None }.emit(app);
}

async fn next(pool: &SqlitePool) -> AppResult<Option<Job>> {
    Ok(sqlx::query_as("SELECT j.id, j.image_path, j.output_dir, w.batch_id, b.note AS batch_note,
        w.organization_json, w.group_name FROM live_photo_jobs j
        LEFT JOIN accepted_works w ON w.id = (SELECT id FROM accepted_works WHERE image_path = j.image_path ORDER BY id DESC LIMIT 1)
        LEFT JOIN batches b ON b.id = w.batch_id
        WHERE j.error IS NULL ORDER BY j.id LIMIT 1")
        .fetch_optional(pool).await?)
}

pub fn start(app: AppHandle) {
    if !cfg!(target_os = "macos") {
        return;
    }
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        loop {
            if let Err(error) = drain(&state.db, &app).await {
                tracing::warn!(%error, "实况图队列读取或更新失败");
                tokio::time::sleep(Duration::from_secs(5)).await;
            } else {
                state.live_photo_wake.notified().await;
            }
        }
    });
}

async fn drain(pool: &SqlitePool, app: &AppHandle) -> AppResult<()> {
    while let Some(job) = next(pool).await? {
        let result = async {
            Tool::local()?
                .convert(Path::new(&job.image_path), &job.directory()?)
                .await
        }
        .await;
        let error = match result {
            Ok(()) => {
                sqlx::query("DELETE FROM live_photo_jobs WHERE id = ?")
                    .bind(job.id)
                    .execute(pool)
                    .await?;
                None
            }
            Err(error) => {
                let message = error.to_string();
                tracing::warn!(image = %job.image_path, %error, "实况图转换失败");
                sqlx::query("UPDATE live_photo_jobs SET error = ? WHERE id = ?")
                    .bind(&message)
                    .bind(job.id)
                    .execute(pool)
                    .await?;
                Some(message)
            }
        };
        let _ = LivePhotoChanged { error }.emit(app);
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn get_live_photo_status(state: State<'_, AppState>) -> AppResult<LivePhotoStatus> {
    let (pending, failed, error) = sqlx::query_as(
        "SELECT COUNT(*) - COUNT(error), COUNT(error),
                (SELECT error FROM live_photo_jobs WHERE error IS NOT NULL ORDER BY id DESC LIMIT 1)
         FROM live_photo_jobs",
    )
    .fetch_one(&state.db)
    .await?;
    Ok(LivePhotoStatus {
        supported: cfg!(target_os = "macos"),
        enabled: enabled(&state.db).await?,
        pending,
        failed,
        error,
    })
}

#[tauri::command]
#[specta::specta]
pub async fn set_live_photo_enabled(state: State<'_, AppState>, enabled: bool) -> AppResult<()> {
    if enabled {
        Tool::local()?;
    }
    settings::set_by_key(
        &state.db,
        ENABLED_KEY,
        if enabled { "true" } else { "false" },
    )
    .await?;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn retry_live_photos(state: State<'_, AppState>, app: AppHandle) -> AppResult<()> {
    Tool::local()?;
    sqlx::query("UPDATE live_photo_jobs SET error = NULL WHERE error IS NOT NULL")
        .execute(&state.db)
        .await?;
    notify(&state, &app);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn open_live_photo_dir(state: State<'_, AppState>, app: AppHandle) -> AppResult<()> {
    let dir = root(&crate::library::root(&state.db).await?);
    std::fs::create_dir_all(&dir)?;
    app.opener()
        .open_path(dir.to_string_lossy().as_ref(), None::<&str>)
        .map_err(|e| AppError::Io(e.to_string()))
}

#[cfg(all(test, unix))]
#[allow(clippy::unwrap_used)] // 测试中的脚本和目录均是隔离夹具。
mod tests {
    use super::*;

    #[tokio::test]
    async fn incomplete_pair_is_hidden_and_completed_pair_is_reused() {
        let tmp = tempfile::tempdir().unwrap();
        let script = tmp.path().join("测试脚本");
        let image = tmp.path().join("验收 原图.jpg");
        let out = tmp.path().join("实况照片");
        let tool = Tool {
            python: "/bin/sh".into(),
            script: script.clone(),
            path: "/usr/bin:/bin".into(),
        };
        std::fs::write(&image, b"original").unwrap();
        let script_start = r#"name="${2##*/}"
stem="${name%.*}"
printf jpeg > "$1/$stem.JPG"
"#;
        std::fs::write(&script, script_start).unwrap();
        assert!(tool.convert(&image, &out).await.is_err());
        assert_eq!(std::fs::read_dir(&out).unwrap().count(), 0);

        std::fs::write(
            &script,
            format!("{script_start}printf video > \"$1/$stem.MOV\"\n"),
        )
        .unwrap();
        tool.convert(&image, &out).await.unwrap();
        // 两次独立转换直接写进同一批次目录。
        let second = tmp.path().join("第二张.png");
        std::fs::write(&second, b"second").unwrap();
        tool.convert(&second, &out).await.unwrap();
        assert_eq!(std::fs::read(out.join("验收 原图.JPG")).unwrap(), b"jpeg");
        assert_eq!(std::fs::read(out.join("验收 原图.MOV")).unwrap(), b"video");
        assert_eq!(std::fs::read(out.join("第二张.JPG")).unwrap(), b"jpeg");
        assert_eq!(std::fs::read(out.join("第二张.MOV")).unwrap(), b"video");
        assert_eq!(std::fs::read_dir(&out).unwrap().count(), 4);
        assert_eq!(std::fs::read(&image).unwrap(), b"original");
        std::fs::remove_file(&script).unwrap();
        tool.convert(&image, &out).await.unwrap();
    }

    #[tokio::test]
    async fn interrupted_publish_resumes_the_same_pair() {
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path();
        let image = out.join("原图.png");
        let ready = out.join(".实况待发布-原图");
        std::fs::create_dir(&ready).unwrap();
        std::fs::write(ready.join("原图.JPG"), b"jpeg").unwrap();
        std::fs::write(ready.join("原图.MOV"), b"video").unwrap();
        let tool = Tool {
            python: "missing".into(),
            script: "missing".into(),
            path: "/usr/bin".into(),
        };

        std::fs::write(out.join("原图.MOV"), b"collision").unwrap();
        assert!(tool.convert(&image, out).await.is_err());
        assert!(!out.join("原图.JPG").exists());
        assert_eq!(std::fs::read(out.join("原图.MOV")).unwrap(), b"collision");

        std::fs::remove_file(out.join("原图.MOV")).unwrap();
        std::fs::hard_link(ready.join("原图.JPG"), out.join("原图.JPG")).unwrap();
        tool.convert(&image, out).await.unwrap();
        assert_eq!(std::fs::read(out.join("原图.JPG")).unwrap(), b"jpeg");
        assert_eq!(std::fs::read(out.join("原图.MOV")).unwrap(), b"video");
        assert!(!ready.exists());
    }

    #[test]
    fn queued_paths_group_by_generation_batch_across_organizations() {
        let tmp = tempfile::tempdir().unwrap();
        let library = tmp.path();
        let expected = root(library).join("桌面补拍_37");
        for organization in ["产品甲/款式甲/需求甲", "产品乙/需求乙", "需求丙"] {
            let original = library
                .join("成品/生成图片")
                .join(organization)
                .join("原图.jpg");
            let old_dir = root(library).join(organization);
            let mut job = Job {
                id: 1,
                image_path: original.to_string_lossy().into(),
                output_dir: old_dir.to_string_lossy().into(),
                batch_id: Some(37),
                batch_note: Some("桌面补拍".into()),
                organization_json: None,
                group_name: None,
            };
            assert_eq!(job.directory().unwrap(), expected);
            job.output_dir = expected.to_string_lossy().into();
            assert_eq!(job.directory().unwrap(), expected);

            // 旧任务队列仍能找到手动改名后的同一批次。
            let renamed = root(library).join("补拍桌面_37");
            std::fs::create_dir_all(&renamed).unwrap();
            job.output_dir = root(library).join("生成批次_37").to_string_lossy().into();
            assert_eq!(job.directory().unwrap(), renamed);
            std::fs::remove_dir(renamed).unwrap();
        }
        assert_ne!(batch_name(37, "桌面补拍"), batch_name(38, "桌面补拍"));
    }

    #[test]
    fn hides_internal_job_ids_and_preserves_readable_batch_titles() {
        let items = [(
            Some(Organization::legacy(
                "明星日历2027",
                "鞠婧祎",
                "台历手机桌面场景",
            )),
            "台历桌景",
        )];
        let title = submission_title(Some("cli-d4a4f5a0df8d2ba49e1d954e596ff120"), items.clone());
        assert_eq!(batch_name(37, &title), "明星日历2027_台历手机桌面场景_37");
        let title = submission_title(Some("关联任务 358，只换桌面，改为清爽阅读角。"), items);
        assert_eq!(
            batch_name(42, &title),
            "关联任务358，只换桌面，改为清爽阅读角_42"
        );
    }

    #[tokio::test]
    async fn queue_uses_acceptance_transaction() {
        let tmp = tempfile::tempdir().unwrap();
        let pool = crate::db::connect(&tmp.path().join("test.db"))
            .await
            .unwrap();
        assert!(!enabled(&pool).await.unwrap());
        let image = tmp.path().join("验收图.jpg");
        let output = tmp.path().join("实况照片");
        let mut tx = pool.begin().await.unwrap();
        enqueue(&mut tx, &image, &output).await.unwrap();
        tx.rollback().await.unwrap();
        assert!(next(&pool).await.unwrap().is_none());
        let mut tx = pool.begin().await.unwrap();
        enqueue(&mut tx, &image, &output).await.unwrap();
        tx.commit().await.unwrap();
        let job = next(&pool).await.unwrap().unwrap();
        assert_eq!(Path::new(&job.image_path), image);
        assert_eq!(Path::new(&job.output_dir), output);
    }
}
