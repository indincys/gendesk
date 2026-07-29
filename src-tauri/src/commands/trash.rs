//! trash 域命令（废纸篓，执行计划 2.1 / 需求 13.4）。
//! 清理 = 物理删文件 + 级联删记录 + 编号回收，不可恢复。

use std::path::PathBuf;

use serde::Serialize;
use specta::Type;
use tauri::State;

use crate::db::repo::tasks as task_repo;
use crate::db::repo::trash as repo;
use crate::db::repo::v2v as v2v_repo;
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
    // 排队位次采样是**观测窗口**，保留期固定不进设置：它既不是业务真相，也没人会想
    // 去调它 —— 排产看的是最近这段时间队列多快，而半年前那一周只是在占索引。
    {
        let cutoff = now - crate::v2v::runner::QUEUE_SAMPLE_RETENTION_DAYS * 86_400;
        match crate::db::repo::v2v::prune_queue_samples(pool, cutoff).await {
            Ok(n) if n > 0 => tracing::info!(pruned = n, "排队位次采样到期清理"),
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "排队位次采样清理失败"),
        }
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
    /// 缩略图像素（0031）。网格按真实比例排版，行高要在渲染前算得出来 ——
    /// 等图片加载完再量，每张落地都会把下面的行往下顶一次。测不到就留 None，
    /// 前端拿一个中性比例兜底，绝不为了排版猜一个假尺寸。
    pub width: Option<i64>,
    pub height: Option<i64>,
    /// 它当初属于哪一批（拿得到的话）。
    ///
    /// 废纸篓的分段规则里，「不是同一批」与「隔了很久」同为切段依据 —— 只按时间切的话，
    /// 一次连着清两批的操作会被并成一段，而人来这一页恰恰是想认出「那次任务」。
    /// 三类拿得到（未通过任务 / 视频 / 作品快照），提示词与参考图没有批次概念。
    pub batch_id: Option<i64>,
    /// 写出这条词的 skill（0032）。一批图整体歪掉时，第一个要问的就是它。
    ///
    /// 三类顺着 `prompt_id` 找得回来（未通过任务 / 提示词本身 / 作品快照）。
    /// 参考图没有词；视频要再跳两张表（clip → 作品 → 词），而视频的词是 v2v 改写
    /// 出来的、不是生图 skill 写的，报生图 skill 反而是错的答案 —— 故两类留 None。
    pub skill: Option<String>,
}

#[tauri::command]
#[specta::specta]
pub async fn list_trash(state: State<'_, crate::state::AppState>) -> AppResult<Vec<TrashItemView>> {
    let rows = repo::list(&state.db).await?;
    let batches = batch_ids_of(&state.db, &rows).await;
    let skills = skills_of(&state.db, &rows).await;
    let mut out: Vec<TrashItemView> = rows
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
                batch_id: batches.get(&r.id).copied(),
                skill: skills.get(&r.id).cloned(),
                width: r.thumb_w,
                height: r.thumb_h,
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
        .collect();
    backfill_thumb_sizes(&state.db, &mut out).await;
    Ok(out)
}

/// 每条废纸篓项的批次归属。拿不到的（提示词 / 参考图 / 源行已随批次退休）不进表。
///
/// `ref_id` 在五类之间指向不同的表，故一条 SQL 拼不出来 —— 两次按类批量查，
/// 作品那一类直接读整行快照（它的源行已经被真删了，只有快照里还有）。
async fn batch_ids_of(
    pool: &sqlx::SqlitePool,
    rows: &[crate::db::repo::trash::TrashItemRow],
) -> std::collections::HashMap<i64, i64> {
    let mut out = std::collections::HashMap::new();
    for (entity, table) in [("task", "tasks"), ("clip", "v2v_clips")] {
        let refs: Vec<(i64, i64)> = rows
            .iter()
            .filter(|r| r.entity_type == entity)
            .filter_map(|r| r.ref_id.map(|rid| (r.id, rid)))
            .collect();
        if refs.is_empty() {
            continue;
        }
        let ph = refs.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!("SELECT id, batch_id FROM {table} WHERE id IN ({ph})");
        let mut q = sqlx::query_as::<_, (i64, Option<i64>)>(&sql);
        for (_, rid) in &refs {
            q = q.bind(rid);
        }
        // 查不动（表没了之类）不该让整页打不开 —— 分段退化成纯按时间切，仍然可用。
        let Ok(found) = q.fetch_all(pool).await else {
            tracing::warn!(entity, "废纸篓批次归属查询失败，本次分段仅按时间切");
            continue;
        };
        for (trash_id, ref_id) in refs {
            if let Some((_, Some(b))) = found.iter().find(|(id, _)| *id == ref_id) {
                out.insert(trash_id, *b);
            }
        }
    }
    for r in rows.iter().filter(|r| r.entity_type == "work") {
        if let Some(b) = r
            .payload_json
            .as_deref()
            .and_then(|j| serde_json::from_str::<serde_json::Value>(j).ok())
            .and_then(|v| v.get("batch_id").and_then(serde_json::Value::as_i64))
        {
            out.insert(r.id, b);
        }
    }
    out
}

/// 每条废纸篓项背后那条词是哪个 skill 写的（0032）。
///
/// 三类顺着 `prompt_id` 找得回来：未通过任务（`tasks.prompt_id`）、提示词本身
/// （`ref_id` 就是它）、作品（还原快照里的 `prompt_id` —— 它的源行已被真删）。
/// 找不到的（词已随批次退休、参考图、视频）不进表，界面就不显示标。
async fn skills_of(
    pool: &sqlx::SqlitePool,
    rows: &[crate::db::repo::trash::TrashItemRow],
) -> std::collections::HashMap<i64, String> {
    // trash_id → prompt_id
    let mut want: Vec<(i64, i64)> = Vec::new();
    let mut task_refs: Vec<(i64, i64)> = Vec::new();
    for r in rows {
        match r.entity_type.as_str() {
            "prompt" => {
                if let Some(pid) = r.ref_id {
                    want.push((r.id, pid));
                }
            }
            "task" => {
                if let Some(tid) = r.ref_id {
                    task_refs.push((r.id, tid));
                }
            }
            "work" => {
                if let Some(pid) = r
                    .payload_json
                    .as_deref()
                    .and_then(|j| serde_json::from_str::<serde_json::Value>(j).ok())
                    .and_then(|v| v.get("prompt_id").and_then(serde_json::Value::as_i64))
                {
                    want.push((r.id, pid));
                }
            }
            _ => {}
        }
    }
    if !task_refs.is_empty() {
        let ph = task_refs.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!("SELECT id, prompt_id FROM tasks WHERE id IN ({ph})");
        let mut q = sqlx::query_as::<_, (i64, i64)>(&sql);
        for (_, tid) in &task_refs {
            q = q.bind(tid);
        }
        match q.fetch_all(pool).await {
            Ok(found) => {
                for (trash_id, task_id) in task_refs {
                    if let Some((_, pid)) = found.iter().find(|(id, _)| *id == task_id) {
                        want.push((trash_id, *pid));
                    }
                }
            }
            // 查不动不该让整页打不开 —— 少一个标而已。
            Err(e) => tracing::warn!(error = %e, "废纸篓 skill 归属：任务→提示词查询失败"),
        }
    }
    let mut out = std::collections::HashMap::new();
    if want.is_empty() {
        return out;
    }
    let ph = want.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!("SELECT id, skill FROM prompts WHERE id IN ({ph}) AND skill IS NOT NULL");
    let mut q = sqlx::query_as::<_, (i64, String)>(&sql);
    for (_, pid) in &want {
        q = q.bind(pid);
    }
    let Ok(found) = q.fetch_all(pool).await else {
        tracing::warn!("废纸篓 skill 归属查询失败，本次不显示 skill 标");
        return out;
    };
    for (trash_id, prompt_id) in want {
        if let Some((_, skill)) = found.iter().find(|(id, _)| *id == prompt_id) {
            if !skill.is_empty() {
                out.insert(trash_id, skill.clone());
            }
        }
    }
    out
}

/// 补齐缩略图像素并写回库里（0031）。补一次，此后不再测。
///
/// 与验收页的 `backfill_sizes` 是同一件事：读的是**缩略图**（比例一样，而它小两个
/// 数量级），且 `image_dimensions` 只读文件头、不解码像素。拿不到就留 None ——
/// 那多半意味着这一项的图已经被上一次清理带走了，前端对 None 用中性比例兜底。
async fn backfill_thumb_sizes(pool: &sqlx::SqlitePool, rows: &mut [TrashItemView]) {
    let todo: Vec<(i64, String)> = rows
        .iter()
        .filter(|r| r.width.is_none() || r.height.is_none())
        .filter_map(|r| {
            r.thumb_path
                .clone()
                .or_else(|| r.image_path.clone())
                .map(|p| (r.id, p))
        })
        .collect();
    if todo.is_empty() {
        return;
    }
    // 纯 IO/CPU，别占着 IPC 的异步执行器（同 v0.14.0 ingest_one 那次的教训）。
    let measured = tokio::task::spawn_blocking(move || {
        todo.into_iter()
            .filter_map(|(id, path)| {
                image::image_dimensions(&path)
                    .ok()
                    .map(|(w, h)| (id, i64::from(w), i64::from(h)))
            })
            .collect::<Vec<_>>()
    })
    .await
    .unwrap_or_default();

    for (id, w, h) in measured {
        if let Some(r) = rows.iter_mut().find(|r| r.id == id) {
            r.width = Some(w);
            r.height = Some(h);
        }
        // 写回失败不影响本次显示，下次再补。
        let _ = repo::set_thumb_size(pool, id, w, h).await;
    }
}

/// 拆分编号 `DZ-0001` → (前缀, 序号)。
fn parse_code(code: &str) -> Option<(String, i64)> {
    let (prefix, num) = code.rsplit_once('-')?;
    let n: i64 = num.trim().parse().ok()?;
    Some((prefix.to_string(), n))
}

/// 剔掉「文件还被活着的 clip 引用」的废纸篓行 —— 那些一条都不许物理删。
///
/// 只对 `entity_type='clip'` 生效：其余四类的文件不会被就地复用。被剔掉的行**留在
/// 废纸篓里**（人还看得见它，也还能自己判断），并留一条 warn 说明为什么没清掉。
async fn filter_live_clip_files(
    pool: &sqlx::SqlitePool,
    rows: Vec<crate::db::repo::trash::TrashItemRow>,
) -> AppResult<Vec<crate::db::repo::trash::TrashItemRow>> {
    let clip_ids: Vec<i64> = rows
        .iter()
        .filter(|r| r.entity_type == "clip")
        .filter_map(|r| r.ref_id)
        .collect();
    if clip_ids.is_empty() {
        return Ok(rows);
    }
    let live = v2v_repo::current_media_paths(pool, &clip_ids).await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        if r.entity_type != "clip" {
            out.push(r);
            continue;
        }
        let mut files: Vec<String> = serde_json::from_str(&r.file_paths_json).unwrap_or_default();
        if let Some(t) = &r.thumb_path {
            files.push(t.clone());
        }
        if files.iter().any(|f| live.contains(f)) {
            tracing::warn!(
                trash_id = r.id,
                clip_id = ?r.ref_id,
                "废纸篓项指向的成片文件仍被活着的 clip 引用（多半是判过不通过又重跑了），跳过物理删除"
            );
            continue;
        }
        out.push(r);
    }
    Ok(out)
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
    // 0) **物理删之前，核对 clip 行现在还认不认这些文件**。
    //
    // 这是第二道闸（第一道是重跑时收回废纸篓行，见 `requeue_v2v_clips`），两道各自
    // 独立成立：视频重跑是就地的，成片路径锚在 clip id 上，于是一条判过「不通过」
    // 的 clip 重跑之后，新片子会落在与旧片子完全相同的路径。若那一行废纸篓记录
    // 因为任何原因还在（撤销、旧版本留下的、手工造的），清空废纸篓就会删掉一条
    // **还活着**的成片。
    //
    // 判据不是 stage 而是**路径**：问的就是「这个文件现在有主人吗」，
    // 而那正是删不删得的唯一依据。
    let rows = filter_live_clip_files(pool, rows).await?;
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
                Some(row) => match work_repo::restore(&state.db, &row).await {
                    // 原任务已经随批次退休了（v0.21.0：提示词是消耗品）。作品照样还原
                    // 得回来——编号与组名在 0027 就冗余进了本行——但要说清楚它跟原任务
                    // 的连线断了，否则「验收记录去哪了」会变成一个没人答得上来的问题。
                    Ok(true) => {
                        failures.push(format!("{label}：已还原，但原任务已退休，验收链接为空"));
                        Ok(true)
                    }
                    Ok(false) => Ok(true),
                    Err(e) => Err(e),
                },
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即失败
mod tests {
    use super::*;
    use crate::db::repo::trash::{NewTrashItem, TrashItemRow};
    use crate::db::test_support::test_pool;

    async fn seed_clip(pool: &sqlx::SqlitePool, stage: &str, video: Option<&str>) -> i64 {
        sqlx::query("INSERT INTO prompt_groups (id,name,prefix,scene,is_temp,created_at) VALUES (1,'g','GG','',0,0)").execute(pool).await.unwrap();
        sqlx::query("INSERT INTO prompts (id,group_id,code,text,status,source,created_at,updated_at) VALUES (1,1,'GG-0001','t','active','library',0,0)").execute(pool).await.unwrap();
        sqlx::query(
            "INSERT INTO accepted_works (id,image_path,thumb_path,prompt_id,prompt_text,group_id,batch_id,accepted_at,prompt_code,group_name)
             VALUES (1,'/img.jpg','/thumb.jpg',1,'原文',1,7,100,'GG-0001','g')",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO v2v_clips (work_id, group_id, group_name, batch_id, stage, source_prompt,
                 variable_part, video_path, poster_path, created_at, updated_at)
             VALUES (1, 1, 'g', 7, ?1, '原文', '', ?2, NULL, 0, 0) RETURNING id",
        )
        .bind(stage)
        .bind(video)
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid()
    }

    async fn trash_clip(pool: &sqlx::SqlitePool, clip_id: i64, files: Vec<String>) -> TrashItemRow {
        let mut tx = pool.begin().await.unwrap();
        let id = repo::insert(
            &mut tx,
            &NewTrashItem {
                entity_type: "clip".into(),
                ref_id: Some(clip_id),
                thumb_path: None,
                prompt_text: None,
                code: Some("GG-0001".into()),
                title: Some("g".into()),
                source_label: "视频验收未通过".into(),
                file_paths: files,
                payload_json: None,
            },
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        repo::take(pool, &[id]).await.unwrap().remove(0)
    }

    /// 第二道闸：一条 clip **现在**还指着的成片文件，一个都不许物理删。
    ///
    /// 视频重跑是就地的（`v2v_clips` 只有一行，路径锚在 clip id 上），所以
    /// 「判不通过 → 重跑 → 新片子落在同一个路径」是常态。那一行陈旧的废纸篓记录若
    /// 还在，清空废纸篓就会删掉一条还活着的成片。
    #[tokio::test]
    async fn purging_never_deletes_a_file_a_live_clip_still_points_at() {
        let (pool, _d) = test_pool().await;
        // 重跑之后：clip 又指着同一个路径了（阶段回到 rev，成片是新的那一条）。
        let clip_id = seed_clip(&pool, "rev", Some("/clips/clip1.mp4")).await;
        let stale = trash_clip(&pool, clip_id, vec!["/clips/clip1.mp4".into()]).await;

        let kept = filter_live_clip_files(&pool, vec![stale]).await.unwrap();
        assert!(
            kept.is_empty(),
            "文件还有主人 → 这条废纸篓记录必须留着，一个字节都不许删"
        );
    }

    /// 批次归属：三类各自从**不同的地方**取，取不到的就是取不到。
    ///
    /// 它是分段规则的一半（另一半是时间间隔）——认错批次会把两次任务并成一段，
    /// 而人来这一页正是想认出「那次任务」。作品尤其容易漏：它的源行已经被真删了，
    /// 批次只剩在还原快照里。
    #[tokio::test]
    async fn batch_ids_come_from_three_different_places() {
        let (pool, _d) = test_pool().await;
        let clip_id = seed_clip(&pool, "rej", None).await; // 顺带建好组 / 提示词 / 作品
        sqlx::query("INSERT INTO batches (id, created_at, output_dir) VALUES (9,0,'/o')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO ref_images (id,name,file_path,thumb_path,width,height,file_size,created_at)
             VALUES (1,'r','/r.jpg','/rt.jpg',1,1,1,0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO tasks (id,batch_id,ref_image_id,prompt_id,prompt_text_snapshot,status,created_at,updated_at)
             VALUES (5,9,1,1,'t','rej',0,0)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let rows = vec![
            row_of("task", Some(5), None),
            row_of("clip", Some(clip_id), None),
            row_of("work", None, Some(r#"{"id":1,"batch_id":42}"#.into())),
            row_of("prompt", Some(1), None),
        ];
        let map = batch_ids_of(&pool, &rows).await;

        assert_eq!(
            map.get(&rows[0].id),
            Some(&9),
            "未通过任务 → tasks.batch_id"
        );
        assert_eq!(map.get(&rows[1].id), Some(&7), "视频 → v2v_clips.batch_id");
        assert_eq!(map.get(&rows[2].id), Some(&42), "作品 → 还原快照里的批次");
        assert_eq!(
            map.get(&rows[3].id),
            None,
            "提示词没有批次概念，不许瞎猜一个"
        );
    }

    /// 源行已经随批次退休了（提示词是消耗品）——那就没有批次，而不是报错或塞个 0。
    #[tokio::test]
    async fn a_retired_source_row_yields_no_batch_rather_than_a_wrong_one() {
        let (pool, _d) = test_pool().await;
        let rows = vec![row_of("task", Some(4242), None)];
        assert!(batch_ids_of(&pool, &rows).await.is_empty());
    }

    /// 造一行内存里的废纸篓记录（`batch_ids_of` 只读这三个字段）。
    /// id 按调用顺序自增，互不相同即可。
    fn row_of(entity: &str, ref_id: Option<i64>, payload: Option<String>) -> TrashItemRow {
        use std::sync::atomic::{AtomicI64, Ordering};
        static NEXT: AtomicI64 = AtomicI64::new(1);
        TrashItemRow {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            entity_type: entity.into(),
            ref_id,
            thumb_path: None,
            prompt_text: None,
            code: None,
            title: None,
            source_label: "x".into(),
            file_paths_json: "[]".into(),
            deleted_at: 0,
            payload_json: payload,
            thumb_w: None,
            thumb_h: None,
        }
    }

    /// 真的没有主人了就照删 —— 闸门只挡住冲突，不挡住正常清理。
    #[tokio::test]
    async fn purging_proceeds_when_the_clip_no_longer_owns_those_files() {
        let (pool, _d) = test_pool().await;
        let clip_id = seed_clip(&pool, "rej", None).await;
        let row = trash_clip(&pool, clip_id, vec!["/clips/clip1.mp4".into()]).await;

        let kept = filter_live_clip_files(&pool, vec![row]).await.unwrap();
        assert_eq!(kept.len(), 1);
    }
}
