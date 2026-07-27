//! trash 域命令（废纸篓，执行计划 2.1 / 需求 13.4）。
//! 清理 = 物理删文件 + 级联删记录 + 编号回收，不可恢复。

use std::path::PathBuf;

use serde::Serialize;
use specta::Type;
use tauri::State;

use crate::db::repo::tasks as task_repo;
use crate::db::repo::trash as repo;
use crate::db::repo::works as work_repo;
use crate::error::AppResult;
use crate::{files, ids};

/// 启动时到期自动清理（E22 批次 + E40 废纸篓，决策 D3）。
/// 各自 0 天 = 关闭。清理失败仅告警，不阻断启动。返回 (删除批次数, 清理废纸篓项数)。
pub async fn run_startup_cleanup(
    pool: &sqlx::SqlitePool,
    batch_retention_days: i64,
    trash_retention_days: i64,
) -> (u64, i64) {
    let now = crate::db::now_unix();
    let mut batches_deleted = 0u64;
    let mut trash_purged = 0i64;

    if batch_retention_days > 0 {
        let cutoff = now - batch_retention_days * 86_400;
        match task_repo::delete_batches_archived_before(pool, cutoff).await {
            Ok(n) => batches_deleted = n,
            Err(e) => tracing::warn!(error = %e, "归档批次自动清理失败"),
        }
    }
    if trash_retention_days > 0 {
        let cutoff = now - trash_retention_days * 86_400;
        match repo::expired_ids(pool, cutoff).await {
            Ok(ids) if !ids.is_empty() => match purge_ids(pool, &ids).await {
                Ok(n) => trash_purged = n,
                Err(e) => tracing::warn!(error = %e, "废纸篓到期项自动清理失败"),
            },
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "废纸篓到期项查询失败"),
        }
    }
    if batches_deleted > 0 || trash_purged > 0 {
        tracing::info!(batches_deleted, trash_purged, "启动自动清理完成（D3）");
    }
    // 启动补跑一次退休扫描：上次退出时可能正好在验收最后几张、或清完废纸篓就关了应用。
    // 条件是幂等的，扫一遍没事可做时它一行 SQL 就返回。
    crate::commands::batches::retire_batches_quietly(pool).await;
    (batches_deleted, trash_purged)
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TrashItemView {
    pub id: i64,
    pub entity_type: String,
    pub code: Option<String>,
    pub title: Option<String>,
    pub ref_name: Option<String>,
    pub thumb_path: Option<String>,
    /// 未通过任务的原图路径（E02：原图暂存至清理前可查看）。仅 task 类有值。
    pub image_path: Option<String>,
    pub prompt_text: Option<String>,
    pub source_label: String,
    pub deleted_at: i64,
    /// 能不能还原回原位。只有 0027 之前删掉的作品是 false（没留整行快照，还不回去），
    /// 其余四类的行一直都在，还原就是把状态拨回来。
    pub restorable: bool,
}

#[tauri::command]
#[specta::specta]
pub async fn list_trash(state: State<'_, crate::state::AppState>) -> AppResult<Vec<TrashItemView>> {
    let rows = repo::list(&state.db).await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            // 未通过任务的原图存于 file_paths 首位（E02）；仅 task 类暴露供查看。
            let image_path = (r.entity_type == "task")
                .then(|| {
                    serde_json::from_str::<Vec<String>>(&r.file_paths_json)
                        .ok()
                        .and_then(|v| v.into_iter().next())
                })
                .flatten();
            let restorable = r.entity_type != "work" || r.payload_json.is_some();
            TrashItemView {
                id: r.id,
                restorable,
                entity_type: r.entity_type,
                code: r.code,
                title: r.title,
                ref_name: None, // trash_items 不冗余参考图名；列表以编号 + 提示词为主
                thumb_path: r.thumb_path,
                image_path,
                prompt_text: r.prompt_text,
                source_label: r.source_label,
                deleted_at: r.deleted_at,
            }
        })
        .collect())
}

/// 拆分编号 `DZ-0001` → (前缀, 序号)。
fn parse_code(code: &str) -> Option<(String, i64)> {
    let (prefix, num) = code.rsplit_once('-')?;
    let n: i64 = num.trim().parse().ok()?;
    Some((prefix.to_string(), n))
}

async fn purge(state: &crate::state::AppState, ids_in: &[i64]) -> AppResult<i64> {
    let n = purge_ids(&state.db, ids_in).await?;
    // 清掉未通过结果，正是「这一批彻底了结了」的最后一步：批次的退休条件里第二条
    // 就是「没有本批的未通过结果还躺在废纸篓里」（那是还原按钮的锚点）。
    if n > 0 {
        crate::commands::batches::retire_batches_quietly(&state.db).await;
    }
    Ok(n)
}

/// 物理删 + 级联删记录 + 编号回收（同事务）。命令层与启动清理（E40）共用。
pub async fn purge_ids(pool: &sqlx::SqlitePool, ids_in: &[i64]) -> AppResult<i64> {
    let rows = repo::take(pool, ids_in).await?;
    if rows.is_empty() {
        return Ok(0);
    }

    // 1) 物理删文件（缩略图 + file_paths_json）。
    for r in &rows {
        if let Some(t) = &r.thumb_path {
            let _ = files::purge(&PathBuf::from(t));
        }
        if let Ok(paths) = serde_json::from_str::<Vec<String>>(&r.file_paths_json) {
            for p in paths {
                let _ = files::purge(&PathBuf::from(p));
            }
        }
    }

    // 2) 级联删记录 + 编号回收（同事务）。
    let mut tx = pool.begin().await?;
    for r in &rows {
        match r.entity_type.as_str() {
            "prompt" => {
                if let Some(code) = &r.code {
                    if let Some((prefix, n)) = parse_code(code) {
                        ids::recycle(&mut tx, &prefix, n).await?;
                    }
                }
                if let Some(pid) = r.ref_id {
                    sqlx::query("DELETE FROM prompts WHERE id = ?1")
                        .bind(pid)
                        .execute(&mut *tx)
                        .await?;
                }
            }
            "ref" => {
                if let Some(rid) = r.ref_id {
                    sqlx::query("DELETE FROM ref_images WHERE id = ?1")
                        .bind(rid)
                        .execute(&mut *tx)
                        .await?;
                }
            }
            _ => {}
        }
    }
    let ids_vec: Vec<i64> = rows.iter().map(|r| r.id).collect();
    repo::delete_rows(&mut tx, &ids_vec).await?;
    tx.commit().await?;

    Ok(rows.len() as i64)
}

/// 还原回执：还原了几条、几条还不回去（连同原因）。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RestoreResult {
    pub restored: i64,
    /// 还不回去的那几条为什么还不回去。空 = 全部成功。
    pub failures: Vec<String>,
}

/// 从废纸篓还原回原位（误删撤回）。
///
/// 五类实体走两条路：
/// - **task / prompt / ref / clip** —— 行一直都在，删除只是把状态拨到了一边，
///   还原就是把它拨回来（未通过 → 回待验收；提示词 → 回 active；参考图 → 清删除戳）。
/// - **work** —— 作品是唯一「删除即真删行」的实体（accepted_works 没有 deleted_at），
///   靠 0027 的 `payload_json` 整行写回，连 id 一起（v2v_clips.work_id 是不设 FK 的锚点，
///   换个新 id 等于把那条视频认领给了别人）。
///
/// 还原**不删** trash 行以外的任何东西，也不动文件：未通过的原图本来就还在盘上
/// （E02 决定的：reject 只是记账，物理删要等「彻底删除/清空」）。这正是它能还原的前提。
#[tauri::command]
#[specta::specta]
pub async fn restore_trash_items(
    state: State<'_, crate::state::AppState>,
    ids: Vec<i64>,
) -> AppResult<RestoreResult> {
    let rows = repo::take(&state.db, &ids).await?;
    let mut restored: Vec<i64> = Vec::new();
    let mut failures: Vec<String> = Vec::new();

    for r in &rows {
        let label = r.code.clone().unwrap_or_else(|| r.source_label.clone());
        let ok: Result<bool, sqlx::Error> = match r.entity_type.as_str() {
            "task" => restore_by_id(
                &state.db,
                r.ref_id,
                "UPDATE tasks SET status = 'rev', updated_at = ?2 WHERE id = ?1 AND status = 'rej'",
            )
            .await,
            "prompt" => restore_by_id(
                &state.db,
                r.ref_id,
                "UPDATE prompts SET status = 'active', updated_at = ?2 WHERE id = ?1",
            )
            .await,
            // ref_images 没有 updated_at，故这条不带时间戳。
            "ref" => match r.ref_id {
                Some(id) => sqlx::query("UPDATE ref_images SET deleted_at = NULL WHERE id = ?1")
                    .bind(id)
                    .execute(&state.db)
                    .await
                    .map(|x| x.rows_affected() > 0),
                None => Ok(false),
            },
            "clip" => restore_by_id(
                &state.db,
                r.ref_id,
                "UPDATE v2v_clips SET stage = 'rev', reviewed_at = NULL, finished_at = COALESCE(finished_at, ?2), updated_at = ?2
                 WHERE id = ?1 AND stage = 'rej'",
            )
            .await,
            "work" => match r
                .payload_json
                .as_deref()
                .and_then(|j| serde_json::from_str::<work_repo::AcceptedWorkRow>(j).ok())
            {
                Some(row) => work_repo::restore(&state.db, &row).await.map(|()| true),
                None => {
                    // 0027 之前删掉的作品没有载荷可还原。说清楚而不是假装成功——
                    // 「点了还原、作品库里却没有」比直接说还不回去更难查。
                    failures.push(format!("{label}：这条是旧版本删除的，没有可还原的记录"));
                    continue;
                }
            },
            other => {
                failures.push(format!("{label}：不认识的类型「{other}」"));
                continue;
            }
        };
        match ok {
            Ok(true) => restored.push(r.id),
            Ok(false) => {
                failures.push(format!("{label}：原记录已不在，或已经不是「已删除」的状态"))
            }
            Err(e) => failures.push(format!("{label}：{e}")),
        }
    }

    // 只删还原成功的那几行废纸篓记录；失败的留着，人还能看见它、还能彻底删。
    if !restored.is_empty() {
        let mut tx = state.db.begin().await?;
        repo::delete_rows(&mut tx, &restored).await?;
        tx.commit().await?;
    }
    Ok(RestoreResult {
        restored: restored.len() as i64,
        failures,
    })
}

/// 跑一条「按 id 把状态拨回去」的 UPDATE（`?1` = id，`?2` = 当前时刻）；
/// 返回是否真的改到了行 —— 改不到就说明原记录已经不在了，那要如实报出来。
async fn restore_by_id(
    pool: &sqlx::SqlitePool,
    ref_id: Option<i64>,
    sql: &str,
) -> Result<bool, sqlx::Error> {
    let Some(id) = ref_id else { return Ok(false) };
    let n = sqlx::query(sql)
        .bind(id)
        .bind(crate::db::now_unix())
        .execute(pool)
        .await?
        .rows_affected();
    Ok(n > 0)
}

#[tauri::command]
#[specta::specta]
pub async fn purge_trash_items(
    state: State<'_, crate::state::AppState>,
    ids: Vec<i64>,
) -> AppResult<i64> {
    purge(&state, &ids).await
}

#[tauri::command]
#[specta::specta]
pub async fn purge_all_trash(state: State<'_, crate::state::AppState>) -> AppResult<i64> {
    let all = repo::all(&state.db).await?;
    let ids: Vec<i64> = all.iter().map(|r| r.id).collect();
    purge(&state, &ids).await
}

#[tauri::command]
#[specta::specta]
pub async fn count_trash(state: State<'_, crate::state::AppState>) -> AppResult<i64> {
    Ok(repo::count(&state.db).await?)
}
