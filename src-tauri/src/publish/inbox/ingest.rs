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

/// 单文件收录结果（供事件与报告使用）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum IngestOutcome {
    /// 成功入库并归档。
    Ingested {
        sku_code: String,
        kind: String,
        titles: usize,
        bodies: usize,
        /// 采纳的话题（SKU 原先无话题时）。
        topics_adopted: Vec<String>,
        /// 话题差异提示（SKU 已有话题、忽略了本文件话题）。
        topic_diff: Option<String>,
    },
    /// 识别不出已知 SKU，待认领。
    Unclaimed { sku_code: Option<String> },
    /// 解析失败，待人工确认。
    Failed { reason: String },
}

impl IngestOutcome {
    pub fn state_code(&self) -> &'static str {
        match self {
            IngestOutcome::Ingested { .. } => "ingested",
            IngestOutcome::Unclaimed { .. } => "unclaimed",
            IngestOutcome::Failed { .. } => "failed",
        }
    }
}

/// 媒体扩展名分类。
fn media_kind(ext: &str) -> Option<&'static str> {
    match ext.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" | "png" | "webp" => Some("gallery"),
        "mp4" | "mov" => Some("video"),
        _ => None,
    }
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
            record_item(pool, file_rel, None, None, &outcome).await?;
            return Ok(outcome);
        }
    };

    let sku_candidate = forced_sku
        .map(str::to_string)
        .or_else(|| parser::resolve_sku(parsed.sku_code.as_deref(), &filename, folder.as_deref()));

    // 查已知 SKU。
    let sku = match &sku_candidate {
        Some(code) => skus::find_by_code(pool, code).await?,
        None => None,
    };
    let Some(sku) = sku else {
        let outcome = IngestOutcome::Unclaimed {
            sku_code: sku_candidate.clone(),
        };
        record_item(
            pool,
            file_rel,
            Some(parsed.kind.code()),
            sku_candidate.as_deref(),
            &outcome,
        )
        .await?;
        return Ok(outcome);
    };

    // 已知 SKU：单事务入库（标题/正文 + 话题采纳）。
    let platform_tag = parsed
        .platform
        .as_deref()
        .map(platform::text_platform_tag)
        .unwrap_or_else(|| platform::GENERAL_TAG.to_string());

    let (topics_adopted, topic_diff) =
        resolve_topics(pool, sku.id, &sku.topics_json, &parsed.topics).await?;

    let mut tx = pool.begin().await?;
    for t in &parsed.titles {
        texts::insert_tx(
            &mut tx,
            &texts::NewTextItem {
                sku_id: sku.id,
                kind: "title".into(),
                text: t.clone(),
                platform: platform_tag.clone(),
                source: "inbox".into(),
            },
        )
        .await?;
    }
    for b in &parsed.bodies {
        texts::insert_tx(
            &mut tx,
            &texts::NewTextItem {
                sku_id: sku.id,
                kind: "body".into(),
                text: b.clone(),
                platform: platform_tag.clone(),
                source: "inbox".into(),
            },
        )
        .await?;
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
        titles: parsed.titles.len(),
        bodies: parsed.bodies.len(),
        topics_adopted,
        topic_diff,
    };
    record_item(
        pool,
        &archived_rel,
        Some(parsed.kind.code()),
        Some(&sku.code),
        &outcome,
    )
    .await?;
    Ok(outcome)
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
async fn record_item(
    pool: &SqlitePool,
    file_rel: &RelPath,
    kind: Option<&str>,
    sku_code: Option<&str>,
    outcome: &IngestOutcome,
) -> AppResult<()> {
    let detail = serde_json::to_string(outcome).ok();
    if let Some(existing) = inbox::find_by_rel(pool, file_rel.as_str()).await? {
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
    }
    Ok(())
}

/// 移动原文件到 收件箱/已收录/{YYYYMMDD}/，返回归档后相对路径。
fn archive_ingested(root: &Path, file_rel: &RelPath) -> AppResult<RelPath> {
    let src = file_rel.to_local(root);
    let filename = src
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| AppError::Io("收录文件无文件名".into()))?
        .to_string();
    let dir_rel = RelPath::from_parts([paths::INBOX, paths::INGESTED, &today_yyyymmdd()]);
    let dir_abs = dir_rel.to_local(root);
    std::fs::create_dir_all(&dir_abs)?;
    // 去重命名。
    let final_name = paths::dedupe_name(&filename, &|n| dir_abs.join(n).exists());
    let dest_rel = dir_rel.join(&final_name);
    std::fs::rename(&src, dest_rel.to_local(root))?;
    Ok(dest_rel)
}

/// 收集一个 SKU 文件夹内的媒体文件为素材包（前置事实 10），移入资产库。
/// 返回新建包的 id 列表。文件按名排序定图序；`cover.jpg`/`<video>_cover.jpg` 识别为封面。
pub async fn collect_media(pool: &SqlitePool, root: &Path, sku_code: &str) -> AppResult<Vec<i64>> {
    let sku = match skus::find_by_code(pool, sku_code).await? {
        Some(s) => s,
        None => return Ok(Vec::new()),
    };
    let folder_rel = RelPath::from_parts([paths::INBOX, sku_code]);
    let folder_abs = folder_rel.to_local(root);
    if !folder_abs.is_dir() {
        return Ok(Vec::new());
    }

    // 枚举媒体文件（忽略隐藏/锁文件）。
    let mut galleries: Vec<(String, String)> = Vec::new(); // (filename, ext)
    let mut videos: Vec<(String, String)> = Vec::new();
    let mut covers: Vec<String> = Vec::new(); // 潜在封面文件名
    for entry in std::fs::read_dir(&folder_abs)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || name.starts_with("~$") {
            continue;
        }
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
            "gallery",
            &gallery_imgs,
            gallery_cover.map(|s| s.as_str()),
            &folder_abs,
        )
        .await?;
        created.push(id);
    }

    // 视频包：每个视频 → 1 包；`<video>_cover.jpg` 为其封面。
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
            &folder_abs,
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
    // 成员文件重命名：图集 img_NN.ext，视频 video.ext。
    for (i, (orig, ext)) in members.iter().enumerate() {
        let new_name = if kind == "gallery" {
            paths::gallery_member(i + 1, ext)
        } else {
            paths::video_member(ext)
        };
        let src = src_folder.join(orig);
        let dest = pack_abs.join(&new_name);
        let bytes = std::fs::metadata(&src).map(|m| m.len()).unwrap_or(0);
        std::fs::rename(&src, &dest)?;
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
            let bytes = std::fs::metadata(&src).map(|m| m.len()).unwrap_or(0);
            std::fs::rename(&src, &dest)?;
            files.push(PackFile {
                name: new_name.clone(),
                orig_name: orig.to_string(),
                bytes,
            });
            cover_name = Some(new_name);
        }
    }

    let files_json = serde_json::to_string(&files)?;
    let id = assets::insert(
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
    .await?;
    Ok(id)
}

/// 全量扫描收件箱：各 SKU 子目录归集媒体 + 收录全部 TXT（排除 已收录/）。
/// 启动补跑与手动「重扫收件箱」共用。
pub async fn rescan(pool: &SqlitePool, root: &Path) -> AppResult<Vec<IngestOutcome>> {
    let inbox_abs = RelPath::from_parts([paths::INBOX]).to_local(root);
    if !inbox_abs.is_dir() {
        return Ok(Vec::new());
    }
    // 1) 各直接子目录（SKU 文件夹）归集媒体（已收录/ 跳过；未知 SKU 归集为 noop）。
    for entry in std::fs::read_dir(&inbox_abs)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name == paths::INGESTED {
            continue;
        }
        collect_media(pool, root, &name).await?;
    }
    // 2) 收录全部 TXT（排除 已收录/ 与隐藏/锁文件）。
    let mut outcomes = Vec::new();
    for entry in walkdir::WalkDir::new(&inbox_abs)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if path.components().any(|c| c.as_os_str() == paths::INGESTED) {
            continue;
        }
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if name.starts_with('.') || name.starts_with("~$") {
            continue;
        }
        if !name.to_ascii_lowercase().ends_with(".txt") {
            continue;
        }
        if let Ok(sub) = path.strip_prefix(root) {
            let rel = RelPath::new(sub.to_string_lossy());
            outcomes.push(ingest_txt(pool, root, &rel, None).await?);
        }
    }
    Ok(outcomes)
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
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
        let ids = collect_media(&pool, root, "SF-9").await.unwrap();
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
}
