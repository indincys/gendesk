//! 收件箱收录事务（发布模块执行计划 §5.1 inbox/ingest）。
//!
//! TXT：解析 → 三冗余识别 SKU → 已知则拆入标题/正文池 + 归档原文件；未知 SKU=待认领；解析失败=待人工确认。
//! 媒体：SKU 文件夹内 jpg/png/webp 整批 → 1 图集包；每个 mp4/mov → 1 视频包；重命名 ASCII 后移入资产库。

use std::path::Path;

use serde::{Deserialize, Serialize};
use specta::Type;
use sqlx::SqlitePool;

use crate::db::repo::{assets, inbox, skus, texts};
use crate::error::{AppError, AppResult};
use crate::publish::inbox::parser;
use crate::publish::paths::{self, RelPath};
use crate::publish::platform;

/// 单文件收录结果（供事件与报告使用）。camelCase 字段名个别指定（specta 不识别 rename_all_fields）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum IngestOutcome {
    /// TXT 成功入库并归档。
    Ingested {
        #[serde(rename = "skuCode")]
        sku_code: String,
        kind: String,
        /// 新增条数（不含重复）。
        titles: usize,
        bodies: usize,
        /// 与池内已有条目完全相同、被跳过的条数。
        duplicates: usize,
        /// 采纳的话题（SKU 原先无话题时）。
        #[serde(rename = "topicsAdopted")]
        topics_adopted: Vec<String>,
        /// 话题差异提示（SKU 已有话题、忽略了本文件话题）。
        #[serde(rename = "topicDiff")]
        topic_diff: Option<String>,
    },
    /// 媒体文件夹成包入库（自动归集或人工认领后）。
    IngestedMedia {
        #[serde(rename = "skuCode")]
        sku_code: String,
        /// 本次建包数。
        packs: usize,
    },
    /// TXT 识别不出已知 SKU，待认领。
    Unclaimed {
        #[serde(rename = "skuCode")]
        sku_code: Option<String>,
    },
    /// 媒体文件夹识别不出已知 SKU，整个文件夹一条待认领（需求 §3.6：进队列由人工处理，不丢弃）。
    UnclaimedMedia {
        /// 收件箱内的文件夹名。
        folder: String,
        /// 文件夹内媒体文件数（含子文件夹）。
        files: usize,
    },
    /// 解析失败，待人工确认。
    Failed { reason: String },
}

impl IngestOutcome {
    /// inbox_items.state 存储值。
    pub fn state_code(&self) -> &'static str {
        match self {
            IngestOutcome::Ingested { .. } | IngestOutcome::IngestedMedia { .. } => "ingested",
            IngestOutcome::Unclaimed { .. } | IngestOutcome::UnclaimedMedia { .. } => "unclaimed",
            IngestOutcome::Failed { .. } => "failed",
        }
    }
}

/// inbox_items.kind 的媒体取值（TXT 的 kind 由解析结果给出：title/body）。
pub const KIND_MEDIA: &str = "media";

/// 一轮 rescan 中的一个条目。
#[derive(Debug, Clone)]
pub struct RescanItem {
    /// 条目对应的收件箱内相对路径（TXT 为文件，媒体为文件夹）。
    pub file_rel: RelPath,
    pub outcome: IngestOutcome,
    /// 本轮**新建或状态变化**——滞留的待认领/失败条目每轮都会被重扫到，
    /// 只有 true 才该弹 toast，否则每次收件箱有任何风吹草动都重发一遍旧提示。
    pub changed: bool,
}

/// 媒体扩展名分类。
fn media_kind(ext: &str) -> Option<&'static str> {
    match ext.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" | "png" | "webp" => Some("gallery"),
        "mp4" | "mov" => Some("video"),
        _ => None,
    }
}

/// 同步软件/浏览器的半成品文件：还在写，收录进来就是半截内容。
fn is_temp_file(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    name.starts_with('.')
        || name.starts_with("~$")
        || lower.starts_with(".syncthing.")
        || lower.ends_with(".tmp")
        || lower.ends_with(".part")
        || lower.ends_with(".crdownload")
        || lower.ends_with(".download")
}

/// 大小采样间隔。
const SIZE_PROBE: std::time::Duration = std::time::Duration::from_millis(500);

fn size_of(p: &Path) -> Option<u64> {
    std::fs::metadata(p).map(|m| m.len()).ok()
}

/// 从候选里筛出**大小已稳定**的文件：一次性采样全部 → 等 500ms → 再采样一次，
/// 两次一致的才算写完。仍在变的本轮跳过（下一次文件系统事件自然会再来一轮）。
///
/// watcher 的 2 秒事件防抖只保证「没有新的文件系统事件」；写入方停顿超过 2 秒
/// （人工分批拷贝、同步软件限速）照样会让我们收录到半截文件。这是第二道闸。
/// 整批只睡一次——按文件逐个睡的话，50 张图要等 25 秒。
async fn filter_size_stable(paths: Vec<std::path::PathBuf>) -> Vec<std::path::PathBuf> {
    if paths.is_empty() {
        return paths;
    }
    let first: Vec<Option<u64>> = paths.iter().map(|p| size_of(p)).collect();
    tokio::time::sleep(SIZE_PROBE).await;
    paths
        .into_iter()
        .zip(first)
        .filter_map(|(p, before)| {
            let stable = before.is_some() && size_of(&p) == before;
            if !stable {
                tracing::debug!(file = %p.display(), "文件大小仍在变化，本轮跳过");
            }
            stable.then_some(p)
        })
        .collect()
}

/// `YYYYMMDD`（本地日期，归档子目录用）。
fn today_yyyymmdd() -> String {
    use chrono::Datelike;
    let now = chrono::Local::now();
    format!("{:04}{:02}{:02}", now.year(), now.month(), now.day())
}

/// 收录一个 TXT 文件（相对收件箱根的 rel）。执行 DB 写 + 文件搬运。
/// `forced_sku`：认领时人工指认的 SKU 编码（跳过三冗余识别）。
pub async fn ingest_txt(
    pool: &SqlitePool,
    root: &Path,
    file_rel: &RelPath,
    forced_sku: Option<&str>,
) -> AppResult<IngestOutcome> {
    let (_, outcome, _) = ingest_txt_inner(pool, root, file_rel, forced_sku).await?;
    Ok(outcome)
}

/// 同 [`ingest_txt`]，另返回（记录落到的 rel，是否新建/状态变化）供 rescan 组装 [`RescanItem`]。
async fn ingest_txt_inner(
    pool: &SqlitePool,
    root: &Path,
    file_rel: &RelPath,
    forced_sku: Option<&str>,
) -> AppResult<(RelPath, IngestOutcome, bool)> {
    let abs = file_rel.to_local(root);
    let content = std::fs::read(&abs)?;
    // 收件箱统一 UTF-8（规范 §2）；容错非 UTF-8 以 lossy 解析。
    let text = String::from_utf8_lossy(&content);

    let filename = abs
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    let folder = abs
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .map(str::to_string);

    let kind_hint = parser::kind_from_filename(&filename);
    let parsed = match parser::parse(&text, kind_hint) {
        Ok(p) => p,
        Err(e) => {
            let outcome = IngestOutcome::Failed {
                reason: e.to_string(),
            };
            let changed = record_item(pool, file_rel, None, None, &outcome).await?;
            return Ok((file_rel.clone(), outcome, changed));
        }
    };

    // ASCII 三冗余候选（头【SKU】> 文件名前缀 > 文件夹名，用于按编码查库 + 报告）。
    let ascii_candidate = forced_sku
        .map(str::to_string)
        .or_else(|| parser::resolve_sku(parsed.sku_code.as_deref(), &filename, folder.as_deref()));

    // 查已知 SKU：先按编码；未命中且非人工认领时，再按别名（头【SKU】原值 / 文件夹名原值，
    // 可能是中文如 A-敖瑞鹏-01）查库。
    let mut sku = match ascii_candidate.as_deref() {
        Some(code) => skus::find_by_code(pool, code).await?,
        None => None,
    };
    if sku.is_none() && forced_sku.is_none() {
        for tok in [parsed.sku_code.as_deref(), folder.as_deref()]
            .into_iter()
            .flatten()
        {
            if let Some(s) = skus::find_by_alias(pool, tok).await? {
                sku = Some(s);
                break;
            }
        }
    }
    // 命中则用真实编码报告，否则回落 ASCII 候选。
    let sku_candidate = sku
        .as_ref()
        .map(|s| s.code.clone())
        .or_else(|| ascii_candidate.clone());

    let Some(sku) = sku else {
        let outcome = IngestOutcome::Unclaimed {
            sku_code: sku_candidate.clone(),
        };
        let changed = record_item(
            pool,
            file_rel,
            Some(parsed.kind.code()),
            sku_candidate.as_deref(),
            &outcome,
        )
        .await?;
        return Ok((file_rel.clone(), outcome, changed));
    };

    // 已知 SKU：单事务入库（标题/正文 + 话题采纳）。
    let platform_tag = parsed
        .platform
        .as_deref()
        .map(platform::text_platform_tag)
        .unwrap_or_else(|| platform::GENERAL_TAG.to_string());

    let (topics_adopted, topic_diff) =
        resolve_topics(pool, sku.id, &sku.topics_json, &parsed.topics).await?;

    // 入库查重：AI 反复生成同一句话是常态，同 SKU 同类型同文本只留一条
    // （否则文本池被同一句刷屏，「最少使用优先」也就失效了）。
    let mut tx = pool.begin().await?;
    let mut titles_new = 0usize;
    let mut bodies_new = 0usize;
    let mut duplicates = 0usize;
    for (kind, list, counter) in [
        ("title", &parsed.titles, &mut titles_new),
        ("body", &parsed.bodies, &mut bodies_new),
    ] {
        for t in list.iter() {
            if texts::exists_same(&mut tx, sku.id, kind, t).await? {
                duplicates += 1;
                continue;
            }
            texts::insert_tx(
                &mut tx,
                &texts::NewTextItem {
                    sku_id: sku.id,
                    kind: kind.into(),
                    text: t.clone(),
                    platform: platform_tag.clone(),
                    source: "inbox".into(),
                },
            )
            .await?;
            *counter += 1;
        }
    }
    if !topics_adopted.is_empty() {
        let json = serde_json::to_string(&topics_adopted)?;
        sqlx::query("UPDATE skus SET topics_json = ?2, updated_at = ?3 WHERE id = ?1")
            .bind(sku.id)
            .bind(&json)
            .bind(crate::db::now_unix())
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;

    // 归档原文件到 收件箱/已收录/{YYYYMMDD}/（去重命名）。
    let archived_rel = archive_ingested(root, file_rel)?;

    let outcome = IngestOutcome::Ingested {
        sku_code: sku.code.clone(),
        kind: parsed.kind.code().to_string(),
        titles: titles_new,
        bodies: bodies_new,
        duplicates,
        topics_adopted,
        topic_diff,
    };
    let changed = record_item(
        pool,
        &archived_rel,
        Some(parsed.kind.code()),
        Some(&sku.code),
        &outcome,
    )
    .await?;
    Ok((archived_rel, outcome, changed))
}

/// 话题三选：SKU 无话题 → 采纳前 5；已有 → 忽略并给差异提示（绝不覆盖，前置事实 11）。
async fn resolve_topics(
    _pool: &SqlitePool,
    _sku_id: i64,
    existing_json: &str,
    incoming: &[String],
) -> AppResult<(Vec<String>, Option<String>)> {
    if incoming.is_empty() {
        return Ok((Vec::new(), None));
    }
    let existing: Vec<String> = serde_json::from_str(existing_json).unwrap_or_default();
    if existing.is_empty() {
        let adopted: Vec<String> = incoming.iter().take(5).cloned().collect();
        Ok((adopted, None))
    } else {
        let diff = format!(
            "已有话题 [{}]，忽略文件话题 [{}]",
            existing.join(" "),
            incoming.join(" ")
        );
        Ok((Vec::new(), Some(diff)))
    }
}

/// 记录 inbox_items（已存在同 file_rel 则更新状态）。
/// 返回**是否新建或状态/详情发生变化**——watcher 据此决定是否推 toast（见 [`RescanItem::changed`]）。
async fn record_item(
    pool: &SqlitePool,
    file_rel: &RelPath,
    kind: Option<&str>,
    sku_code: Option<&str>,
    outcome: &IngestOutcome,
) -> AppResult<bool> {
    let detail = serde_json::to_string(outcome).ok();
    if let Some(existing) = inbox::find_by_rel(pool, file_rel.as_str()).await? {
        let changed = existing.state != outcome.state_code()
            || existing.detail_json.as_deref() != detail.as_deref();
        sqlx::query(
            "UPDATE inbox_items SET kind=?2, sku_code=?3, state=?4, detail_json=?5 WHERE id=?1",
        )
        .bind(existing.id)
        .bind(kind)
        .bind(sku_code)
        .bind(outcome.state_code())
        .bind(&detail)
        .execute(pool)
        .await?;
        Ok(changed)
    } else {
        inbox::insert(
            pool,
            &inbox::NewInboxItem {
                file_rel: file_rel.as_str().to_string(),
                kind: kind.map(str::to_string),
                sku_code: sku_code.map(str::to_string),
                state: outcome.state_code().to_string(),
                detail_json: detail,
            },
        )
        .await?;
        Ok(true)
    }
}

/// 移动收件箱内的文件/文件夹到 `收件箱/{subdir}/{YYYYMMDD}/`，返回归档后相对路径。
/// `subdir` 取 [`paths::INGESTED`]（收录成功）或 [`paths::DISCARDED`]（人工丢弃）——
/// 两者都被 rescan 排除在外，故归档过的东西不会被下一轮重新收录。
/// 源不存在时视为已归档，原样返回（幂等）。
pub fn archive_to(root: &Path, file_rel: &RelPath, subdir: &str) -> AppResult<RelPath> {
    let src = file_rel.to_local(root);
    if !src.exists() {
        return Ok(file_rel.clone());
    }
    let name = src
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| AppError::Io("归档目标无文件名".into()))?
        .to_string();
    let dir_rel = RelPath::from_parts([paths::INBOX, subdir, &today_yyyymmdd()]);
    let dir_abs = dir_rel.to_local(root);
    std::fs::create_dir_all(&dir_abs)?;
    let final_name = paths::dedupe_name(&name, &|n| dir_abs.join(n).exists());
    let dest_rel = dir_rel.join(&final_name);
    std::fs::rename(&src, dest_rel.to_local(root))?;
    Ok(dest_rel)
}

/// 移动原文件到 收件箱/已收录/{YYYYMMDD}/，返回归档后相对路径。
fn archive_ingested(root: &Path, file_rel: &RelPath) -> AppResult<RelPath> {
    archive_to(root, file_rel, paths::INGESTED)
}

/// 收集一个收件箱 SKU 文件夹内的媒体为素材包（前置事实 10），移入资产库。
/// `folder_name` 为收件箱内的目录名（可为 SKU 编码或中文别名，如 A-敖瑞鹏-01）。
/// 规则：
/// - 文件夹**根目录**散放的图片整批 → 1 个默认图集包；每个视频 → 1 个视频包。
/// - 每个**直接子文件夹** → 独立成包（子文件夹内图片 → 1 图集包；每个视频 → 1 视频包）。
///
/// 图序按文件名排序；`cover.*`/`<video>_cover.*` 识别为封面。
///
/// `forced_sku`：人工认领时指认的 SKU 编码（跳过按文件夹名的三冗余识别）。
pub async fn collect_media(
    pool: &SqlitePool,
    root: &Path,
    folder_name: &str,
    forced_sku: Option<&str>,
) -> AppResult<Vec<i64>> {
    let found = match forced_sku {
        Some(code) => skus::find_by_code(pool, code).await?,
        None => skus::find_by_code_or_alias(pool, folder_name).await?,
    };
    let Some(sku) = found else {
        return Ok(Vec::new());
    };
    let folder_abs = RelPath::from_parts([paths::INBOX, folder_name]).to_local(root);
    if !folder_abs.is_dir() {
        return Ok(Vec::new());
    }

    let mut created = Vec::new();
    // 1) 根目录散放媒体 → 默认图集（dir_base=gallery）+ 视频包。
    created.extend(collect_dir_media(pool, root, &sku, &folder_abs, "gallery").await?);
    // 2) 每个直接子文件夹 → 独立成包（图集目录名取子文件夹名）。
    for entry in std::fs::read_dir(&folder_abs)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if paths::INBOX_ARCHIVES.contains(&name.as_str()) || name.starts_with('.') {
            continue;
        }
        created.extend(collect_dir_media(pool, root, &sku, &entry.path(), &name).await?);
    }
    Ok(created)
}

/// 递归数一个收件箱文件夹内的媒体文件（待认领记录的 detail 计数）。
fn count_media_files(dir: &Path) -> usize {
    walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            !name.starts_with('.') && media_kind(&paths::ascii_ext(&name)).is_some()
        })
        .count()
}

/// 从单个目录（非递归）归集媒体为素材包。`gallery_dir_base` 决定图集在资产库的目录名基。
async fn collect_dir_media(
    pool: &SqlitePool,
    root: &Path,
    sku: &skus::SkuRow,
    src_dir: &Path,
    gallery_dir_base: &str,
) -> AppResult<Vec<i64>> {
    // 枚举媒体文件（忽略隐藏/锁文件/同步软件半成品与子目录），并等大小稳定。
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    for entry in std::fs::read_dir(src_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if is_temp_file(&name) {
            continue;
        }
        candidates.push(entry.path());
    }
    let stable = filter_size_stable(candidates).await;

    let mut galleries: Vec<(String, String)> = Vec::new(); // (filename, ext)
    let mut videos: Vec<(String, String)> = Vec::new();
    let mut covers: Vec<String> = Vec::new(); // 潜在封面文件名
    for path in stable {
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        let ext = paths::ascii_ext(&name);
        let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(&name);
        if stem.eq_ignore_ascii_case("cover") || stem.to_ascii_lowercase().ends_with("_cover") {
            covers.push(name.clone());
        }
        match media_kind(&ext) {
            Some("gallery") => galleries.push((name, ext)),
            Some("video") => videos.push((name, ext)),
            _ => {}
        }
    }

    let mut created = Vec::new();

    // 图集包：非封面的图片整批 → 1 包。
    let mut gallery_imgs: Vec<(String, String)> = galleries
        .into_iter()
        .filter(|(n, _)| !covers.contains(n))
        .collect();
    gallery_imgs.sort_by(|a, b| a.0.cmp(&b.0));
    let gallery_cover = covers.iter().find(|c| {
        let stem = c.rsplit_once('.').map(|(s, _)| s).unwrap_or(c);
        stem.eq_ignore_ascii_case("cover")
    });
    if !gallery_imgs.is_empty() {
        let id = build_pack(
            pool,
            root,
            &sku.code,
            sku.id,
            "gallery",
            gallery_dir_base,
            &gallery_imgs,
            gallery_cover.map(|s| s.as_str()),
            src_dir,
        )
        .await?;
        created.push(id);
    }

    // 视频包：每个视频 → 1 包；`<video>_cover.*` 为其封面。
    for (vname, vext) in videos {
        let vstem = vname.rsplit_once('.').map(|(s, _)| s).unwrap_or(&vname);
        let vcover = covers.iter().find(|c| {
            let cstem = c.rsplit_once('.').map(|(s, _)| s).unwrap_or(c);
            cstem.eq_ignore_ascii_case(&format!("{vstem}_cover"))
        });
        let base = paths::ascii_slug(vstem);
        let id = build_pack(
            pool,
            root,
            &sku.code,
            sku.id,
            "video",
            &base,
            &[(vname.clone(), vext)],
            vcover.map(|s| s.as_str()),
            src_dir,
        )
        .await?;
        created.push(id);
    }

    Ok(created)
}

/// 元数据条目（files_json 元素）。
#[derive(Debug, Serialize)]
struct PackFile {
    name: String,
    #[serde(rename = "origName")]
    orig_name: String,
    bytes: u64,
}

/// 建包：分配唯一目录 → 复制/移动并 ASCII 重命名 → 落库。
#[allow(clippy::too_many_arguments)]
async fn build_pack(
    pool: &SqlitePool,
    root: &Path,
    sku_code: &str,
    sku_id: i64,
    kind: &str,
    dir_base: &str,
    members: &[(String, String)], // (orig_filename, ext)
    cover_orig: Option<&str>,
    src_folder: &Path,
) -> AppResult<i64> {
    // 目标包目录：资产库/{SKU}/{unique}。
    let sku_lib_rel = RelPath::from_parts([paths::ASSET_LIB, sku_code]);
    let sku_lib_abs = sku_lib_rel.to_local(root);
    std::fs::create_dir_all(&sku_lib_abs)?;
    let base = paths::ascii_slug(dir_base);
    let dir_name = paths::dedupe_name(&base, &|n| sku_lib_abs.join(n).exists());
    let pack_rel = sku_lib_rel.join(&dir_name);
    let pack_abs = pack_rel.to_local(root);
    std::fs::create_dir_all(&pack_abs)?;

    let mut files: Vec<PackFile> = Vec::new();
    // 已搬走的 (dest, 原位置)：落库失败时按此原路搬回，不让文件消失在一个没有记录的目录里。
    let mut moved: Vec<(std::path::PathBuf, std::path::PathBuf)> = Vec::new();

    // 成员文件重命名：图集 img_NN.ext，视频 video.ext。
    for (i, (orig, ext)) in members.iter().enumerate() {
        let new_name = if kind == "gallery" {
            paths::gallery_member(i + 1, ext)
        } else {
            paths::video_member(ext)
        };
        let src = src_folder.join(orig);
        let dest = pack_abs.join(&new_name);
        let bytes = size_of(&src).unwrap_or(0);
        if let Err(e) = std::fs::rename(&src, &dest) {
            rollback_moves(&moved);
            return Err(e.into());
        }
        moved.push((dest, src));
        files.push(PackFile {
            name: new_name,
            orig_name: orig.clone(),
            bytes,
        });
    }
    // 封面重命名为 cover.<ext>。
    let mut cover_name: Option<String> = None;
    if let Some(orig) = cover_orig {
        let ext = paths::ascii_ext(orig);
        let new_name = format!("cover.{ext}");
        let src = src_folder.join(orig);
        if src.exists() {
            let dest = pack_abs.join(&new_name);
            let bytes = size_of(&src).unwrap_or(0);
            if let Err(e) = std::fs::rename(&src, &dest) {
                rollback_moves(&moved);
                return Err(e.into());
            }
            moved.push((dest, src));
            files.push(PackFile {
                name: new_name.clone(),
                orig_name: orig.to_string(),
                bytes,
            });
            cover_name = Some(new_name);
        }
    }

    let files_json = serde_json::to_string(&files)?;
    let inserted = assets::insert(
        pool,
        &assets::NewPack {
            sku_id,
            kind: kind.to_string(),
            dir_rel: pack_rel.as_str().to_string(),
            files_json,
            cover: cover_name,
            source: "inbox".into(),
        },
    )
    .await;
    match inserted {
        Ok(id) => Ok(id),
        Err(e) => {
            // 落库失败 → 文件搬回原位，目录删掉。否则素材就成了「磁盘上有、库里没有」的孤儿。
            rollback_moves(&moved);
            let _ = std::fs::remove_dir(&pack_abs);
            Err(e.into())
        }
    }
}

/// 把已搬走的文件搬回原位（建包落库失败时的回滚）。
/// 回滚本身失败只能 warn——此时文件在包目录里，落一个 `.orphan` 标记便于人工找回。
fn rollback_moves(moved: &[(std::path::PathBuf, std::path::PathBuf)]) {
    for (dest, src) in moved {
        if let Err(e) = std::fs::rename(dest, src) {
            tracing::warn!(
                file = %dest.display(),
                error = %e,
                "建包回滚失败，文件仍在资产库目录内（无对应记录）"
            );
            let _ = std::fs::write(dest.with_extension("orphan"), b"pack insert failed\n");
        }
    }
}

/// 一个收件箱路径是否位于归档子目录（已收录/已丢弃）内 —— 归档过的东西不再重扫。
fn in_archive(path: &Path) -> bool {
    path.components().any(|c| {
        c.as_os_str()
            .to_str()
            .is_some_and(|s| paths::INBOX_ARCHIVES.contains(&s))
    })
}

/// 全量扫描收件箱：各 SKU 子目录归集媒体 + 收录全部 TXT（排除 已收录/、已丢弃/）。
/// 启动补跑与手动「重扫收件箱」共用。
///
/// **单文件失败不阻断全局**：坏文件记为 failed 条目后继续下一个（否则一份坏 TXT
/// 能让整个收件箱停摆）。
pub async fn rescan(pool: &SqlitePool, root: &Path) -> AppResult<Vec<RescanItem>> {
    let inbox_abs = RelPath::from_parts([paths::INBOX]).to_local(root);
    if !inbox_abs.is_dir() {
        return Ok(Vec::new());
    }
    let mut items: Vec<RescanItem> = Vec::new();

    // 1) 各直接子目录（SKU 文件夹）归集媒体；未知 SKU 的文件夹进「待认领」（不静默滞留）。
    for entry in std::fs::read_dir(&inbox_abs)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if paths::INBOX_ARCHIVES.contains(&name.as_str()) || name.starts_with('.') {
            continue;
        }
        let folder_rel = RelPath::from_parts([paths::INBOX, &name]);
        match skus::find_by_code_or_alias(pool, &name).await? {
            Some(sku) => match collect_media(pool, root, &name, None).await {
                Ok(ids) if ids.is_empty() => {}
                Ok(ids) => {
                    let outcome = IngestOutcome::IngestedMedia {
                        sku_code: sku.code.clone(),
                        packs: ids.len(),
                    };
                    // 建包后源文件已移走，用「文件夹 + 时间戳」作 file_rel 保证每批一条新记录。
                    let rel = folder_rel.join(format!("#{}", crate::db::now_unix()));
                    let changed =
                        record_item(pool, &rel, Some(KIND_MEDIA), Some(&sku.code), &outcome)
                            .await?;
                    items.push(RescanItem {
                        file_rel: rel,
                        outcome,
                        changed,
                    });
                }
                Err(e) => {
                    tracing::warn!(folder = %name, error = %e, "归集媒体失败，跳过该文件夹");
                    let outcome = IngestOutcome::Failed {
                        reason: e.to_string(),
                    };
                    let changed = record_item(pool, &folder_rel, Some(KIND_MEDIA), None, &outcome)
                        .await
                        .unwrap_or(false);
                    items.push(RescanItem {
                        file_rel: folder_rel,
                        outcome,
                        changed,
                    });
                }
            },
            None => {
                let files = count_media_files(&entry.path());
                if files == 0 {
                    continue; // 只有 TXT 的文件夹交给下面的 TXT 分支处理。
                }
                let outcome = IngestOutcome::UnclaimedMedia {
                    folder: name.clone(),
                    files,
                };
                let changed =
                    record_item(pool, &folder_rel, Some(KIND_MEDIA), None, &outcome).await?;
                items.push(RescanItem {
                    file_rel: folder_rel,
                    outcome,
                    changed,
                });
            }
        }
    }

    // 2) 收录全部 TXT（排除归档目录、隐藏/锁文件、同步软件半成品；等大小稳定）。
    let mut txts: Vec<std::path::PathBuf> = Vec::new();
    for entry in walkdir::WalkDir::new(&inbox_abs)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if in_archive(path) {
            continue;
        }
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if is_temp_file(name) || !name.to_ascii_lowercase().ends_with(".txt") {
            continue;
        }
        txts.push(path.to_path_buf());
    }
    for path in filter_size_stable(txts).await {
        let Ok(sub) = path.strip_prefix(root) else {
            continue;
        };
        let rel = RelPath::new(sub.to_string_lossy());
        match ingest_txt_recording(pool, root, &rel).await {
            Ok(item) => items.push(item),
            Err(e) => tracing::warn!(file = %rel.as_str(), error = %e, "记录收录结果失败，跳过"),
        }
    }
    Ok(items)
}

/// 收录一个 TXT 并记录结果；收录本身失败（IO/DB）时记为 failed 条目而非向上传播——
/// rescan 是全量扫描，一个坏文件不该中断其余文件的收录。
async fn ingest_txt_recording(
    pool: &SqlitePool,
    root: &Path,
    rel: &RelPath,
) -> AppResult<RescanItem> {
    match ingest_txt_inner(pool, root, rel, None).await {
        Ok((file_rel, outcome, changed)) => Ok(RescanItem {
            file_rel,
            outcome,
            changed,
        }),
        Err(e) => {
            tracing::warn!(file = %rel.as_str(), error = %e, "收录 TXT 失败，记为待人工确认");
            let outcome = IngestOutcome::Failed {
                reason: e.to_string(),
            };
            let changed = record_item(pool, rel, None, None, &outcome).await?;
            Ok(RescanItem {
                file_rel: rel.clone(),
                outcome,
                changed,
            })
        }
    }
}

/// 由任意绝对路径图片**复制**建一个图集包（作品库联动 / 手动导入用，保留原文件）。
/// 空列表返回 None。
pub async fn build_gallery_from_paths(
    pool: &SqlitePool,
    root: &Path,
    sku_id: i64,
    sku_code: &str,
    abs_paths: &[String],
    source: &str,
) -> AppResult<Option<i64>> {
    let imgs: Vec<&String> = abs_paths
        .iter()
        .filter(|p| media_kind(&paths::ascii_ext(p)) == Some("gallery"))
        .collect();
    if imgs.is_empty() {
        return Ok(None);
    }
    let sku_lib_rel = RelPath::from_parts([paths::ASSET_LIB, sku_code]);
    let sku_lib_abs = sku_lib_rel.to_local(root);
    std::fs::create_dir_all(&sku_lib_abs)?;
    let dir_name = paths::dedupe_name("gallery", &|n| sku_lib_abs.join(n).exists());
    let pack_rel = sku_lib_rel.join(&dir_name);
    let pack_abs = pack_rel.to_local(root);
    std::fs::create_dir_all(&pack_abs)?;

    let mut files: Vec<PackFile> = Vec::new();
    for (i, src_str) in imgs.iter().enumerate() {
        let src = Path::new(src_str.as_str());
        let ext = paths::ascii_ext(src_str);
        let new_name = paths::gallery_member(i + 1, &ext);
        let dest = pack_abs.join(&new_name);
        std::fs::copy(src, &dest)?;
        let bytes = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
        files.push(PackFile {
            name: new_name,
            orig_name: src
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string(),
            bytes,
        });
    }
    let files_json = serde_json::to_string(&files)?;
    let id = assets::insert(
        pool,
        &assets::NewPack {
            sku_id,
            kind: "gallery".into(),
            dir_rel: pack_rel.as_str().to_string(),
            files_json,
            cover: None,
            source: source.to_string(),
        },
    )
    .await?;
    Ok(Some(id))
}

/// 从一条成片 + 封面**拷贝**建视频型素材包（视频流水线 → 资产库）。
///
/// 与 `build_pack` 的区别是拷贝而非搬移：成片文件仍被 `v2v_clips.video_path` 引用，
/// 搬走它会让视频流水线里那条成片当场变成死链（点开验收过的成片播不了）。
/// 与 `build_gallery_from_paths` 同一形状，只是成员是一个视频 + 一张封面。
pub async fn build_video_from_paths(
    pool: &SqlitePool,
    root: &Path,
    sku_id: i64,
    sku_code: &str,
    video_abs: &str,
    poster_abs: Option<&str>,
    source: &str,
) -> AppResult<Option<i64>> {
    let ext = paths::ascii_ext(video_abs);
    if media_kind(&ext) != Some("video") {
        return Ok(None);
    }
    if !Path::new(video_abs).is_file() {
        return Ok(None);
    }
    let sku_lib_rel = RelPath::from_parts([paths::ASSET_LIB, sku_code]);
    let sku_lib_abs = sku_lib_rel.to_local(root);
    std::fs::create_dir_all(&sku_lib_abs)?;
    let dir_name = paths::dedupe_name("video", &|n| sku_lib_abs.join(n).exists());
    let pack_rel = sku_lib_rel.join(&dir_name);
    let pack_abs = pack_rel.to_local(root);
    std::fs::create_dir_all(&pack_abs)?;

    let mut files: Vec<PackFile> = Vec::new();
    let member = paths::video_member(&ext);
    let dest = pack_abs.join(&member);
    std::fs::copy(video_abs, &dest)?;
    files.push(PackFile {
        name: member,
        orig_name: file_name_of(video_abs),
        bytes: std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0),
    });

    // 封面：即梦成片的首帧就是那张验收图，所以封面一定拿得到（poster 是它的副本）。
    let mut cover_name: Option<String> = None;
    if let Some(p) = poster_abs.filter(|p| Path::new(p).is_file()) {
        let cext = paths::ascii_ext(p);
        let name = format!("cover.{cext}");
        let cdest = pack_abs.join(&name);
        std::fs::copy(p, &cdest)?;
        files.push(PackFile {
            name: name.clone(),
            orig_name: file_name_of(p),
            bytes: std::fs::metadata(&cdest).map(|m| m.len()).unwrap_or(0),
        });
        cover_name = Some(name);
    }

    let files_json = serde_json::to_string(&files)?;
    let id = assets::insert(
        pool,
        &assets::NewPack {
            sku_id,
            kind: "video".into(),
            dir_rel: pack_rel.as_str().to_string(),
            files_json,
            cover: cover_name,
            source: source.to_string(),
        },
    )
    .await?;
    Ok(Some(id))
}

fn file_name_of(p: &str) -> String {
    Path::new(p)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // 测试断言失败即测试失败，是期望行为
mod tests {
    use super::*;
    use crate::commands::publish_settings::ensure_partitions;
    use crate::db::test_support::test_pool;

    async fn seed_sku(pool: &SqlitePool, code: &str) -> i64 {
        skus::insert(
            pool,
            &skus::NewSku {
                code: code.into(),
                style_name: "款".into(),
                product_name: String::new(),
                tier: "warm".into(),
                topics_json: "[]".into(),
                platforms_json: None,
                note: String::new(),
            },
        )
        .await
        .unwrap()
    }

    fn write(root: &Path, rel: &str, content: &[u8]) {
        let p = RelPath::new(rel).to_local(root);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }

    #[tokio::test]
    async fn ingest_title_txt_known_sku() {
        let (pool, dir) = test_pool().await;
        let root = dir.path();
        ensure_partitions(root).unwrap();
        seed_sku(&pool, "SF-YD-201").await;
        write(
            root,
            "收件箱/SF-YD-201/标题_小红书.txt",
            "【SKU】SF-YD-201\n【平台】小红书\n【类型】标题\n\n标题一\n标题二\n".as_bytes(),
        );
        let rel = RelPath::new("收件箱/SF-YD-201/标题_小红书.txt");
        let outcome = ingest_txt(&pool, root, &rel, None).await.unwrap();
        assert_eq!(outcome.state_code(), "ingested");
        // 标题池两条
        let sku = skus::find_by_code(&pool, "SF-YD-201")
            .await
            .unwrap()
            .unwrap();
        let titles = texts::list(&pool, sku.id, "title").await.unwrap();
        assert_eq!(titles.len(), 2);
        assert_eq!(titles[0].platform, "xhs");
        // 原文件已归档，原位不存在
        assert!(!rel.to_local(root).exists());
    }

    #[tokio::test]
    async fn ingest_unknown_sku_stays_unclaimed() {
        let (pool, dir) = test_pool().await;
        let root = dir.path();
        ensure_partitions(root).unwrap();
        write(
            root,
            "收件箱/UNKNOWN/标题_通用.txt",
            "【类型】标题\n\n标题一\n".as_bytes(),
        );
        let rel = RelPath::new("收件箱/UNKNOWN/标题_通用.txt");
        let outcome = ingest_txt(&pool, root, &rel, None).await.unwrap();
        assert_eq!(outcome.state_code(), "unclaimed");
        // 文件保留原位
        assert!(rel.to_local(root).exists());
        assert_eq!(inbox::count_pending(&pool).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn ingest_topics_adopted_then_ignored() {
        let (pool, dir) = test_pool().await;
        let root = dir.path();
        ensure_partitions(root).unwrap();
        let sid = seed_sku(&pool, "AB-1").await;
        write(
            root,
            "收件箱/AB-1/标题_通用.txt",
            "【SKU】AB-1\n【类型】标题\n【话题】#沙发 #家居 #新品\n\nT1\n".as_bytes(),
        );
        ingest_txt(
            &pool,
            root,
            &RelPath::new("收件箱/AB-1/标题_通用.txt"),
            None,
        )
        .await
        .unwrap();
        let sku = skus::get(&pool, sid).await.unwrap().unwrap();
        let topics: Vec<String> = serde_json::from_str(&sku.topics_json).unwrap();
        assert_eq!(topics, vec!["沙发", "家居", "新品"]);
        // 第二个文件带不同话题 → 忽略，报差异
        write(
            root,
            "收件箱/AB-1/标题_抖音.txt",
            "【SKU】AB-1\n【类型】标题\n【话题】#别的\n\nT2\n".as_bytes(),
        );
        let out = ingest_txt(
            &pool,
            root,
            &RelPath::new("收件箱/AB-1/标题_抖音.txt"),
            None,
        )
        .await
        .unwrap();
        if let IngestOutcome::Ingested {
            topic_diff,
            topics_adopted,
            ..
        } = out
        {
            assert!(topics_adopted.is_empty());
            assert!(topic_diff.is_some());
        } else {
            panic!("应为 ingested");
        }
        // SKU 话题未被覆盖
        let sku = skus::get(&pool, sid).await.unwrap().unwrap();
        let topics: Vec<String> = serde_json::from_str(&sku.topics_json).unwrap();
        assert_eq!(topics, vec!["沙发", "家居", "新品"]);
    }

    #[tokio::test]
    async fn collect_gallery_and_video_packs() {
        let (pool, dir) = test_pool().await;
        let root = dir.path();
        ensure_partitions(root).unwrap();
        let sid = seed_sku(&pool, "SF-9").await;
        for n in [
            "img_b.jpg",
            "img_a.jpg",
            "cover.jpg",
            "clip.mp4",
            "clip_cover.jpg",
        ] {
            write(root, &format!("收件箱/SF-9/{n}"), b"x");
        }
        let ids = collect_media(&pool, root, "SF-9", None).await.unwrap();
        assert_eq!(ids.len(), 2, "1 图集 + 1 视频");
        let packs = assets::list_by_sku(&pool, sid).await.unwrap();
        let gallery = packs.iter().find(|p| p.kind == "gallery").unwrap();
        // 图序按文件名排序，img_a 在前 → img_01
        let files: serde_json::Value = serde_json::from_str(&gallery.files_json).unwrap();
        assert_eq!(files[0]["name"], "img_01.jpg");
        assert_eq!(files[0]["origName"], "img_a.jpg");
        assert_eq!(gallery.cover.as_deref(), Some("cover.jpg"));
        let video = packs.iter().find(|p| p.kind == "video").unwrap();
        assert_eq!(video.cover.as_deref(), Some("cover.jpg"));
        // 源文件夹已清空媒体（移入资产库）
        let moved = RelPath::new("资产库/SF-9").to_local(root);
        assert!(moved.is_dir());
    }

    #[tokio::test]
    async fn collect_resolves_alias_and_subfolders() {
        let (pool, dir) = test_pool().await;
        let root = dir.path();
        ensure_partitions(root).unwrap();
        let sid = seed_sku(&pool, "NFC-W-01").await;
        skus::set_alias(&pool, sid, "A-敖瑞鹏-01").await.unwrap();

        // 收件箱文件夹用中文别名命名，根目录散图 + 两个子文件夹各成图集。
        write(root, "收件箱/A-敖瑞鹏-01/img_a.jpg", b"x");
        write(root, "收件箱/A-敖瑞鹏-01/img_b.jpg", b"x");
        write(root, "收件箱/A-敖瑞鹏-01/图集1/p1.jpg", b"x");
        write(root, "收件箱/A-敖瑞鹏-01/图集1/cover.jpg", b"x");
        write(root, "收件箱/A-敖瑞鹏-01/图集2/q1.jpg", b"x");
        write(root, "收件箱/A-敖瑞鹏-01/图集2/q2.jpg", b"x");

        // 以磁盘上的中文目录名调用（rescan 即如此），应经别名解析到 NFC-W-01。
        let ids = collect_media(&pool, root, "A-敖瑞鹏-01", None)
            .await
            .unwrap();
        assert_eq!(ids.len(), 3, "根目录默认图集 + 两个子文件夹图集");
        let packs = assets::list_by_sku(&pool, sid).await.unwrap();
        assert_eq!(packs.iter().filter(|p| p.kind == "gallery").count(), 3);
        // 子文件夹图集封面被识别
        assert!(packs
            .iter()
            .any(|p| p.cover.as_deref() == Some("cover.jpg")));
        // 全部落在 资产库/NFC-W-01（用真实编码而非别名）
        assert!(RelPath::new("资产库/NFC-W-01").to_local(root).is_dir());
    }

    // A1：收件箱放图 → 自动入库 → **不做任何人工操作** → 排期能选中该包。
    // 旧行为 lifecycle=new 导致排期永远选不到，每个 SKU 恒报「无可用素材包」。
    #[tokio::test]
    async fn inbox_pack_is_schedulable_without_manual_activation() {
        use crate::commands::publish_settings::PublishSettings;
        use crate::db::repo::{accounts, planning};
        use crate::publish::planner;

        let (pool, dir) = test_pool().await;
        let root = dir.path();
        ensure_partitions(root).unwrap();
        // 热款：每日应发，排期与日期无关，断言稳定。
        let sid = skus::insert(
            &pool,
            &skus::NewSku {
                code: "SF-A1".into(),
                style_name: "款".into(),
                product_name: String::new(),
                tier: "hot".into(),
                topics_json: "[]".into(),
                platforms_json: Some("[\"xhs\"]".into()),
                note: String::new(),
            },
        )
        .await
        .unwrap();
        // 图集包需要标题 + 正文（内容类型由包类型决定）。
        for (kind, text) in [("title", "标题"), ("body", "正文")] {
            texts::insert(
                &pool,
                &texts::NewTextItem {
                    sku_id: sid,
                    kind: kind.into(),
                    text: text.into(),
                    platform: "general".into(),
                    source: "manual".into(),
                },
            )
            .await
            .unwrap();
        }
        accounts::insert(
            &pool,
            &accounts::NewAccount {
                platform: "xhs".into(),
                name: "号A".into(),
                daily_limit: 3,
                slots_json: None,
            },
        )
        .await
        .unwrap();
        write(root, "收件箱/SF-A1/a.jpg", b"x");

        // 自动收录（watcher 走的就是 rescan），全程无人工干预。
        let items = rescan(&pool, root).await.unwrap();
        assert!(items.iter().any(|i| i.outcome.state_code() == "ingested"));
        let packs = assets::list_by_sku(&pool, sid).await.unwrap();
        assert_eq!(packs.len(), 1);
        assert_eq!(packs[0].lifecycle, "active", "入库即可用");

        let s = PublishSettings {
            root_local: root.to_string_lossy().to_string(),
            time_slots: vec!["11:30-13:00".into()],
            ..PublishSettings::default()
        };
        let sheet_id = planner::generate_sheet(&pool, "2026-07-16", &s)
            .await
            .unwrap();
        let rows = planning::list_tasks_by_sheet(&pool, sheet_id)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1, "该 SKU 应被排入任务单");
        let set = planning::get_daily_set(&pool, rows[0].set_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(set.pack_id, packs[0].id, "选中的正是收件箱入库的那个包");
    }

    // A4：未知 SKU 的媒体文件夹进「待认领」（一个文件夹一条），认领后成包入库。
    #[tokio::test]
    async fn unknown_media_folder_becomes_unclaimed_then_claimable() {
        let (pool, dir) = test_pool().await;
        let root = dir.path();
        ensure_partitions(root).unwrap();
        write(root, "收件箱/未知款/1.jpg", b"x");
        write(root, "收件箱/未知款/2.jpg", b"x");

        let items = rescan(&pool, root).await.unwrap();
        let media: Vec<_> = items
            .iter()
            .filter(|i| matches!(i.outcome, IngestOutcome::UnclaimedMedia { .. }))
            .collect();
        assert_eq!(media.len(), 1, "50 张图也只出一条记录，按文件夹计");
        if let IngestOutcome::UnclaimedMedia { folder, files } = &media[0].outcome {
            assert_eq!(folder, "未知款");
            assert_eq!(*files, 2);
        }
        let rec = inbox::find_by_rel(&pool, "收件箱/未知款")
            .await
            .unwrap()
            .expect("待认领记录已落库");
        assert_eq!(rec.state, "unclaimed");
        assert_eq!(rec.kind.as_deref(), Some(KIND_MEDIA));

        // 人工指认 SKU → 强制归集成包。
        let sid = seed_sku(&pool, "SF-CLAIM").await;
        let ids = collect_media(&pool, root, "未知款", Some("SF-CLAIM"))
            .await
            .unwrap();
        assert_eq!(ids.len(), 1);
        let packs = assets::list_by_sku(&pool, sid).await.unwrap();
        assert_eq!(packs[0].dir_rel, "资产库/SF-CLAIM/gallery");
    }

    // A3：丢弃 = 移档。归档目录被 rescan 排除，故丢弃的东西不会被下一轮重新收录。
    #[tokio::test]
    async fn discarded_file_does_not_come_back_on_rescan() {
        let (pool, dir) = test_pool().await;
        let root = dir.path();
        ensure_partitions(root).unwrap();
        write(
            root,
            "收件箱/UNKNOWN/标题_通用.txt",
            "【类型】标题\n\n标题一\n".as_bytes(),
        );
        let rel = RelPath::new("收件箱/UNKNOWN/标题_通用.txt");
        let items = rescan(&pool, root).await.unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].outcome.state_code(), "unclaimed");

        // 丢弃（命令层同款动作：移档 + 记录转 discarded）。
        let archived = archive_to(root, &rel, paths::DISCARDED).unwrap();
        assert!(archived.as_str().starts_with("收件箱/已丢弃/"));
        assert!(archived.to_local(root).exists());
        assert!(!rel.to_local(root).exists());
        let rec = inbox::find_by_rel(&pool, rel.as_str())
            .await
            .unwrap()
            .unwrap();
        inbox::set_state(&pool, rec.id, "discarded", archived.as_str(), None)
            .await
            .unwrap();

        // 再扫一轮：不复活。
        let again = rescan(&pool, root).await.unwrap();
        assert!(again.is_empty(), "已丢弃目录不再被扫描：{again:?}");
        assert!(
            inbox::list(&pool, None).await.unwrap().is_empty(),
            "丢弃条目不出现在列表"
        );
        assert_eq!(inbox::count_pending(&pool).await.unwrap(), 0);
    }

    // A3：一个坏文件不阻断其余收录；滞留条目连续两轮只在第一轮报变化（toast 只弹一次）。
    #[tokio::test]
    async fn bad_file_does_not_block_others_and_stale_items_do_not_re_toast() {
        let (pool, dir) = test_pool().await;
        let root = dir.path();
        ensure_partitions(root).unwrap();
        seed_sku(&pool, "SF-OK").await;
        // 空内容 → 解析失败；另一份正常 TXT 必须照常入库。
        write(root, "收件箱/SF-OK/坏文件.txt", b"");
        write(
            root,
            "收件箱/SF-OK/标题_通用.txt",
            "【类型】标题\n\n标题一\n".as_bytes(),
        );

        let items = rescan(&pool, root).await.unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(
            items
                .iter()
                .filter(|i| i.outcome.state_code() == "ingested")
                .count(),
            1,
            "坏文件不阻断好文件"
        );
        assert_eq!(
            items
                .iter()
                .filter(|i| i.outcome.state_code() == "failed")
                .count(),
            1
        );
        assert!(items.iter().all(|i| i.changed), "首轮全是新条目");

        // 第二轮：好文件已归档不再扫到；坏文件滞留但状态未变 → changed=false（不再弹 toast）。
        let again = rescan(&pool, root).await.unwrap();
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].outcome.state_code(), "failed");
        assert!(!again[0].changed, "滞留条目状态未变，不该重复推事件");
    }

    #[tokio::test]
    async fn ingest_txt_resolves_by_alias() {
        let (pool, dir) = test_pool().await;
        let root = dir.path();
        ensure_partitions(root).unwrap();
        let sid = seed_sku(&pool, "NFC-W-02").await;
        skus::set_alias(&pool, sid, "B-张三-02").await.unwrap();
        // TXT 无【SKU】头，仅靠中文文件夹别名识别。
        write(
            root,
            "收件箱/B-张三-02/标题_小红书.txt",
            "【类型】标题\n\n标题一\n".as_bytes(),
        );
        let rel = RelPath::new("收件箱/B-张三-02/标题_小红书.txt");
        let outcome = ingest_txt(&pool, root, &rel, None).await.unwrap();
        assert_eq!(outcome.state_code(), "ingested");
        let titles = texts::list(&pool, sid, "title").await.unwrap();
        assert_eq!(titles.len(), 1);
    }
}
