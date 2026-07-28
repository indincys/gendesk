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
    /// 图库分组（0019 起为 `ref_groups.id`，与提示词组无关）。
    pub group_id: Option<i64>,
    pub file_path: String,
    pub thumb_path: String,
    pub width: i64,
    pub height: i64,
    /// 最近一次挂靠的提示词组（E32 挂靠记忆）；生成页据此预填挂靠。
    pub last_group_id: Option<i64>,
    /// 已归档（0016）：批次开跑后自动置位，生成页选择器默认折起，库页仍可见可恢复。
    pub archived: bool,
    /// 生成页临时上传（0019）：不进长期图库，图库页与「从参考图库选择」都不列它。
    pub ephemeral: bool,
}

/// 图库分组视图（0019）。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RefGroupView {
    pub id: i64,
    pub name: String,
    pub sort_order: i64,
    /// 组内图片数（不含临时上传与已删除）。
    pub count: i64,
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

    let bytes = std::fs::read(&src)?;
    let file_size = bytes.len() as i64;
    // E30b：内容 hash（去重比对用）。用手里这份字节，与落盘的内容同源。
    let content_hash = Some(files::content_hash_bytes(&bytes));

    // 落进 refs/。这里用 write 而不是 copy，是为了保证「哈希算的」与「盘上存的」
    // 是同一串字节；权限位差异对库内文件没有意义。别改回 copy。
    let dest = unique_path(refs_dir, &stem, &ext);
    std::fs::write(&dest, &bytes)?;

    // 尺寸只读文件头。这也是写进 ref_images.width/height 的那个值，必须是原图尺寸。
    let (w, h) = image::ImageReader::new(std::io::Cursor::new(&bytes))
        .with_guessed_format()?
        .into_dimensions()?;

    // E41：长边或字节数超限就出一份上传用压缩副本。判定在解码之前。
    let oversize = w > files::UPLOAD_MAX_EDGE
        || h > files::UPLOAD_MAX_EDGE
        || bytes.len() as u64 > files::UPLOAD_MAX_BYTES;

    // 唯一一次解码。超限图解到 ≥2048（上传副本要），否则解到 ≥512（只做缩略图）。
    // 6240 宽的源在 1/2 档解出 3120，两个派生物都够。
    let target = if oversize {
        files::UPLOAD_MAX_EDGE
    } else {
        files::THUMB_MAX
    };
    let img = files::decode::decode_scaled(&bytes, target, permit)?;

    let thumb = unique_path(thumbs_dir, &stem, "jpg");
    files::write_thumbnail_from(&img, &thumb)?;

    let upload_path = if oversize {
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
    group_id: Option<i64>,
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
                    ref_group_id: group_id,
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
                    group_id,
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

fn emit_progress(
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

/// 导入前重复扫描（E30b）：按内容 hash 比对已有库 + 本次列表内，标注重复项。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RefScanItem {
    pub path: String,
    pub name: String,
    pub duplicate: bool,
    /// 与之重复的已有图名（库内）或本次靠前的文件名。
    pub dup_of: Option<String>,
}

#[tauri::command]
#[specta::specta]
pub async fn scan_ref_imports(
    state: State<'_, AppState>,
    paths: Vec<String>,
) -> AppResult<Vec<RefScanItem>> {
    // 库内 hash → 名称（best-effort 展示重复源）。
    let existing = repo::active_hash_names(&state.db).await?;
    let mut seen: std::collections::HashMap<String, String> = existing.into_iter().collect();
    let mut out = Vec::with_capacity(paths.len());
    for path in paths {
        let src = PathBuf::from(&path);
        let name = src
            .file_stem()
            .and_then(|s| s.to_str())
            .map(files::sanitize_filename)
            .unwrap_or_else(|| "未命名".into());
        let hash = files::content_hash(&src).ok();
        let dup_of = hash.as_ref().and_then(|h| seen.get(h).cloned());
        let duplicate = dup_of.is_some();
        if let Some(h) = hash {
            seen.entry(h).or_insert_with(|| name.clone());
        }
        out.push(RefScanItem {
            path,
            name,
            duplicate,
            dup_of,
        });
    }
    Ok(out)
}

/// 批量改分组（E30b）。gid=None 为未分组。
#[tauri::command]
#[specta::specta]
pub async fn set_ref_images_group(
    state: State<'_, AppState>,
    ids: Vec<i64>,
    group_id: Option<i64>,
) -> AppResult<()> {
    repo::set_group_many(&state.db, &ids, group_id).await?;
    Ok(())
}

/// 批量删除参考图 → 进废纸篓（E30b）。
#[tauri::command]
#[specta::specta]
pub async fn trash_ref_images(state: State<'_, AppState>, ids: Vec<i64>) -> AppResult<()> {
    for id in ids {
        trash_one_ref(&state, id).await?;
    }
    Ok(())
}

/// 单张参考图入废纸篓（原图 + 缩略图 + 上传副本一并暂存至清理）。
async fn trash_one_ref(state: &AppState, id: i64) -> AppResult<()> {
    let Some(row) = repo::soft_delete(&state.db, id).await? else {
        return Ok(());
    };
    let mut file_paths = vec![row.file_path.clone(), row.thumb_path.clone()];
    if let Some(up) = &row.upload_path {
        file_paths.push(up.clone());
    }
    let mut tx = state.db.begin().await?;
    crate::db::repo::trash::insert(
        &mut tx,
        &crate::db::repo::trash::NewTrashItem {
            entity_type: "ref".into(),
            ref_id: Some(row.id),
            thumb_path: Some(row.thumb_path.clone()),
            prompt_text: None,
            code: None,
            title: None,
            source_label: "手动删除".into(),
            file_paths,
            payload_json: None,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

/// 列出未删除参考图（供参考图库/生成页选择）。
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
            group_id: r.ref_group_id,
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

// ---------- 图库分组（0019） ----------

#[tauri::command]
#[specta::specta]
pub async fn list_ref_groups(state: State<'_, AppState>) -> AppResult<Vec<RefGroupView>> {
    Ok(repo::list_groups(&state.db)
        .await?
        .into_iter()
        .map(|(g, count)| RefGroupView {
            id: g.id,
            name: g.name,
            sort_order: g.sort_order,
            count,
        })
        .collect())
}

/// 新建图库分组。重名（NOCASE）直接返回既有那个，不报错——用户要的是「有这个组」，
/// 不是一句「名字被占了」。
#[tauri::command]
#[specta::specta]
pub async fn create_ref_group(state: State<'_, AppState>, name: String) -> AppResult<RefGroupView> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::InvalidInput("分组名不能为空".into()));
    }
    if let Some(g) = repo::find_group_by_name(&state.db, name).await? {
        let count = repo::list_groups(&state.db)
            .await?
            .into_iter()
            .find(|(r, _)| r.id == g.id)
            .map(|(_, c)| c)
            .unwrap_or(0);
        return Ok(RefGroupView {
            id: g.id,
            name: g.name,
            sort_order: g.sort_order,
            count,
        });
    }
    let g = repo::create_group(&state.db, name).await?;
    Ok(RefGroupView {
        id: g.id,
        name: g.name,
        sort_order: g.sort_order,
        count: 0,
    })
}

#[tauri::command]
#[specta::specta]
pub async fn rename_ref_group(state: State<'_, AppState>, id: i64, name: String) -> AppResult<()> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::InvalidInput("分组名不能为空".into()));
    }
    if let Some(other) = repo::find_group_by_name(&state.db, name).await? {
        if other.id != id {
            return Err(AppError::InvalidInput(format!("已有同名分组「{name}」")));
        }
    }
    if !repo::rename_group(&state.db, id, name).await? {
        return Err(AppError::InvalidInput("分组不存在".into()));
    }
    Ok(())
}

/// 删除图库分组。组内图片**不删**，只是回到未分组。
#[tauri::command]
#[specta::specta]
pub async fn delete_ref_group(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    if !repo::delete_group(&state.db, id).await? {
        return Err(AppError::InvalidInput("分组不存在".into()));
    }
    Ok(())
}

/// 调整参考图分组。
#[tauri::command]
#[specta::specta]
pub async fn set_ref_image_group(
    state: State<'_, AppState>,
    id: i64,
    group_id: Option<i64>,
) -> AppResult<()> {
    repo::set_group(&state.db, id, group_id).await?;
    Ok(())
}

/// 归档 / 取消归档参考图（0016）。批次开跑后由 `engine::create_batch` 自动归档；
/// 此命令供参考图库手动恢复（或手动归档一张用不上的旧图）。
#[tauri::command]
#[specta::specta]
pub async fn set_ref_image_archived(
    state: State<'_, AppState>,
    id: i64,
    archived: bool,
) -> AppResult<()> {
    if !repo::set_archived(&state.db, id, archived).await? {
        return Err(AppError::InvalidInput("参考图不存在".into()));
    }
    Ok(())
}

/// 参考图详情（含使用统计）。
#[derive(Debug, Clone, serde::Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RefImageDetail {
    pub id: i64,
    pub name: String,
    pub group_id: Option<i64>,
    pub file_path: String,
    pub thumb_path: String,
    pub width: i64,
    pub height: i64,
    pub used_count: i64,
    pub works_count: i64,
}

#[tauri::command]
#[specta::specta]
pub async fn get_ref_image(state: State<'_, AppState>, id: i64) -> AppResult<RefImageDetail> {
    let r = repo::get(&state.db, id)
        .await?
        .ok_or_else(|| AppError::InvalidInput("参考图不存在".into()))?;
    let (used, works) = repo::usage_stats(&state.db, id).await?;
    Ok(RefImageDetail {
        id: r.id,
        name: r.name,
        group_id: r.ref_group_id,
        file_path: r.file_path,
        thumb_path: r.thumb_path,
        width: r.width,
        height: r.height,
        used_count: used,
        works_count: works,
    })
}

/// 更换参考图文件（保留 id 与关联）。
#[tauri::command]
#[specta::specta]
pub async fn replace_ref_image_file(
    state: State<'_, AppState>,
    id: i64,
    path: String,
) -> AppResult<()> {
    state.dirs.init()?;
    let (refs_dir, thumbs_dir) = (state.dirs.refs(), state.dirs.thumbs());
    // 走 `ingest_one` 而不是在这里再写一遍拷贝 + 缩略图 + hash + 压缩副本：
    // 那是同一条管线的第二份抄本，而两份抄本只会在下一次改口径时分叉
    // （E30b 的 hash、E41 的上传副本都是后来补进去的，抄本迟早会漏掉某一次）。
    // 顺带把这整段纯 CPU/IO 挪进 `spawn_blocking`（同 v0.14.0 导入那次的教训）。
    let ing =
        files::decode::bounded(move |permit| ingest_one(&path, &refs_dir, &thumbs_dir, permit))
            .await?;
    repo::update_file(
        &state.db,
        id,
        &ing.file_path,
        &ing.thumb_path,
        ing.width,
        ing.height,
        ing.file_size,
        ing.content_hash.as_deref(),
        ing.upload_path.as_deref(),
    )
    .await?;
    Ok(())
}

/// 删除参考图 → 进废纸篓。
#[tauri::command]
#[specta::specta]
pub async fn trash_ref_image(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    trash_one_ref(&state, id).await
}
