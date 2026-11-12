//! refs 域：参考图导入（执行计划 2.1 / 1.5）。拷入 refs/、生成 512px 缩略图、入库。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::State;
use tauri_specta::Event;

use crate::db::repo::refs as repo;
use crate::error::{AppError, AppResult};
use crate::files;
use crate::state::AppState;

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RefImageView {
    pub id: i64,
    pub name: String,
    pub file_path: String,
    pub thumb_path: String,
    pub width: i64,
    pub height: i64,
    /// 最近一次挂靠的提示词组（E32 挂靠记忆）；生成页据此预填挂靠。
    pub last_group_id: Option<i64>,
    /// 已归档（0016）：批次开跑后自动置位，生成页选择器默认折起。
    pub archived: bool,
    /// 生成页临时上传（0019）：不参与内容去重，生成页之外不列出。
    pub ephemeral: bool,
}

/// `refs://import-progress`：批量导入逐张进度。
///
/// 导入是「拷贝 + 解码 + 缩略图 + hash + 压缩副本」，单张几百毫秒起，一次十几张就是
/// 十几秒静默。没有这条事件，用户看到的是一个完全无响应的界面——于是反复重按上传，
/// 同一批图进库五六遍（这正是本次要修的现场）。
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct RefImportProgress {
    /// 已处理张数（含失败）。
    pub done: i64,
    pub total: i64,
    /// 当前正在处理的文件名（done 阶段为空串）。
    pub name: String,
    /// running / done
    pub phase: String,
    /// 失败张数（逐张容错：一张坏图不该中断整批）。
    pub failed: i64,
}

/// 在目录内生成不冲突的路径。
fn unique_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let first = if ext.is_empty() {
        dir.join(stem)
    } else {
        dir.join(format!("{stem}.{ext}"))
    };
    if !first.exists() {
        return first;
    }
    let mut n = 1;
    loop {
        let name = if ext.is_empty() {
            format!("{stem}_{n}")
        } else {
            format!("{stem}_{n}.{ext}")
        };
        let candidate = dir.join(name);
        if !candidate.exists() {
            return candidate;
        }
        n += 1;
    }
}

/// 单张落盘产物（拷贝 + 缩略图 + hash + 上传副本），入库前的纯文件侧结果。
///
/// `pub(crate)`：工单收件（`intake`）要走**同一条**落盘路径。参考图入库这件事必须
/// 只有一份实现——缩略图尺寸、hash 口径、上传压缩副本的阈值一旦分叉，
/// 「手动传的图能用、工单送来的图不能用」这类问题就没有统一答案。
pub(crate) struct Ingested {
    pub name: String,
    pub file_path: String,
    pub thumb_path: String,
    pub width: i64,
    pub height: i64,
    pub file_size: i64,
    pub content_hash: Option<String>,
    pub upload_path: Option<String>,
}

/// 把一张源图落进库目录（同步、CPU/IO 密集，调用方须经 `files::decode::bounded`）。
///
/// **一次读、一次解码。** 原来这里是 4 次整文件读 + 2 次全分辨率解码：
/// `fs::copy` 读一遍、`generate_thumbnail` 读+解一遍、`content_hash` 再读一遍、
/// `make_upload_copy` 又读+解一遍。对一份 120 张 26 MP 相机图的工单，那是
/// 约 3 GB 的重复 IO 加 240 次 26 MP 解码，全串行——确认一次要四分钟。
///
/// 现在：读一次进内存，哈希、落盘、量尺寸都用这份字节；解码只做一次，
/// 且直接解到「够用的最小档」（超限图解到 ≥2048，正好同时喂饱上传副本和缩略图）。
pub(crate) fn ingest_one(
    path: &str,
    refs_dir: &Path,
    thumbs_dir: &Path,
    permit: &files::decode::DecodePermit,
) -> AppResult<Ingested> {
    let src = PathBuf::from(path);
    let stem = src
        .file_stem()
        .and_then(|s| s.to_str())
        .map(files::sanitize_filename)
        .ok_or_else(|| AppError::InvalidInput(format!("无效文件路径：{path}")))?;
    let ext = src
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("png")
        .to_lowercase();

    let inspected = files::decode::inspect_image(&src, permit)?;
    let needs_copy = inspected.needs_upload_copy();
    let bytes = inspected.bytes;
    let (w, h) = (inspected.width, inspected.height);
    let img = inspected.image;
    let file_size = bytes.len() as i64;
    let content_hash = Some(files::content_hash_bytes(&bytes));
    let dest = unique_path(refs_dir, &stem, &ext);
    std::fs::write(&dest, &bytes)?;

    let thumb = unique_path(thumbs_dir, &stem, "jpg");
    files::write_thumbnail_from(&img, &thumb)?;

    // 原文件原样保存（含 EXIF，hash 口径不变）；发给上游的是摆正后的副本。
    let upload_path = if needs_copy {
        let upload_dest = unique_path(refs_dir, &format!("{stem}_up"), "jpg");
        match files::write_upload_copy_from(&img, &upload_dest) {
            Ok(()) => Some(upload_dest.to_string_lossy().to_string()),
            Err(_) => None,
        }
    } else {
        None
    };

    Ok(Ingested {
        name: stem,
        file_path: dest.to_string_lossy().to_string(),
        thumb_path: thumb.to_string_lossy().to_string(),
        width: w as i64,
        height: h as i64,
        file_size,
        content_hash,
        upload_path,
    })
}

/// 导入参考图：拷入库、生成缩略图、写记录，返回视图列表。
///
/// `ephemeral = true` 为生成页的临时上传（不进长期图库，见 0019）。
/// 全程逐张推 `refs://import-progress`：一张坏图只算失败一张，不中断整批。
#[tauri::command]
#[specta::specta]
pub async fn import_ref_images(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    paths: Vec<String>,
    ephemeral: bool,
) -> AppResult<Vec<RefImageView>> {
    state.dirs.init()?;
    let refs_dir = state.dirs.refs();
    let thumbs_dir = state.dirs.thumbs();
    let total = paths.len() as i64;
    let mut views = Vec::with_capacity(paths.len());
    let mut failed = 0i64;

    for (i, path) in paths.into_iter().enumerate() {
        let short = short_name(&path);
        // 开始处理这一张就先报一次：进度条在第一张的几百毫秒里也是动的。
        emit_progress(&app, i as i64, total, &short, "running", failed);

        let (rd, td) = (refs_dir.clone(), thumbs_dir.clone());
        let p = path.clone();
        // 解码 + 缩放 + 重编码是纯 CPU；留在异步执行器上会把整个 IPC 卡住。
        // `bounded` 顺带把它纳入进程级解码预算。
        let res = files::decode::bounded(move |permit| ingest_one(&p, &rd, &td, permit)).await;

        match res {
            Ok(ing) => {
                let new = repo::NewRefImage {
                    name: ing.name,
                    file_path: ing.file_path,
                    thumb_path: ing.thumb_path,
                    width: ing.width,
                    height: ing.height,
                    file_size: ing.file_size,
                    content_hash: ing.content_hash,
                    upload_path: ing.upload_path,
                    ephemeral,
                };
                let id = repo::insert(&state.db, &new).await?;
                views.push(RefImageView {
                    id,
                    name: new.name,
                    file_path: new.file_path,
                    thumb_path: new.thumb_path,
                    width: new.width,
                    height: new.height,
                    last_group_id: None,
                    archived: false,
                    ephemeral,
                });
            }
            Err(e) => {
                failed += 1;
                tracing::warn!(path = %path, error = %e, "参考图导入失败，跳过该张");
            }
        }
        emit_progress(&app, i as i64 + 1, total, &short, "running", failed);
    }

    emit_progress(&app, total, total, "", "done", failed);
    Ok(views)
}

/// 展示用短名（含扩展名，用户在文件管理器里看到的那个名字）。
fn short_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(path)
        .to_string()
}

pub(crate) fn emit_progress(
    app: &tauri::AppHandle,
    done: i64,
    total: i64,
    name: &str,
    phase: &str,
    failed: i64,
) {
    let _ = RefImportProgress {
        done,
        total,
        name: name.to_string(),
        phase: phase.to_string(),
        failed,
    }
    .emit(app);
}

/// 列出未删除参考图（供生成页选择）。
///
/// `include_ephemeral` = 要不要连生成页的临时上传（0019）一起返回。
///
/// **默认不要**，只有生成页传 true —— 它得渲染自己刚传的那几张。这个开关从消费端
/// 收回到参数里，是因为「每个消费端自己记得过滤」这条约定漏一处就静默出错：
/// 引导卡片的「上传参考图」那一步就漏了，于是随手在生成页拖一张试跑，
/// 那一步立刻显示成已完成，而长期图库里其实一张都没有。
#[tauri::command]
#[specta::specta]
pub async fn list_ref_images(
    state: State<'_, AppState>,
    include_ephemeral: bool,
) -> AppResult<Vec<RefImageView>> {
    let rows = repo::list_active(&state.db).await?;
    Ok(rows
        .into_iter()
        .filter(|r| include_ephemeral || !r.ephemeral)
        .map(|r| RefImageView {
            id: r.id,
            name: r.name,
            file_path: r.file_path,
            thumb_path: r.thumb_path,
            width: r.width,
            height: r.height,
            last_group_id: r.last_group_id,
            archived: r.archived_at.is_some(),
            ephemeral: r.ephemeral,
        })
        .collect())
}

// ───────────────────────────── 存量方向修复 ─────────────────────────────

/// 一张图的修复结果（只列需要摆正的那些）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrientationFix {
    pub ref_image_id: i64,
    pub name: String,
    /// 修复前库里记的尺寸。
    pub stored: (i64, i64),
    /// 按 EXIF 方向摆正后的显示尺寸。
    pub display: (u32, u32),
    /// 有排队或在跑的任务挂着它：旧上传副本先留着，避免正要上传的那条打不开文件。
    pub in_use: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrientationRepairReport {
    pub dry_run: bool,
    pub scanned: i64,
    /// 带非默认方向、本次需要（或已经）摆正的张数。
    pub needs_fix: i64,
    pub repaired: i64,
    /// 已经是摆正口径（之前修过或新版导入），跳过。
    pub already_upright: i64,
    pub missing: Vec<String>,
    pub failed: Vec<String>,
    pub fixes: Vec<OrientationFix>,
}

/// 把缩略图、上传副本与 `width/height` 重做成按 EXIF 摆正的口径。
///
/// 原文件不动（`content_hash` 依赖它的字节，去重口径不能变）；新缩略图与上传副本换新文件名
/// 写入后再改库，界面按路径缓存的旧缩略图因此不会残留。旧文件在改库之后删除，
/// 有排队/在跑任务挂着的那张保留旧上传副本（调度器每次派发重新读路径，排队的会用新副本）。
///
/// 只在显式命令里跑（`gendesk library fix-orientation`），应用启动不做任何补扫。
pub(crate) async fn repair_orientation(
    pool: &sqlx::SqlitePool,
    dry_run: bool,
) -> AppResult<OrientationRepairReport> {
    #[derive(sqlx::FromRow)]
    struct Row {
        id: i64,
        name: String,
        file_path: String,
        thumb_path: String,
        upload_path: Option<String>,
        width: i64,
        height: i64,
    }
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT id, name, file_path, thumb_path, upload_path, width, height
           FROM ref_images WHERE deleted_at IS NULL ORDER BY id",
    )
    .fetch_all(pool)
    .await?;
    let mut report = OrientationRepairReport {
        dry_run,
        scanned: rows.len() as i64,
        needs_fix: 0,
        repaired: 0,
        already_upright: 0,
        missing: Vec::new(),
        failed: Vec::new(),
        fixes: Vec::new(),
    };
    for row in rows {
        let src = PathBuf::from(&row.file_path);
        if !src.is_file() {
            report.missing.push(row.file_path);
            continue;
        }
        let probe = src.clone();
        let header = tokio::task::spawn_blocking(move || -> AppResult<_> {
            let bytes = std::fs::read(&probe)?;
            files::decode::display_dimensions(&bytes)
        })
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
        let (dw, dh, orientation) = match header {
            Ok(v) => v,
            Err(e) => {
                report.failed.push(format!("{}：{e}", row.file_path));
                continue;
            }
        };
        if orientation == image::metadata::Orientation::NoTransforms {
            continue;
        }
        report.needs_fix += 1;
        // 交换宽高的方向：库里尺寸已是显示尺寸且有上传副本，就是修过的。
        // 180°/镜像不改尺寸、判不出来，重做一遍结果相同。
        if files::decode::swaps_axes(orientation)
            && (row.width, row.height) == (i64::from(dw), i64::from(dh))
            && row.upload_path.is_some()
        {
            report.already_upright += 1;
            continue;
        }
        let in_use: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM tasks t
                 WHERE t.status IN ('q','run','retry')
                   AND (t.ref_image_id = ?1
                        OR t.batch_id IN (SELECT batch_id FROM batch_refs WHERE ref_image_id = ?1)))",
        )
        .bind(row.id)
        .fetch_one(pool)
        .await?;
        report.fixes.push(OrientationFix {
            ref_image_id: row.id,
            name: row.name.clone(),
            stored: (row.width, row.height),
            display: (dw, dh),
            in_use,
        });
        if dry_run {
            continue;
        }
        let thumb_dir = Path::new(&row.thumb_path)
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        let refs_dir = src.parent().map(Path::to_path_buf).unwrap_or_default();
        let stem = src
            .file_stem()
            .and_then(|s| s.to_str())
            .map(files::sanitize_filename)
            .unwrap_or_else(|| format!("ref{}", row.id));
        let made = files::decode::bounded(move |permit| {
            let inspected = files::decode::inspect_image(&src, permit)?;
            let thumb = unique_path(&thumb_dir, &format!("{stem}_upright"), "jpg");
            files::write_thumbnail_from(&inspected.image, &thumb)?;
            let upload = unique_path(&refs_dir, &format!("{stem}_up_upright"), "jpg");
            if let Err(e) = files::write_upload_copy_from(&inspected.image, &upload) {
                let _ = std::fs::remove_file(&thumb);
                return Err(e);
            }
            Ok((thumb, upload, inspected.width, inspected.height))
        })
        .await;
        let (thumb, upload, w, h) = match made {
            Ok(v) => v,
            Err(e) => {
                report.failed.push(format!("{}：{e}", row.file_path));
                continue;
            }
        };
        let updated = sqlx::query(
            "UPDATE ref_images SET width = ?2, height = ?3, thumb_path = ?4, upload_path = ?5
              WHERE id = ?1",
        )
        .bind(row.id)
        .bind(i64::from(w))
        .bind(i64::from(h))
        .bind(thumb.to_string_lossy().to_string())
        .bind(upload.to_string_lossy().to_string())
        .execute(pool)
        .await;
        if let Err(e) = updated {
            let _ = std::fs::remove_file(&thumb);
            let _ = std::fs::remove_file(&upload);
            report.failed.push(format!("{}：{e}", row.file_path));
            continue;
        }
        report.repaired += 1;
        let _ = std::fs::remove_file(&row.thumb_path);
        if let Some(old) = row.upload_path.filter(|p| p != &row.file_path) {
            if !in_use {
                let _ = std::fs::remove_file(old);
            }
        }
    }
    tracing::info!(
        scanned = report.scanned,
        needs_fix = report.needs_fix,
        repaired = report.repaired,
        failed = report.failed.len(),
        dry_run,
        "参考图方向修复"
    );
    Ok(report)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即失败
mod tests {
    use super::*;
    use crate::db::test_support::test_pool;
    use crate::files::decode::tests::{red_left_blue_right, with_orientation};

    /// 模拟修复前的存量：原文件带 Orientation=6，库里记的是横幅原始尺寸与横躺的缩略图。
    #[tokio::test]
    async fn repair_rotates_stored_thumbnails_upload_copies_and_dimensions_once() {
        let (pool, dir) = test_pool().await;
        let refs = dir.path().join("refs/library");
        let thumbs = dir.path().join("thumbs/library");
        std::fs::create_dir_all(&refs).unwrap();
        std::fs::create_dir_all(&thumbs).unwrap();
        let original = refs.join("竖拍.jpeg");
        std::fs::write(
            &original,
            with_orientation(&red_left_blue_right(300, 200), 6),
        )
        .unwrap();
        let old_thumb = thumbs.join("竖拍.jpg");
        let old_upload = refs.join("竖拍_up.jpg");
        std::fs::write(&old_thumb, red_left_blue_right(300, 200)).unwrap();
        std::fs::write(&old_upload, red_left_blue_right(300, 200)).unwrap();
        let plain = refs.join("横拍.jpeg");
        std::fs::write(&plain, red_left_blue_right(300, 200)).unwrap();
        let s = |p: &Path| p.to_string_lossy().to_string();
        let rotated_id = repo::insert(
            &pool,
            &repo::NewRefImage {
                name: "竖拍".into(),
                file_path: s(&original),
                thumb_path: s(&old_thumb),
                width: 300,
                height: 200,
                file_size: 1,
                content_hash: None,
                upload_path: Some(s(&old_upload)),
                ephemeral: false,
            },
        )
        .await
        .unwrap();
        repo::insert(
            &pool,
            &repo::NewRefImage {
                name: "横拍".into(),
                file_path: s(&plain),
                thumb_path: s(&old_thumb),
                width: 300,
                height: 200,
                file_size: 1,
                content_hash: None,
                upload_path: None,
                ephemeral: false,
            },
        )
        .await
        .unwrap();

        let preview = repair_orientation(&pool, true).await.unwrap();
        assert_eq!(
            (preview.scanned, preview.needs_fix, preview.repaired),
            (2, 1, 0)
        );
        assert_eq!(preview.fixes[0].display, (200, 300));
        let untouched = repo::get(&pool, rotated_id).await.unwrap().unwrap();
        assert_eq!(
            (untouched.width, untouched.height),
            (300, 200),
            "预演不改库"
        );

        let done = repair_orientation(&pool, false).await.unwrap();
        assert_eq!((done.repaired, done.failed.len()), (1, 0));
        let row = repo::get(&pool, rotated_id).await.unwrap().unwrap();
        assert_eq!((row.width, row.height), (200, 300));
        assert_ne!(
            row.thumb_path,
            s(&old_thumb),
            "换新文件名，界面不会命中旧缓存"
        );
        let thumb = image::open(&row.thumb_path).unwrap();
        assert!(thumb.height() > thumb.width(), "缩略图应是竖的");
        let upload: Option<String> =
            sqlx::query_scalar("SELECT upload_path FROM ref_images WHERE id = ?1")
                .bind(rotated_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        let upload = image::open(upload.unwrap()).unwrap();
        assert!(upload.height() > upload.width(), "上传副本应是竖的");
        assert!(!old_upload.exists(), "没有任务挂着时删掉横躺的旧副本");
        assert!(original.exists(), "原文件不动");

        let again = repair_orientation(&pool, false).await.unwrap();
        assert_eq!(
            (again.already_upright, again.repaired),
            (1, 0),
            "修过的不再重做"
        );
    }
}
