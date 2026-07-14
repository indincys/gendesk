//! 素材包域命令（发布模块执行计划 4.1 assets 域）。
//!
//! 生命周期派生：new|active|retired 为存储态；exhausted（已用尽/冷却中）由台账 + 查重窗口
//! 在查询时计算（前置事实 5）。锁定校验：被状态 ≠ closed 的任务单引用的包禁改/删/退役。

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::State;

use crate::commands::publish_settings::{self, PublishSettings};
use crate::db::repo::{assets as repo, ledger, skus, works as works_repo};
use crate::error::{AppError, AppResult};
use crate::publish::inbox::ingest;
use crate::publish::paths::RelPath;
use crate::publish::platform::Platform;
use crate::state::AppState;

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PackFileView {
    pub name: String,
    pub orig_name: String,
    pub bytes: i64,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PackView {
    pub id: i64,
    pub sku_id: i64,
    pub kind: String,
    pub dir_rel: String,
    pub files: Vec<PackFileView>,
    pub cover: Option<String>,
    /// 缩略图绝对本地路径（前端 assetSrc 读）：封面优先，无封面取包内首张图片；
    /// 无封面的视频包为 None（V1 不抽帧）。
    pub thumb_path: Option<String>,
    /// 存储态：new|active|retired。
    pub lifecycle: String,
    /// 派生态：new|active|exhausted|retired。
    pub derived: String,
    /// 回可用日期（exhausted 时的最早解冻 Unix 秒）。
    pub available_at: Option<i64>,
    /// 被未关闭任务单引用（锁定）。
    pub locked: bool,
    pub source: String,
    pub note: String,
    pub file_count: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

/// 派生生命周期（纯函数，前置事实 5）：
/// - retired/new 原样返回；
/// - active：若全部启用平台都在查重窗口内（有近发布）→ exhausted，available_at = 最早解冻时刻；否则 active。
///
/// `last_pub`：该包各平台最近一次发布时间（platform code → Unix 秒）。
/// `enabled_platforms`：SKU 生效的平台 code 集（空则视为 active，不判用尽）。
fn derive_lifecycle(
    stored: &str,
    last_pub: &[(String, i64)],
    enabled_platforms: &[String],
    dedup_days: i64,
    now: i64,
) -> (String, Option<i64>) {
    if stored == "retired" || stored == "new" {
        return (stored.to_string(), None);
    }
    if enabled_platforms.is_empty() {
        return ("active".to_string(), None);
    }
    let window = dedup_days.max(0) * 86_400;
    let mut earliest_free: Option<i64> = None;
    for p in enabled_platforms {
        let last = last_pub.iter().find(|(plat, _)| plat == p).map(|(_, t)| *t);
        match last {
            Some(t) if t + window > now => {
                let free_at = t + window;
                earliest_free = Some(earliest_free.map_or(free_at, |e: i64| e.min(free_at)));
            }
            // 该平台不在窗口内 → 仍可用，整体未用尽。
            _ => return ("active".to_string(), None),
        }
    }
    ("exhausted".to_string(), earliest_free)
}

/// SKU 生效平台（覆盖优先，否则全局矩阵）。
fn enabled_platforms(sku_platforms: Option<&Vec<String>>, s: &PublishSettings) -> Vec<String> {
    if let Some(over) = sku_platforms {
        return over.clone();
    }
    let m = &s.platform_matrix;
    Platform::ALL
        .into_iter()
        .filter(|p| match p {
            Platform::Douyin => m.douyin,
            Platform::Xhs => m.xhs,
            Platform::Kuaishou => m.kuaishou,
            Platform::Shipinhao => m.shipinhao,
            Platform::Bilibili => m.bilibili,
        })
        .map(|p| p.code().to_string())
        .collect()
}

async fn to_view(state: &AppState, r: repo::PackRow, s: &PublishSettings) -> AppResult<PackView> {
    let files: Vec<PackFileView> = serde_json::from_str::<Vec<serde_json::Value>>(&r.files_json)
        .unwrap_or_default()
        .into_iter()
        .map(|v| PackFileView {
            name: v
                .get("name")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
            orig_name: v
                .get("origName")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
            bytes: v.get("bytes").and_then(|x| x.as_i64()).unwrap_or(0),
        })
        .collect();

    // 派生生命周期：读该 SKU 生效平台 + 该包各平台最近发布。
    let sku = skus::get(&state.db, r.sku_id).await?;
    let sku_platforms = sku
        .as_ref()
        .and_then(|s| s.platforms_json.as_deref())
        .and_then(|j| serde_json::from_str::<Vec<String>>(j).ok());
    let platforms = enabled_platforms(sku_platforms.as_ref(), s);

    let now = crate::db::now_unix();
    let mut conn = state.db.acquire().await?;
    let mut last_pub = Vec::new();
    for p in &platforms {
        if let Some(t) = ledger::last_publish_in_window(&mut conn, r.id, p, 0).await? {
            last_pub.push((p.clone(), t));
        }
    }
    drop(conn);
    let (derived, available_at) =
        derive_lifecycle(&r.lifecycle, &last_pub, &platforms, s.dedup_days, now);

    let locked = repo::is_locked(&state.db, r.id).await?;

    // 缩略图：封面优先，否则包内首张图片（视频包无封面时没有缩略图，V1 不抽帧）。
    let thumb_name = r.cover.clone().or_else(|| {
        files
            .iter()
            .find(|f| {
                matches!(
                    crate::publish::paths::ascii_ext(&f.name).as_str(),
                    "jpg" | "jpeg" | "png" | "webp"
                )
            })
            .map(|f| f.name.clone())
    });
    let thumb_path = match (
        publish_settings::root_local(&state.db).await.ok(),
        thumb_name,
    ) {
        (Some(root), Some(name)) => {
            let p = RelPath::new(&r.dir_rel).join(&name).to_local(&root);
            Some(p.to_string_lossy().to_string())
        }
        _ => None,
    };

    Ok(PackView {
        id: r.id,
        sku_id: r.sku_id,
        kind: r.kind,
        dir_rel: r.dir_rel,
        file_count: files.len() as i64,
        files,
        cover: r.cover,
        thumb_path,
        lifecycle: r.lifecycle,
        derived,
        available_at,
        locked,
        source: r.source,
        note: r.note,
        created_at: r.created_at,
        updated_at: r.updated_at,
    })
}

#[tauri::command]
#[specta::specta]
pub async fn list_asset_packs(state: State<'_, AppState>, sku_id: i64) -> AppResult<Vec<PackView>> {
    let s = publish_settings::load(&state.db).await?;
    let rows = repo::list_by_sku(&state.db, sku_id).await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        out.push(to_view(&state, r, &s).await?);
    }
    Ok(out)
}

/// 手动导入素材文件（走与收件箱相同的归集/命名管线）。
#[tauri::command]
#[specta::specta]
pub async fn import_media_files(
    state: State<'_, AppState>,
    sku_id: i64,
    paths: Vec<String>,
) -> AppResult<Vec<PackView>> {
    let root = publish_settings::root_local(&state.db).await?;
    let sku = skus::get(&state.db, sku_id)
        .await?
        .ok_or_else(|| AppError::InvalidInput("SKU 不存在".into()))?;
    // 复制到 收件箱/{SKU}/ 暂存 → 复用归集管线。
    let inbox_rel = RelPath::from_parts([crate::publish::paths::INBOX, &sku.code]);
    let inbox_abs = inbox_rel.to_local(&root);
    std::fs::create_dir_all(&inbox_abs)?;
    for p in &paths {
        let src = std::path::Path::new(p);
        if let Some(name) = src.file_name() {
            std::fs::copy(src, inbox_abs.join(name))?;
        }
    }
    let ids = ingest::collect_media(&state.db, &root, &sku.code, Some(&sku.code)).await?;
    let s = publish_settings::load(&state.db).await?;
    let mut out = Vec::new();
    for id in ids {
        if let Some(r) = repo::get(&state.db, id).await? {
            out.push(to_view(&state, r, &s).await?);
        }
    }
    Ok(out)
}

/// 作品库联动：把选中的输出图复制为一个图集包。
#[tauri::command]
#[specta::specta]
pub async fn pack_from_works(
    state: State<'_, AppState>,
    sku_id: i64,
    work_ids: Vec<i64>,
) -> AppResult<Option<PackView>> {
    let root = publish_settings::root_local(&state.db).await?;
    let sku = skus::get(&state.db, sku_id)
        .await?
        .ok_or_else(|| AppError::InvalidInput("SKU 不存在".into()))?;
    let mut abs_paths = Vec::new();
    for wid in &work_ids {
        if let Some(w) = works_repo::get(&state.db, *wid).await? {
            abs_paths.push(w.image_path);
        }
    }
    let id =
        ingest::build_gallery_from_paths(&state.db, &root, sku_id, &sku.code, &abs_paths, "works")
            .await?;
    match id {
        Some(id) => {
            let s = publish_settings::load(&state.db).await?;
            let r = repo::get(&state.db, id)
                .await?
                .ok_or_else(|| AppError::Internal("新建包读取失败".into()))?;
            Ok(Some(to_view(&state, r, &s).await?))
        }
        None => Ok(None),
    }
}

async fn ensure_unlocked(state: &AppState, id: i64) -> AppResult<()> {
    if repo::is_locked(&state.db, id).await? {
        return Err(AppError::InvalidInput(
            "该素材包被未关闭的任务单引用，无法删除/退役/移动".into(),
        ));
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn retire_pack(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    ensure_unlocked(&state, id).await?;
    repo::set_lifecycle(&state.db, id, "retired").await?;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn restore_pack(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    repo::set_lifecycle(&state.db, id, "active").await?;
    Ok(())
}

/// 删除素材包：校验锁定 → 物理删目录 + 删记录。
#[tauri::command]
#[specta::specta]
pub async fn delete_pack(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    ensure_unlocked(&state, id).await?;
    let pack = repo::get(&state.db, id)
        .await?
        .ok_or_else(|| AppError::InvalidInput("素材包不存在".into()))?;
    if let Ok(root) = publish_settings::root_local(&state.db).await {
        let dir = RelPath::new(&pack.dir_rel).to_local(&root);
        if dir.is_dir() {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
    repo::delete(&state.db, id).await?;
    Ok(())
}

#[derive(Debug, Clone, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PackPatch {
    pub note: Option<String>,
    /// `Some(None)` = 清除封面；`Some(Some)` = 设为该包内文件名。
    pub cover: Option<Option<String>>,
}

#[tauri::command]
#[specta::specta]
pub async fn update_pack(state: State<'_, AppState>, id: i64, patch: PackPatch) -> AppResult<()> {
    let cover_arg: Option<Option<&str>> = patch.cover.as_ref().map(|o| o.as_deref());
    repo::update_fields(&state.db, id, patch.note.as_deref(), cover_arg).await?;
    // 生命周期只由显式路径改（activate_pack / retire_pack / restore_pack）：
    // 改个备注就顺带让包参与排期是意料之外的副作用。
    Ok(())
}

/// 手动把 new 包标记为 active（可用）。
#[tauri::command]
#[specta::specta]
pub async fn activate_pack(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    repo::set_lifecycle(&state.db, id, "active").await?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即测试失败，是期望行为
mod tests {
    use super::*;

    const DAY: i64 = 86_400;

    #[test]
    fn derive_new_and_retired_passthrough() {
        assert_eq!(
            derive_lifecycle("new", &[], &["xhs".into()], 30, 0).0,
            "new"
        );
        assert_eq!(
            derive_lifecycle("retired", &[], &["xhs".into()], 30, 0).0,
            "retired"
        );
    }

    #[test]
    fn derive_active_when_no_platforms() {
        assert_eq!(derive_lifecycle("active", &[], &[], 30, 0).0, "active");
    }

    #[test]
    fn derive_active_when_any_platform_free() {
        let now = 100 * DAY;
        let last = vec![("xhs".into(), now - 5 * DAY)]; // xhs 近发布
                                                        // 抖音无记录 → 仍可用
        let (d, at) = derive_lifecycle("active", &last, &["xhs".into(), "douyin".into()], 30, now);
        assert_eq!(d, "active");
        assert_eq!(at, None);
    }

    #[test]
    fn derive_exhausted_when_all_in_window() {
        let now = 100 * DAY;
        let last = vec![
            ("xhs".into(), now - 5 * DAY),     // 解冻 now+25d
            ("douyin".into(), now - 10 * DAY), // 解冻 now+20d（更早）
        ];
        let (d, at) = derive_lifecycle("active", &last, &["xhs".into(), "douyin".into()], 30, now);
        assert_eq!(d, "exhausted");
        assert_eq!(at, Some(now - 10 * DAY + 30 * DAY)); // 最早解冻
    }

    #[test]
    fn derive_active_when_window_passed() {
        let now = 100 * DAY;
        let last = vec![("xhs".into(), now - 40 * DAY)]; // 超过 30 天窗口
        let (d, _) = derive_lifecycle("active", &last, &["xhs".into()], 30, now);
        assert_eq!(d, "active");
    }
}
