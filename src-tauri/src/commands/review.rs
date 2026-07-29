//! review 域命令（执行计划 2.1 / 需求 13 / R7 / R8）。

use std::path::PathBuf;

use serde::Serialize;
use specta::Type;
use sqlx::FromRow;
use tauri::State;

use crate::db::now_unix;
use crate::db::repo::{
    prompts as prompt_repo, tasks as task_repo, trash as trash_repo, works as work_repo,
};
use crate::error::AppResult;
use crate::files;
use crate::state::AppState;

/// 待验收项视图。
#[derive(Debug, Clone, Serialize, Type, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct ReviewItemView {
    pub id: i64,
    pub batch_id: i64,
    pub ref_name: String,
    pub prompt_code: String,
    pub group_name: String,
    pub key_alias: Option<String>,
    pub result_image_path: Option<String>,
    pub result_thumb_path: Option<String>,
    pub prompt_text: String,
    /// 参考图缩略图/原图（E08 大图对比）。
    pub ref_thumb_path: Option<String>,
    pub ref_image_path: Option<String>,
    /// 结果图真实像素（0027）。验收页按真实比例排版，行高在渲染前就要算得出来 ——
    /// 等图片加载完再量，每张图落地都会把它下面的行往下顶一次，滚动时就是持续抖动。
    pub result_width: Option<i64>,
    pub result_height: Option<i64>,
    /// 任务创建时刻。验收页的「时间」档按它切日 / 任务簇。
    ///
    /// 取 `created_at` 而不是 `updated_at`：后者会随重试、随状态迁移一路往后跳，
    /// 于是「这批是什么时候跑的」在同一批里会给出好几个互相矛盾的答案。
    pub created_at: i64,
    /// 写出这条词的 skill（0032）。None = 手工导入 / 历史数据 / 工单没声明。
    pub skill: Option<String>,
}

/// 验收结果。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AcceptResult {
    pub accepted: i64,
    /// 本次因验收通过而转正的临时分组名（前端 toast）
    pub promoted_groups: Vec<String>,
    /// 本次自动进入视频流水线「待改写」的条数（提示词组用途 = 图生视频）。
    ///
    /// 交接从「你要记得回作品库找出来再点导出」变成「它自己就在那了」——
    /// 那个「找出来」的动作本来就不该存在：哪些图是首帧图，在写那份 txt 时就已经决定了。
    pub queued_v2v: i64,
}

const REVIEW_SELECT: &str = "SELECT t.id, t.batch_id, COALESCE(r.name,'') AS ref_name,
        COALESCE(p.code,'') AS prompt_code, COALESCE(g.name,'') AS group_name,
        k.name AS key_alias, t.result_image_path, t.result_thumb_path, t.prompt_text_snapshot AS prompt_text,
        r.thumb_path AS ref_thumb_path, r.file_path AS ref_image_path,
        t.result_width, t.result_height, t.created_at, p.skill
    FROM tasks t
    LEFT JOIN ref_images r ON r.id = t.ref_image_id
    LEFT JOIN prompts p ON p.id = t.prompt_id
    LEFT JOIN prompt_groups g ON g.id = p.group_id
    LEFT JOIN api_keys k ON k.id = t.api_key_id
    WHERE t.status = 'rev'";

#[tauri::command]
#[specta::specta]
pub async fn list_pending_review(
    state: State<'_, AppState>,
    batch_id: Option<i64>,
) -> AppResult<Vec<ReviewItemView>> {
    // 批次倒序：最近一批排在最顶部，往下依次是更早的批次；批次内保持生成序（id 升序），
    // 参考图 × 提示词的对应关系读起来才连贯。
    const ORDER: &str = " ORDER BY t.batch_id DESC, t.id ASC";
    let (sql, bind) = match batch_id {
        Some(_) => (
            format!("{REVIEW_SELECT} AND t.batch_id = ?{ORDER}"),
            batch_id,
        ),
        None => (format!("{REVIEW_SELECT}{ORDER}"), None),
    };
    let mut q = sqlx::query_as::<_, ReviewItemView>(&sql);
    if let Some(b) = bind {
        q = q.bind(b);
    }
    let mut rows = q.fetch_all(&state.db).await?;
    backfill_sizes(&state.db, &mut rows).await;
    Ok(rows)
}

/// 补齐 0027 之前生成的结果图像素，并写回库里（补一次，此后不再算）。
///
/// 读的是**缩略图**而不是原图：比例一样，而缩略图小两个数量级；且
/// `image_dimensions` 只读文件头，不解码像素。拿不到就留 None ——
/// 前端对 None 用一个中性比例兜底，绝不为了排版去猜一个假尺寸。
async fn backfill_sizes(pool: &sqlx::SqlitePool, rows: &mut [ReviewItemView]) {
    let todo: Vec<(i64, String)> = rows
        .iter()
        .filter(|r| r.result_width.is_none() || r.result_height.is_none())
        .filter_map(|r| {
            r.result_thumb_path
                .clone()
                .or_else(|| r.result_image_path.clone())
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
                    .map(|(w, h)| (id, w as i64, h as i64))
            })
            .collect::<Vec<_>>()
    })
    .await
    .unwrap_or_default();

    for (id, w, h) in measured {
        if let Some(r) = rows.iter_mut().find(|r| r.id == id) {
            r.result_width = Some(w);
            r.result_height = Some(h);
        }
        // 写回失败不影响本次显示，下次再补。
        let _ = task_repo::set_result_size(pool, id, w, h).await;
    }
}

/// 验收通过内部承载。
#[derive(FromRow)]
struct AcceptRow {
    id: i64,
    batch_id: i64,
    ref_image_id: i64,
    prompt_id: i64,
    prompt_text_snapshot: String,
    draw_index: i64,
    result_image_path: Option<String>,
    result_thumb_path: Option<String>,
    ref_name: String,
    prompt_code: String,
    prompt_title: Option<String>,
    prompt_text: String,
    group_id: Option<i64>,
    is_temp: i64,
    group_name: String,
}

const ACCEPT_SELECT: &str = "SELECT t.id, t.batch_id, t.ref_image_id, t.prompt_id,
        t.prompt_text_snapshot, t.draw_index, t.result_image_path, t.result_thumb_path,
        COALESCE(r.name,'') AS ref_name, COALESCE(p.code,'') AS prompt_code,
        p.title AS prompt_title,
        COALESCE(p.text,'') AS prompt_text, p.group_id,
        COALESCE(g.is_temp,0) AS is_temp, COALESCE(g.name,'') AS group_name
    FROM tasks t
    LEFT JOIN ref_images r ON r.id = t.ref_image_id
    LEFT JOIN prompts p ON p.id = t.prompt_id
    LEFT JOIN prompt_groups g ON g.id = p.group_id
    WHERE t.status = 'rev' AND t.id IN ";

/// 一次取回本批全部待验收行（一条 IN 查询，不是 N 条 SELECT）。
async fn accept_rows(pool: &sqlx::SqlitePool, ids: &[i64]) -> AppResult<Vec<AcceptRow>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let holes = vec!["?"; ids.len()].join(",");
    let sql = format!("{ACCEPT_SELECT}({holes})");
    let mut q = sqlx::query_as::<_, AcceptRow>(&sql);
    for id in ids {
        q = q.bind(*id);
    }
    let mut rows = q.fetch_all(pool).await?;
    // 按调用方给的顺序还原 —— 验收是人一张张点出来的序列，输出与提示都跟着它走。
    rows.sort_by_key(|r| ids.iter().position(|i| *i == r.id).unwrap_or(usize::MAX));
    Ok(rows)
}

/// 通过所选：输出原图 + 写作品快照 + 微调写回(R8) + 临时组转正(R7)。
///
/// ## 拷贝先做完，再动库
///
/// 输出拷贝是**阻塞 IO**，一张几 MB，一次验收几十上百张 —— 留在异步执行器上会把整个
/// IPC 卡住（同 v0.14.0 参考图导入那次的教训：界面十几秒一声不吭，人以为没点上）。
/// 故整批拷贝进一个 `spawn_blocking`，一次线程往返而不是每张一次。
///
/// **不把整批塞进一个事务**（这一点与「整批改单事务」的直觉相反，理由是文件）：
/// 拷贝无法回滚。真做成一个事务，第 150 张失败就会把前 149 条作品记录回滚掉，
/// 而它们的输出文件已经躺在 outputs/ 里了 —— 变成一堆没有主人的图。现在的顺序
/// （先整批拷完，任一张失败就整个报错、一行库都不写）反而更接近原子：
/// 要么全部拷成功再记账，要么什么账都没记。
#[tauri::command]
#[specta::specta]
pub async fn accept_tasks(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    task_ids: Vec<i64>,
) -> AppResult<AcceptResult> {
    let mut promoted: Vec<String> = Vec::new();
    let mut accepted = 0i64;
    let mut batches: Vec<i64> = Vec::new();
    // 用途 = 图生视频的组，其通过图自动入视频流水线。用途缓存按组算一次：
    // 一个批次里同组常有十几张，逐张查标签是白跑十几次 SQL。
    let mut i2v_groups: Vec<(i64, bool)> = Vec::new();
    let mut to_queue: Vec<i64> = Vec::new();
    let date = files::date_yymmdd(now_unix());

    let rows = accept_rows(&state.db, &task_ids).await?;
    // 算出每张的落点（纯字符串活，留在这边），再一次性交给阻塞线程去拷。
    let mut jobs: Vec<(i64, String, PathBuf)> = Vec::new();
    for row in &rows {
        let Some(src) = row.result_image_path.clone() else {
            continue; // 无结果图，跳过
        };
        // 任务6：输出到 outputs/{批次}/{分组}/参考图名_YYMMDD_编号.EXT。
        // 按提示词分组分文件夹存放，多分组批次天然各归各处而非全混一处。
        // 分组名做文件系统安全清洗，空分组归入「未分组」。
        let group_folder = if row.group_name.trim().is_empty() {
            "未分组".to_string()
        } else {
            files::sanitize_filename(&row.group_name)
        };
        let out_dir = state
            .dirs
            .outputs()
            .join(row.batch_id.to_string())
            .join(&group_folder);
        // 任务1：输出扩展名跟随源结果格式（默认 jpg；用户保留原格式时可能 png）。
        let ext = files::output_ext_from_path(&src);
        let filename =
            files::output_filename(&row.ref_name, &row.prompt_code, &date, row.draw_index, &ext);
        jobs.push((row.id, src, out_dir.join(&filename)));
    }
    if jobs.is_empty() {
        return Ok(AcceptResult {
            accepted: 0,
            promoted_groups: Vec::new(),
            queued_v2v: 0,
        });
    }
    let to_copy: Vec<(i64, String, PathBuf)> = jobs.clone();
    // 拷贝失败必须上报：否则会记录 pass + works 指向不存在的输出文件（磁盘满/源丢失）。
    let copied: std::io::Result<Vec<i64>> = tokio::task::spawn_blocking(move || {
        let mut ok = Vec::with_capacity(to_copy.len());
        for (id, src, dest) in to_copy {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&src, &dest)?;
            ok.push(id);
        }
        Ok(ok)
    })
    .await
    .map_err(|e| crate::error::AppError::Io(format!("输出拷贝任务失败：{e}")))?;
    copied?;

    for row in rows {
        let Some((_, src, out_path)) = jobs.iter().find(|(id, _, _)| *id == row.id).cloned() else {
            continue;
        };
        let thumb = row.result_thumb_path.clone().unwrap_or_else(|| src.clone());

        // 事务：写作品 + 微调写回 + 临时组转正 + 状态迁移。
        let mut tx = state.db.begin().await?;
        let work_id = work_repo::insert(
            &mut tx,
            &work_repo::NewWork {
                task_id: row.id,
                image_path: out_path.to_string_lossy().to_string(),
                thumb_path: thumb,
                prompt_id: row.prompt_id,
                prompt_text: row.prompt_text_snapshot.clone(),
                group_id: row.group_id,
                ref_image_id: row.ref_image_id,
                batch_id: row.batch_id,
                // 编号与组名当场存成快照（0027）：提示词是消耗品，批次跑完就随批次删掉，
                // 现读 JOIN 的话作品会在那一刻丢掉自己的身份。
                prompt_code: row.prompt_code.clone(),
                group_name: row.group_name.clone(),
            },
        )
        .await?;
        tx.commit().await?;

        // R8：微调过则写回提示词库。
        if row.prompt_text_snapshot != row.prompt_text {
            prompt_repo::apply_edit(&state.db, row.prompt_id, &row.prompt_text_snapshot).await?;
        }
        // R7：临时组首次通过 → 转正式。
        if row.is_temp != 0 {
            if let Some(gid) = row.group_id {
                if prompt_repo::promote_group(&state.db, gid).await? {
                    promoted.push(row.group_name.clone());
                }
            }
        }
        // 用途 = 图生视频 → 这张通过图自动进「待改写」。
        // 用途标在**提示词组**上：一张图的用途由它的提示词决定，提示词的用途由那份 txt 决定。
        if let Some(gid) = row.group_id {
            let is_i2v = match i2v_groups.iter().find(|(g, _)| *g == gid) {
                Some((_, v)) => *v,
                None => {
                    let tags = prompt_repo::group_tags(&state.db, gid).await?;
                    let v = tags.iter().any(|t| t == crate::purpose::PURPOSE_I2V);
                    i2v_groups.push((gid, v));
                    v
                }
            };
            if is_i2v {
                to_queue.push(work_id);
            }
        }

        task_repo::set_status(&state.db, row.id, "pass").await?;
        accepted += 1;
        if !batches.contains(&row.batch_id) {
            batches.push(row.batch_id);
        }
    }

    // 入队 + 物化工单。**放在验收循环之后**：一批验收只重写一次工单，而不是每张一次。
    let queued_v2v = crate::commands::v2v::enqueue_works(&state.db, &to_queue).await?;
    if queued_v2v > 0 {
        crate::commands::v2v::refresh_handoff(&state.db, &app).await;
    }

    for b in &batches {
        let _ = task_repo::archive_if_all_terminal(&state.db, *b).await;
        // 验收改变了任务态：补发批次汇总，驱动侧栏「待验收」徽章即时更新。
        state.engine.emit_summary(*b).await;
    }
    // 汇总发完之后再退休：先删批次会让上面那句 emit 对着一个已经不存在的批次算数。
    crate::commands::batches::retire_batches_quietly(&state.db).await;
    promoted.dedup();
    Ok(AcceptResult {
        accepted,
        promoted_groups: promoted,
        queued_v2v,
    })
}

/// 不通过任务待清理时应物理删除的文件列表（E02）：原图 + 缩略图。
/// reject 时**不**立即物理删除原图——原图随此列表暂存至废纸篓，「彻底删除/清空」才删。
fn rejected_file_paths(image: &Option<String>, thumb: &Option<String>) -> Vec<String> {
    let mut paths = Vec::new();
    if let Some(img) = image {
        paths.push(img.clone());
    }
    if let Some(t) = thumb {
        paths.push(t.clone());
    }
    paths
}

/// 不通过所选：置 rej + 原图暂存进废纸篓（E02，不立即物理删除）+ 留缩略图，单事务。
#[tauri::command]
#[specta::specta]
pub async fn reject_tasks(state: State<'_, AppState>, task_ids: Vec<i64>) -> AppResult<i64> {
    let mut rejected = 0i64;
    let mut batches: Vec<i64> = Vec::new();

    // 一条 IN 查询取回整批，而不是每条一次 SELECT。
    for row in accept_rows(&state.db, &task_ids).await? {
        // E02：原图不再立即物理删除，随缩略图一并暂存进废纸篓 file_paths，
        // 由「彻底删除/清空废纸篓」时的 purge 统一物理删除。误触「不通过」不再丢原图。
        let file_paths = rejected_file_paths(&row.result_image_path, &row.result_thumb_path);
        let mut tx = state.db.begin().await?;
        trash_repo::insert(
            &mut tx,
            &trash_repo::NewTrashItem {
                entity_type: "task".into(),
                ref_id: Some(row.id),
                thumb_path: row.result_thumb_path.clone(),
                prompt_text: Some(row.prompt_text_snapshot.clone()),
                code: Some(row.prompt_code.clone()),
                title: row.prompt_title.clone(),
                source_label: "验收未通过".into(),
                file_paths,
                payload_json: None,
            },
        )
        .await?;
        tx.commit().await?;

        task_repo::set_status(&state.db, row.id, "rej").await?;
        rejected += 1;
        if !batches.contains(&row.batch_id) {
            batches.push(row.batch_id);
        }
    }

    for b in &batches {
        let _ = task_repo::archive_if_all_terminal(&state.db, *b).await;
        // 验收改变了任务态：补发批次汇总，驱动侧栏「待验收」徽章即时更新。
        state.engine.emit_summary(*b).await;
    }
    // 这里通常**不会**真的退休任何批次：刚判的这几条正躺在废纸篓里等着「误删可还原」，
    // 而那正是退休条件里的第二条。等废纸篓清干净了才轮到它。
    crate::commands::batches::retire_batches_quietly(&state.db).await;
    Ok(rejected)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即失败
mod tests {
    use super::*;
    use crate::db::test_support::test_pool;
    use sqlx::SqlitePool;

    async fn seed_task(pool: &SqlitePool, status: &str) {
        sqlx::query("INSERT INTO prompt_groups (id,name,prefix,scene,is_temp,created_at) VALUES (1,'g','GG','',0,0)").execute(pool).await.unwrap();
        sqlx::query("INSERT INTO prompts (id,group_id,code,text,status,source,created_at,updated_at) VALUES (1,1,'GG-0001','t','active','library',0,0)").execute(pool).await.unwrap();
        sqlx::query("INSERT INTO ref_images (id,name,file_path,thumb_path,width,height,file_size,created_at) VALUES (1,'r','/f','/t',1,1,1,0)").execute(pool).await.unwrap();
        sqlx::query("INSERT INTO batches (id,created_at,output_dir,params_json,status) VALUES (1,0,'/out','{}','running')").execute(pool).await.unwrap();
        sqlx::query("INSERT INTO tasks (id,batch_id,ref_image_id,prompt_id,prompt_text_snapshot,status,result_image_path,result_thumb_path,created_at,updated_at) VALUES (1,1,1,1,'t',?1,'/img.jpg','/thumb.jpg',0,0)")
            .bind(status).execute(pool).await.unwrap();
    }

    // 幂等守卫：验收命令只对 rev（待验收）任务生效。长按 ⏎ / 连点导致的重复提交，
    // 第二次查询已是 pass/rej，ACCEPT_SELECT 返回 None → 不会重复输出/记账。
    #[tokio::test]
    async fn accept_select_only_matches_rev_tasks() {
        let (pool, _d) = test_pool().await;
        seed_task(&pool, "pass").await;
        let rows = accept_rows(&pool, &[1]).await.unwrap();
        assert!(rows.is_empty(), "已通过任务不应被验收命令再次选中");

        sqlx::query("UPDATE tasks SET status='rev' WHERE id=1")
            .execute(&pool)
            .await
            .unwrap();
        let rows = accept_rows(&pool, &[1]).await.unwrap();
        assert_eq!(rows.len(), 1, "rev 待验收任务应被选中");
    }

    // 验收页排序：最近一批在最顶部（批次倒序），批次内保持生成序（id 升序）。
    #[tokio::test]
    async fn pending_review_lists_newest_batch_first() {
        let (pool, _d) = test_pool().await;
        seed_task(&pool, "rev").await;
        // 第二个批次（id 更大 = 更近），内含两个任务。
        sqlx::query("INSERT INTO batches (id,created_at,output_dir,params_json,status) VALUES (2,0,'/out','{}','running')").execute(&pool).await.unwrap();
        for tid in [2, 3] {
            sqlx::query("INSERT INTO tasks (id,batch_id,ref_image_id,prompt_id,prompt_text_snapshot,status,result_image_path,result_thumb_path,created_at,updated_at) VALUES (?1,2,1,1,'t','rev','/img.jpg','/thumb.jpg',0,0)")
                .bind(tid).execute(&pool).await.unwrap();
        }

        let sql = format!("{REVIEW_SELECT} ORDER BY t.batch_id DESC, t.id ASC");
        let rows = sqlx::query_as::<_, ReviewItemView>(&sql)
            .fetch_all(&pool)
            .await
            .unwrap();
        let order: Vec<(i64, i64)> = rows.iter().map(|r| (r.batch_id, r.id)).collect();
        assert_eq!(
            order,
            vec![(2, 2), (2, 3), (1, 1)],
            "最近批次(2)整体排在更早批次(1)之前，批次内按 id 升序"
        );
    }

    // E02：不通过时原图必须进入待清理文件列表（不立即物理删除），否则误触即永久丢原图。
    #[test]
    fn rejected_file_paths_retains_original_image() {
        let paths = rejected_file_paths(
            &Some("/data/results/img.jpg".into()),
            &Some("/data/thumbs/img.jpg".into()),
        );
        assert!(
            paths.contains(&"/data/results/img.jpg".to_string()),
            "原图须列入废纸篓待清理文件，供 purge 时才物理删除"
        );
        assert!(
            paths.contains(&"/data/thumbs/img.jpg".to_string()),
            "缩略图一并列入待清理文件"
        );
    }

    // 极端：无缩略图仍须保住原图。
    #[test]
    fn rejected_file_paths_handles_missing_thumb() {
        let paths = rejected_file_paths(&Some("/data/results/img.jpg".into()), &None);
        assert_eq!(paths, vec!["/data/results/img.jpg".to_string()]);
    }
}
