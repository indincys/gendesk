//! 批次 / 任务 / 执行记录数据仓（引擎与命令共用）。

// 部分查询由 M2 引擎与 M3 页面分别消费；先落地。
#![allow(dead_code)]

use sqlx::{FromRow, SqliteConnection, SqlitePool};

use crate::db::now_unix;
use crate::engine::events::SummaryCounts;

#[derive(Debug, Clone, FromRow)]
pub struct BatchRow {
    pub id: i64,
    pub created_at: i64,
    pub output_dir: String,
    pub params_json: String,
    pub status: String,
    /// 批次备注名（E10）；None = 未命名。
    pub note: Option<String>,
}

#[derive(Debug, Clone, FromRow)]
pub struct TaskRow {
    pub id: i64,
    pub batch_id: i64,
    pub ref_image_id: i64,
    pub prompt_id: i64,
    pub prompt_text_snapshot: String,
    pub status: String,
    pub api_key_id: Option<i64>,
    pub error_type: Option<String>,
    pub error_message: Option<String>,
    pub retry_count: i64,
    pub result_image_path: Option<String>,
    pub result_thumb_path: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

// ---------------- 批次 ----------------

pub async fn create_batch(
    conn: &mut SqliteConnection,
    output_dir: &str,
    params_json: &str,
) -> Result<i64, sqlx::Error> {
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO batches (created_at, output_dir, params_json, status)
         VALUES (?1, ?2, ?3, 'running') RETURNING id",
    )
    .bind(now_unix())
    .bind(output_dir)
    .bind(params_json)
    .fetch_one(&mut *conn)
    .await?;
    Ok(id)
}

pub async fn add_batch_ref(
    conn: &mut SqliteConnection,
    batch_id: i64,
    ref_image_id: i64,
    prompt_group_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO batch_refs (batch_id, ref_image_id, prompt_group_id) VALUES (?1, ?2, ?3)",
    )
    .bind(batch_id)
    .bind(ref_image_id)
    .bind(prompt_group_id)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

pub async fn list_batches(pool: &SqlitePool) -> Result<Vec<BatchRow>, sqlx::Error> {
    sqlx::query_as::<_, BatchRow>("SELECT * FROM batches ORDER BY created_at DESC, id DESC")
        .fetch_all(pool)
        .await
}

pub async fn get_batch(pool: &SqlitePool, id: i64) -> Result<Option<BatchRow>, sqlx::Error> {
    sqlx::query_as::<_, BatchRow>("SELECT * FROM batches WHERE id = ?1")
        .bind(id)
        .fetch_optional(pool)
        .await
}

/// 批次备注名（E10）：空串归一为 NULL（清除备注）。
pub async fn rename_batch(pool: &SqlitePool, id: i64, note: &str) -> Result<(), sqlx::Error> {
    let trimmed = note.trim();
    let value: Option<&str> = if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    };
    sqlx::query("UPDATE batches SET note = ?2 WHERE id = ?1")
        .bind(id)
        .bind(value)
        .execute(pool)
        .await?;
    Ok(())
}

/// 删除归档时刻早于 cutoff 的批次（E22 / D3）。tasks/task_attempts/batch_refs 经外键
/// ON DELETE CASCADE 一并删除；accepted_works.task_id 为 ON DELETE SET NULL 故作品保留
/// （D3「作品不动」）。返回删除批次数。
pub async fn delete_batches_archived_before(
    pool: &SqlitePool,
    cutoff: i64,
) -> Result<u64, sqlx::Error> {
    let res = sqlx::query(
        "DELETE FROM batches WHERE status = 'archived' AND archived_at IS NOT NULL AND archived_at < ?1",
    )
    .bind(cutoff)
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

/// 批次首张有缩略图的任务缩略图路径（E10 批次切换器预览）。
pub async fn batch_first_thumb(
    pool: &SqlitePool,
    batch_id: i64,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar::<_, String>(
        "SELECT result_thumb_path FROM tasks
         WHERE batch_id = ?1 AND result_thumb_path IS NOT NULL ORDER BY id ASC LIMIT 1",
    )
    .bind(batch_id)
    .fetch_optional(pool)
    .await
}

pub async fn set_batch_status(pool: &SqlitePool, id: i64, status: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE batches SET status = ?2 WHERE id = ?1")
        .bind(id)
        .bind(status)
        .execute(pool)
        .await?;
    Ok(())
}

/// 若批次内所有任务均为终态（pass/rej/fail）则归档，返回是否归档。
pub async fn archive_if_all_terminal(
    pool: &SqlitePool,
    batch_id: i64,
) -> Result<bool, sqlx::Error> {
    let pending: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM tasks WHERE batch_id = ?1 AND status NOT IN ('pass','rej','fail')",
    )
    .bind(batch_id)
    .fetch_one(pool)
    .await?;
    if pending == 0 {
        // 仅在真正发生「→archived」迁移时返回 true：多个并发 worker 同时收尾时避免
        // 重复触发批次完成通知（E04）。
        let res = sqlx::query(
            "UPDATE batches SET status = 'archived', archived_at = ?2 WHERE id = ?1 AND status != 'archived'",
        )
        .bind(batch_id)
        .bind(now_unix())
        .execute(pool)
        .await?;
        Ok(res.rows_affected() > 0)
    } else {
        Ok(false)
    }
}

// ---------------- 任务 ----------------

pub async fn insert_task(
    conn: &mut SqliteConnection,
    batch_id: i64,
    ref_image_id: i64,
    prompt_id: i64,
    snapshot: &str,
    draw_index: i64,
) -> Result<i64, sqlx::Error> {
    let now = now_unix();
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO tasks (batch_id, ref_image_id, prompt_id, prompt_text_snapshot, draw_index, status, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 'q', ?6, ?6) RETURNING id",
    )
    .bind(batch_id)
    .bind(ref_image_id)
    .bind(prompt_id)
    .bind(snapshot)
    .bind(draw_index)
    .bind(now)
    .fetch_one(&mut *conn)
    .await?;
    Ok(id)
}

pub async fn get_task(pool: &SqlitePool, id: i64) -> Result<Option<TaskRow>, sqlx::Error> {
    sqlx::query_as::<_, TaskRow>("SELECT * FROM tasks WHERE id = ?1")
        .bind(id)
        .fetch_optional(pool)
        .await
}

/// 取待派发的 'q' 任务（FIFO）。
pub async fn fetch_queued(pool: &SqlitePool, limit: i64) -> Result<Vec<TaskRow>, sqlx::Error> {
    sqlx::query_as::<_, TaskRow>("SELECT * FROM tasks WHERE status = 'q' ORDER BY id ASC LIMIT ?1")
        .bind(limit)
        .fetch_all(pool)
        .await
}

/// 通用状态置换（仅写 status + updated_at）。合法性由调度器状态机守卫。
pub async fn set_status(pool: &SqlitePool, id: i64, status: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE tasks SET status = ?2, updated_at = ?3 WHERE id = ?1")
        .bind(id)
        .bind(status)
        .bind(now_unix())
        .execute(pool)
        .await?;
    Ok(())
}

/// 认领一个任务：`from` → `run`。**返回是否认领成功**。
///
/// `AND status = ?4` 不是多余的防御，它是这条路径唯一的并发闸门：调度器先
/// `fetch_queued` 读一批，再逐个派发，两步之间是异步的 —— 用户在这段窗口里点
/// 「批量中止」把行删了或改成别的态，无谓词的 UPDATE 会把一个已经不该跑的任务
/// 重新写成 `run`，然后 spawn 一个 worker **发一次付费请求**，而结果无处可写。
///
/// 谓词用调用方刚读到的那个状态原文（`q` 或 `retry`），而不是写死 `'q'`：
/// 冷却结束的重试任务也走这条路（`Retry → Run` 是合法迁移），写死会让它们永不派发。
pub async fn mark_running(
    pool: &SqlitePool,
    id: i64,
    api_key_id: i64,
    from: &str,
) -> Result<bool, sqlx::Error> {
    let res = sqlx::query(
        "UPDATE tasks SET status = 'run', api_key_id = ?2, error_type = NULL, error_message = NULL, updated_at = ?3 WHERE id = ?1 AND status = ?4",
    )
    .bind(id)
    .bind(api_key_id)
    .bind(now_unix())
    .bind(from)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// 落盘完成 → 待验收。`size` 是结果图的真实像素（0027）：验收页按真实比例排版，
/// 而比例必须在渲染**之前**就知道，否则每张图加载完都会把下面的行往下顶一次。
/// 拿不到（解码失败）就存 NULL，由验收页读缩略图文件头补齐。
pub async fn mark_review(
    pool: &SqlitePool,
    id: i64,
    result_image_path: &str,
    result_thumb_path: &str,
    size: Option<(u32, u32)>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE tasks SET status = 'rev', result_image_path = ?2, result_thumb_path = ?3,
            result_width = ?5, result_height = ?6,
            error_type = NULL, error_message = NULL, updated_at = ?4 WHERE id = ?1",
    )
    .bind(id)
    .bind(result_image_path)
    .bind(result_thumb_path)
    .bind(now_unix())
    .bind(size.map(|(w, _)| w as i64))
    .bind(size.map(|(_, h)| h as i64))
    .execute(pool)
    .await?;
    Ok(())
}

/// 补写结果像素（0027 之前生成的历史任务）。只在读到过一次之后写回，之后不再重算。
pub async fn set_result_size(
    pool: &SqlitePool,
    id: i64,
    w: i64,
    h: i64,
) -> Result<(), sqlx::Error> {
    // 不动 updated_at：这是补一条早就该有的元数据，不是任务本身发生了什么。
    sqlx::query("UPDATE tasks SET result_width = ?2, result_height = ?3 WHERE id = ?1")
        .bind(id)
        .bind(w)
        .bind(h)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn mark_retry(
    pool: &SqlitePool,
    id: i64,
    retry_count: i64,
    error_type: &str,
    error_message: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE tasks SET status = 'retry', retry_count = ?2, error_type = ?3, error_message = ?4, updated_at = ?5 WHERE id = ?1",
    )
    .bind(id)
    .bind(retry_count)
    .bind(error_type)
    .bind(error_message)
    .bind(now_unix())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn mark_fail(
    pool: &SqlitePool,
    id: i64,
    error_type: &str,
    error_message: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE tasks SET status = 'fail', error_type = ?2, error_message = ?3, updated_at = ?4 WHERE id = ?1",
    )
    .bind(id)
    .bind(error_type)
    .bind(error_message)
    .bind(now_unix())
    .execute(pool)
    .await?;
    Ok(())
}

/// 恢复一个无输出的失败任务（fail → q），清错误并重置本轮自动恢复预算。
///
/// `rev/pass/rej` 明确不接受：它们已经有输出或已完成验收，不能借这个命令重新生成。
pub async fn recover(
    pool: &SqlitePool,
    id: i64,
    edited_prompt: Option<&str>,
) -> Result<bool, sqlx::Error> {
    let res = sqlx::query(
        "UPDATE tasks SET status = 'q', error_type = NULL, error_message = NULL, retry_count = 0,
             updated_at = ?2
             , prompt_text_snapshot = COALESCE(?3, prompt_text_snapshot)
         WHERE id = ?1 AND status = 'fail'",
    )
    .bind(id)
    .bind(now_unix())
    .bind(edited_prompt)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// 删除单个任务（「不需要了」）。生成中/重试中的任务拒绝删除（返回 None），避免与在途
/// worker 竞争；成功则返回其所属批次 id（供调用方重估归档 + 补发汇总）。
/// task_attempts 由外键 ON DELETE CASCADE 一并清除。
pub async fn delete_task(pool: &SqlitePool, id: i64) -> Result<Option<i64>, sqlx::Error> {
    let batch_id: Option<i64> = sqlx::query_scalar(
        "SELECT batch_id FROM tasks WHERE id = ?1 AND status NOT IN ('run','retry')",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    if let Some(bid) = batch_id {
        sqlx::query("DELETE FROM tasks WHERE id = ?1")
            .bind(id)
            .execute(pool)
            .await?;
        Ok(Some(bid))
    } else {
        Ok(None)
    }
}

/// 删除某批次全部失败任务。返回删除行数。
pub async fn delete_failed(pool: &SqlitePool, batch_id: i64) -> Result<i64, sqlx::Error> {
    let n = sqlx::query("DELETE FROM tasks WHERE batch_id = ?1 AND status = 'fail'")
        .bind(batch_id)
        .execute(pool)
        .await?
        .rows_affected();
    Ok(n as i64)
}

/// 取消批次剩余排队任务（E03）：删除该批次全部 'q' 态任务。只删排队态，
/// 在途（run/retry）与终态任务不受影响。返回删除数。
pub async fn cancel_pending(pool: &SqlitePool, batch_id: i64) -> Result<i64, sqlx::Error> {
    let n = sqlx::query("DELETE FROM tasks WHERE batch_id = ?1 AND status = 'q'")
        .bind(batch_id)
        .execute(pool)
        .await?
        .rows_affected();
    Ok(n as i64)
}

/// 按 id 批量删除处于指定状态的任务；返回 (删除数, 涉及的批次 id)。
///
/// 「中止」传 `["q"]`（只掐掉还没开跑的），「删除」传除 run/retry 外的全部状态。
/// **run/retry 一律不动**：与在途 worker 抢同一行会让它把结果写进一条已经不存在的任务，
/// 而那份图就此谁也找不到（`delete_task` 出于同样理由拒绝在途任务）。
///
/// 一条 SQL 而不是前端 for 循环发 N 次 IPC：选中 200 个任务时那是 200 次往返，
/// 中途任何一次失败都会留下一个说不清删到哪儿的中间态。
pub async fn delete_tasks_where(
    pool: &SqlitePool,
    ids: &[i64],
    statuses: &[&str],
) -> Result<(i64, Vec<i64>), sqlx::Error> {
    if ids.is_empty() || statuses.is_empty() {
        return Ok((0, Vec::new()));
    }
    let id_ph = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let st_ph = statuses.iter().map(|_| "?").collect::<Vec<_>>().join(",");

    // 先取受影响的批次：删完就查不到了，而调用方要靠它重估归档 + 补发汇总。
    let sql = format!(
        "SELECT DISTINCT batch_id FROM tasks WHERE id IN ({id_ph}) AND status IN ({st_ph})"
    );
    let mut q = sqlx::query_scalar::<_, i64>(&sql);
    for id in ids {
        q = q.bind(id);
    }
    for s in statuses {
        q = q.bind(*s);
    }
    let batches = q.fetch_all(pool).await?;

    let sql = format!("DELETE FROM tasks WHERE id IN ({id_ph}) AND status IN ({st_ph})");
    let mut q = sqlx::query(&sql);
    for id in ids {
        q = q.bind(id);
    }
    for s in statuses {
        q = q.bind(*s);
    }
    let n = q.execute(pool).await?.rows_affected() as i64;
    Ok((n, batches))
}

/// 按 id 批量恢复失败任务。违规任务必须逐条改词，批量入口不接受；返回数据库真正更新的 id。
pub async fn recover_many(pool: &SqlitePool, ids: &[i64]) -> Result<Vec<i64>, sqlx::Error> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let ph = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!(
        "UPDATE tasks
            SET status='q', error_type=NULL, error_message=NULL, retry_count=0, updated_at=?
          WHERE id IN ({ph}) AND status='fail'
            AND (error_type IS NULL OR error_type <> 'ContentPolicy')
          RETURNING id"
    );
    let mut q = sqlx::query_scalar::<_, i64>(&sql).bind(now_unix());
    for id in ids {
        q = q.bind(id);
    }
    q.fetch_all(pool).await
}

/// 一次退休扫描的账。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RetireReport {
    pub batches: i64,
    pub prompts: i64,
    pub groups: i64,
}

impl RetireReport {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// 已了结的批次退出历史，连同它消耗掉的提示词一起删（0027）。
///
/// 「提示词是消耗品」这条决定的执行者。一个批次满足两个条件才算了结：
///
/// 1. **没有任何任务还悬着** —— 全部落在 pass/rej（或者任务都被删光了）。
///    `fail` 不算了结：失败的任务还等着人决定重试还是删掉。
/// 2. **没有本批的未通过结果还躺在废纸篓里** —— 那些行是「误删可还原」的锚点，
///    批次一删任务就没了，还原按钮会指向一个不存在的任务。所以清空废纸篓
///    也是触发退休的时机之一。
///
/// 删批次会级联带走 tasks / task_attempts / batch_refs（外键 CASCADE），
/// `accepted_works.task_id` 是 ON DELETE SET NULL，故**作品一张都不掉**。
///
/// **编号不回收**。废纸篓清理会把编号还进号池（那是「这条从来没成过」的语义），
/// 而这里恰恰相反：编号已经印在输出文件名与作品行上，是花掉的。号池按前缀存 next_seq，
/// 分组删掉也不影响它——同名 txt 再导入一次，前缀一样、编号从上次的下一个继续。
/// 废纸篓里那些被删掉的作品，各自属于哪个批次。
///
/// 载荷解析不动就跳过：它只是让这一批**晚一点**退休，而解析失败不该把整轮扫描弄挂。
async fn batches_held_by_trashed_works(
    pool: &SqlitePool,
) -> Result<std::collections::HashSet<i64>, sqlx::Error> {
    let rows: Vec<(Option<String>,)> =
        sqlx::query_as("SELECT payload_json FROM trash_items WHERE entity_type = 'work'")
            .fetch_all(pool)
            .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(j,)| j)
        .filter_map(|j| serde_json::from_str::<crate::db::repo::works::AcceptedWorkRow>(&j).ok())
        .filter_map(|w| w.batch_id)
        .collect())
}

pub async fn retire_resolved_batches(pool: &SqlitePool) -> Result<RetireReport, sqlx::Error> {
    let ids: Vec<i64> = sqlx::query_scalar(
        "SELECT b.id FROM batches b
         WHERE NOT EXISTS (
                 SELECT 1 FROM tasks t
                 WHERE t.batch_id = b.id AND t.status NOT IN ('pass','rej'))
           AND NOT EXISTS (
                 SELECT 1 FROM trash_items ti JOIN tasks t ON t.id = ti.ref_id
                 WHERE ti.entity_type = 'task' AND t.batch_id = b.id)",
    )
    .fetch_all(pool)
    .await?;

    // 废纸篓里**被删掉的作品**同样是还原锚点，而它们不在上面那条 SQL 的视野里：
    // 作品是唯一「删除即真删行」的实体，靠 0027 的 `payload_json` 整行写回，
    // 于是 `trash_items` 与 `accepted_works` 之间没有任何可 JOIN 的东西 ——
    // 归属只写在那份 JSON 里。批次一退休，本批的任务跟着级联删掉，
    // 那份载荷里的 `task_id` 就成了指向不存在行的外键。
    //
    // 在 Rust 里解析而不是用 `json_extract`：本仓库其余地方一处都没依赖 JSON1，
    // 为一句条件引入一个「打包时可能没编进去」的扩展不划算。
    let held = batches_held_by_trashed_works(pool).await?;

    let mut report = RetireReport::default();
    for bid in ids {
        if held.contains(&bid) {
            continue;
        }
        // 本批消耗掉的分组：以任务实际用到的提示词为准（batch_refs 只是挂靠意图，
        // 而任务才是真的跑过什么）。必须在删批次之前取——删完就查不到了。
        let groups: Vec<i64> = sqlx::query_scalar(
            "SELECT DISTINCT p.group_id FROM tasks t
             JOIN prompts p ON p.id = t.prompt_id WHERE t.batch_id = ?1",
        )
        .bind(bid)
        .fetch_all(pool)
        .await?;

        let mut tx = pool.begin().await?;
        sqlx::query("DELETE FROM batches WHERE id = ?1")
            .bind(bid)
            .execute(&mut *tx)
            .await?;
        report.batches += 1;

        for gid in groups {
            // 只删「再没有任何任务引用、也没在废纸篓里等着还原」的提示词。
            // 前者挡住还在别的批次里跑的同一组（同一份 txt 可以被跑两次）；
            // 后者挡住手动删进废纸篓、还等着「还原」的那几条。
            let n = sqlx::query(
                "DELETE FROM prompts WHERE group_id = ?1
                   AND NOT EXISTS (SELECT 1 FROM tasks t WHERE t.prompt_id = prompts.id)
                   AND NOT EXISTS (SELECT 1 FROM trash_items ti
                                   WHERE ti.entity_type = 'prompt' AND ti.ref_id = prompts.id)",
            )
            .bind(gid)
            .execute(&mut *tx)
            .await?
            .rows_affected() as i64;
            report.prompts += n;

            let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM prompts WHERE group_id = ?1")
                .bind(gid)
                .fetch_one(&mut *tx)
                .await?;
            if left == 0 {
                // tag_bindings 是多态表、没有外键，不手动清就会攒下一堆指向已删分组的绑定，
                // 而作品库的「用途」判定正是 `EXISTS(tag_bindings … entity_id = w.group_id)`
                // —— 分组 id 被后来的分组复用时，旧绑定会把用途安到无关的组头上。
                sqlx::query(
                    "DELETE FROM tag_bindings WHERE entity_type = 'prompt_group' AND entity_id = ?1",
                )
                .bind(gid)
                .execute(&mut *tx)
                .await?;
                sqlx::query("DELETE FROM prompt_groups WHERE id = ?1")
                    .bind(gid)
                    .execute(&mut *tx)
                    .await?;
                report.groups += 1;
            }
        }
        tx.commit().await?;
    }
    Ok(report)
}

/// 五视觉组计数（批次汇总）。
pub async fn counts_for_batch(
    pool: &SqlitePool,
    batch_id: i64,
) -> Result<SummaryCounts, sqlx::Error> {
    let rows: Vec<(String, i64)> =
        sqlx::query_as("SELECT status, COUNT(*) FROM tasks WHERE batch_id = ?1 GROUP BY status")
            .bind(batch_id)
            .fetch_all(pool)
            .await?;
    let mut c = SummaryCounts::default();
    for (status, n) in rows {
        match status.as_str() {
            "q" => c.pending += n,
            "run" | "retry" => c.running += n,
            "fail" => c.failed += n,
            "rev" => c.review += n,
            "pass" => c.passed += n,
            "rej" => c.rejected += n,
            _ => {}
        }
        c.total += n;
    }
    Ok(c)
}

/// 批次实际请求次数（含重试）：该批次全部任务的 task_attempts 计数（E15）。
pub async fn request_count_for_batch(pool: &SqlitePool, batch_id: i64) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM task_attempts a
         JOIN tasks t ON t.id = a.task_id WHERE t.batch_id = ?1",
    )
    .bind(batch_id)
    .fetch_one(pool)
    .await
}

/// 中断恢复：run/retry → fail(Interrupted)。返回受影响任务 id。
pub async fn recover_interrupted(pool: &SqlitePool) -> Result<Vec<i64>, sqlx::Error> {
    let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM tasks WHERE status IN ('run','retry')")
        .fetch_all(pool)
        .await?;
    let now = now_unix();
    let mut tx = pool.begin().await?;
    if !ids.is_empty() {
        sqlx::query(
            "UPDATE tasks SET status = 'fail', error_type = 'Interrupted',
                error_message = '上次退出时任务被中断，任务现场已保留，可点击恢复继续', updated_at = ?1
             WHERE status IN ('run','retry')",
        )
        .bind(now)
        .execute(&mut *tx)
        .await?;
    }
    // worker 在 insert_attempt 后、finish_attempt 前退出会留下 outcome='pending'。
    // 它不参与派发，却会永久污染 Key 成功率，看起来也像一条没有归宿的执行记录。
    sqlx::query(
        "UPDATE task_attempts
            SET finished_at = ?1, outcome = 'error', error_type = 'Interrupted',
                error_message = COALESCE(error_message, '上次退出时执行记录未完成'),
                duration_ms = COALESCE(duration_ms, MAX(0, (?1 - started_at) * 1000))
          WHERE outcome = 'pending'",
    )
    .bind(now)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(ids)
}

/// 某 Key 近若干次成功尝试的耗时（ms），用于伪进度 expected 估算。
pub async fn key_success_durations(
    pool: &SqlitePool,
    api_key_id: i64,
    limit: i64,
) -> Result<Vec<i64>, sqlx::Error> {
    let v: Vec<(i64,)> = sqlx::query_as(
        "SELECT duration_ms FROM task_attempts
         WHERE api_key_id = ?1 AND outcome = 'success' AND duration_ms IS NOT NULL
         ORDER BY started_at DESC LIMIT ?2",
    )
    .bind(api_key_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(v.into_iter().map(|(d,)| d).collect())
}

// ---------------- 执行记录 ----------------

pub async fn insert_attempt(
    pool: &SqlitePool,
    task_id: i64,
    api_key_id: i64,
    started_at: i64,
) -> Result<i64, sqlx::Error> {
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO task_attempts (task_id, api_key_id, started_at, outcome)
         VALUES (?1, ?2, ?3, 'pending') RETURNING id",
    )
    .bind(task_id)
    .bind(api_key_id)
    .bind(started_at)
    .fetch_one(pool)
    .await?;
    Ok(id)
}

// task_attempts 落库字段较多，集中一处写入；参数即列，无需再拆结构。
#[allow(clippy::too_many_arguments)]
pub async fn finish_attempt(
    pool: &SqlitePool,
    attempt_id: i64,
    finished_at: i64,
    outcome: &str,
    error_type: Option<&str>,
    error_message: Option<&str>,
    http_status: Option<i64>,
    duration_ms: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE task_attempts SET finished_at = ?2, outcome = ?3, error_type = ?4,
            error_message = ?5, http_status = ?6, duration_ms = ?7 WHERE id = ?1",
    )
    .bind(attempt_id)
    .bind(finished_at)
    .bind(outcome)
    .bind(error_type)
    .bind(error_message)
    .bind(http_status)
    .bind(duration_ms)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即失败，是期望行为
mod tests {
    use super::*;
    use crate::db::test_support::test_pool;

    /// 造 1 批次 + N 任务（默认 q），返回 (batch_id, task_ids)。
    async fn seed(pool: &SqlitePool, n: usize) -> (i64, Vec<i64>) {
        let mut tx = pool.begin().await.unwrap();
        let bid = create_batch(&mut tx, "/out", "{}").await.unwrap();
        sqlx::query("INSERT INTO prompt_groups (id,name,prefix,scene,is_temp,created_at) VALUES (1,'g','GG','',0,0)").execute(&mut *tx).await.unwrap();
        sqlx::query("INSERT INTO prompts (id,group_id,code,text,status,source,created_at,updated_at) VALUES (1,1,'GG-0001','t','active','library',0,0)").execute(&mut *tx).await.unwrap();
        sqlx::query("INSERT INTO ref_images (id,name,file_path,thumb_path,width,height,file_size,created_at) VALUES (1,'r','/a','/t',1,1,1,0)").execute(&mut *tx).await.unwrap();
        let mut ids = Vec::new();
        for _ in 0..n {
            ids.push(insert_task(&mut tx, bid, 1, 1, "t", 1).await.unwrap());
        }
        tx.commit().await.unwrap();
        (bid, ids)
    }

    /// 认领任务是有闸门的：只有仍停在调用方刚读到的那个状态时才认领得到。
    ///
    /// 调度器先 `fetch_queued` 读一批，再逐个派发，两步之间隔着若干次 await。用户在
    /// 这段窗口里点「批量中止」把行删了或改成别的态 —— 无谓词的 UPDATE 会把它重新
    /// 写成 `run`，然后 spawn 一个 worker 去发一次**付费请求**，而结果无处可写。
    #[tokio::test]
    async fn claiming_a_task_requires_it_to_still_be_where_we_left_it() {
        let (pool, _d) = test_pool().await;
        let (_bid, ids) = seed(&pool, 2).await;
        sqlx::query(
            "INSERT INTO api_keys (id,name,keyring_account,base_url,model,concurrency_limit,enabled,created_at)
             VALUES (1,'k','acct','http://x','m',1,1,0)",
        )
        .execute(&pool)
        .await
        .unwrap();

        // 正常：状态没变 → 认领成功。
        assert!(mark_running(&pool, ids[0], 1, "q").await.unwrap());
        assert_eq!(
            get_task(&pool, ids[0]).await.unwrap().unwrap().status,
            "run"
        );

        // 中止：用户在读队列与派发之间把它掐了。
        set_status(&pool, ids[1], "fail").await.unwrap();
        assert!(
            !mark_running(&pool, ids[1], 1, "q").await.unwrap(),
            "状态已经变了 → 不得认领，调度器据此不 spawn worker"
        );
        assert_eq!(
            get_task(&pool, ids[1]).await.unwrap().unwrap().status,
            "fail",
            "认领失败不得把它改回 run"
        );

        // 删除：行都没了。
        delete_tasks_where(&pool, &[ids[1]], &["fail"])
            .await
            .unwrap();
        assert!(!mark_running(&pool, ids[1], 1, "q").await.unwrap());

        // 重试任务走的是 `retry → run`，谓词用调用方读到的状态原文而不是写死 'q' ——
        // 写死会让冷却结束的重试任务永远派发不出去。
        set_status(&pool, ids[0], "retry").await.unwrap();
        assert!(mark_running(&pool, ids[0], 1, "retry").await.unwrap());
    }

    // E22 / D3：归档满 N 天的批次启动时删除（级联任务），作品（accepted_works）不受影响。
    #[tokio::test]
    async fn delete_old_archived_batches_keeps_works() {
        let (pool, _d) = test_pool().await;
        let (bid, ids) = seed(&pool, 2).await;
        // 造一条作品指向该批次任务。
        sqlx::query(
            "INSERT INTO accepted_works (task_id, image_path, thumb_path, prompt_id, prompt_text, ref_image_id, batch_id, accepted_at)
             VALUES (?1, '/out/x.jpg', '/t.jpg', 1, 't', 1, ?2, 0)",
        )
        .bind(ids[0])
        .bind(bid)
        .execute(&pool)
        .await
        .unwrap();
        // 批次归档且 archived_at 为 40 天前。
        let old = now_unix() - 40 * 86_400;
        sqlx::query("UPDATE batches SET status='archived', archived_at=?2 WHERE id=?1")
            .bind(bid)
            .bind(old)
            .execute(&pool)
            .await
            .unwrap();

        // cutoff = 30 天前：40 天前的批次到期。
        let cutoff = now_unix() - 30 * 86_400;
        let deleted = delete_batches_archived_before(&pool, cutoff).await.unwrap();
        assert_eq!(deleted, 1, "到期归档批次应被删除");

        let batch_left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM batches WHERE id=?1")
            .bind(bid)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(batch_left, 0, "批次已删");
        let tasks_left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE batch_id=?1")
            .bind(bid)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(tasks_left, 0, "任务经级联删除");
        // 作品保留（task_id 被置空，记录仍在）——D3「作品不动」。
        let works_left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM accepted_works")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(works_left, 1, "作品是独立快照，不随批次删除");
    }

    // E22：未到期的归档批次与运行中批次不删。
    #[tokio::test]
    async fn delete_old_archived_batches_spares_recent_and_running() {
        let (pool, _d) = test_pool().await;
        let (bid, _) = seed(&pool, 1).await;
        // 归档但仅 5 天前。
        let recent = now_unix() - 5 * 86_400;
        sqlx::query("UPDATE batches SET status='archived', archived_at=?2 WHERE id=?1")
            .bind(bid)
            .bind(recent)
            .execute(&pool)
            .await
            .unwrap();
        let cutoff = now_unix() - 30 * 86_400;
        let deleted = delete_batches_archived_before(&pool, cutoff).await.unwrap();
        assert_eq!(deleted, 0, "未满 30 天不删");
    }

    // E03：取消剩余只删 'q' 态，在途 run/retry 与终态 pass 不受影响。
    #[tokio::test]
    async fn cancel_pending_deletes_only_queued() {
        let (pool, _d) = test_pool().await;
        let (bid, ids) = seed(&pool, 5).await;
        // ids: 0=run(在途) 1=retry(在途) 2=pass(终态) 3,4=q(排队)
        set_status(&pool, ids[0], "run").await.unwrap();
        set_status(&pool, ids[1], "retry").await.unwrap();
        set_status(&pool, ids[2], "pass").await.unwrap();

        let n = cancel_pending(&pool, bid).await.unwrap();
        assert_eq!(n, 2, "仅取消 2 个排队任务");

        let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE batch_id = ?1")
            .bind(bid)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(remaining, 3, "run/retry/pass 三个任务保留");
        let q_left: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE batch_id = ?1 AND status = 'q'")
                .bind(bid)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(q_left, 0, "排队任务全部清空");
        // 在途任务仍在，不被误删。
        assert!(get_task(&pool, ids[0]).await.unwrap().is_some());
        assert!(get_task(&pool, ids[1]).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn manual_recovery_only_updates_failed_tasks_and_reports_real_ids() {
        let (pool, _d) = test_pool().await;
        let (_bid, ids) = seed(&pool, 6).await;
        set_status(&pool, ids[0], "fail").await.unwrap();
        sqlx::query("UPDATE tasks SET retry_count=3 WHERE id=?1")
            .bind(ids[0])
            .execute(&pool)
            .await
            .unwrap();
        set_status(&pool, ids[1], "fail").await.unwrap();
        sqlx::query("UPDATE tasks SET error_type='ContentPolicy' WHERE id=?1")
            .bind(ids[1])
            .execute(&pool)
            .await
            .unwrap();
        set_status(&pool, ids[2], "rev").await.unwrap();
        set_status(&pool, ids[3], "pass").await.unwrap();
        set_status(&pool, ids[4], "rej").await.unwrap();
        set_status(&pool, ids[5], "run").await.unwrap();

        let done = recover_many(&pool, &ids).await.unwrap();
        assert_eq!(done, vec![ids[0]], "只返回真正更新成功的失败任务");
        assert_eq!(
            get_task(&pool, ids[0]).await.unwrap().unwrap().retry_count,
            0,
            "人工恢复应获得一轮完整的自动恢复预算"
        );
        for id in &ids[1..] {
            assert_ne!(
                get_task(&pool, *id).await.unwrap().unwrap().status,
                "q",
                "待验收、完成和在途任务都不得被恢复入口送回生成队列"
            );
        }
        assert!(!recover(&pool, ids[3], None).await.unwrap());
    }

    #[tokio::test]
    async fn delete_task_refuses_running_and_cascades_attempts() {
        let (pool, _d) = test_pool().await;
        let (bid, ids) = seed(&pool, 2).await;
        let t = ids[0];
        // 造一个 Key + 一条 attempt，验证级联删除。
        sqlx::query("INSERT INTO api_keys (id,name,keyring_account,base_url,model,concurrency_limit,enabled,created_at) VALUES (1,'k','acct','http://x/v1','m',2,1,0)")
            .execute(&pool).await.unwrap();
        insert_attempt(&pool, t, 1, 0).await.unwrap();

        // 运行中拒绝删除。
        set_status(&pool, t, "run").await.unwrap();
        assert_eq!(delete_task(&pool, t).await.unwrap(), None, "运行中不应删除");
        // 置回可删终态后删除成功，返回批次 id。
        set_status(&pool, t, "fail").await.unwrap();
        assert_eq!(delete_task(&pool, t).await.unwrap(), Some(bid));
        assert!(get_task(&pool, t).await.unwrap().is_none(), "任务已删除");
        let att: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM task_attempts WHERE task_id = ?1")
            .bind(t)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(att, 0, "attempts 应级联删除");
    }

    #[tokio::test]
    async fn delete_failed_only_removes_failed() {
        let (pool, _d) = test_pool().await;
        let (bid, ids) = seed(&pool, 3).await;
        set_status(&pool, ids[0], "fail").await.unwrap();
        set_status(&pool, ids[1], "fail").await.unwrap();
        // ids[2] 留 q
        let n = delete_failed(&pool, bid).await.unwrap();
        assert_eq!(n, 2);
        let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE batch_id = ?1")
            .bind(bid)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(remaining, 1, "仅失败任务被删，q 保留");
    }

    // ── 批次退出历史（0027：提示词是消耗品） ────────────────────────────

    async fn count_of(pool: &SqlitePool, table: &str) -> i64 {
        sqlx::query_scalar::<_, i64>(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(pool)
            .await
            .unwrap()
    }

    /// 往废纸篓里塞一条指向某任务的「验收未通过」记录（还原按钮的锚点）。
    async fn trash_task(pool: &SqlitePool, task_id: i64) {
        sqlx::query(
            "INSERT INTO trash_items (entity_type, ref_id, source_label, deleted_at)
             VALUES ('task', ?1, '验收未通过', 0)",
        )
        .bind(task_id)
        .execute(pool)
        .await
        .unwrap();
    }

    /// 被删掉的**作品**同样挡住退休 —— 而它不在那条 SQL 的视野里。
    ///
    /// 作品是唯一「删除即真删行」的实体，靠 payload_json 整行写回，于是 trash_items
    /// 与 accepted_works 之间没有任何可 JOIN 的东西：归属只写在那份 JSON 里。
    /// 批次一退休，本批任务级联消失，那份载荷里的 task_id 就成了悬空外键。
    #[tokio::test]
    async fn batch_waits_while_a_deleted_work_of_its_own_sits_in_trash() {
        let (pool, _d) = test_pool().await;
        let (bid, ids) = seed(&pool, 1).await;
        set_status(&pool, ids[0], "pass").await.unwrap();
        let payload = serde_json::json!({
            "id": 1, "task_id": ids[0], "image_path": "/o.jpg", "thumb_path": "/t.jpg",
            "prompt_id": 1, "prompt_text": "t", "group_id": 1, "ref_image_id": 1,
            "batch_id": bid, "favorite": 0, "accepted_at": 0,
            "prompt_code": "GG-0001", "group_name": "g"
        })
        .to_string();
        sqlx::query(
            "INSERT INTO trash_items (entity_type, ref_id, source_label, deleted_at, payload_json)
             VALUES ('work', 1, '作品删除', 0, ?1)",
        )
        .bind(&payload)
        .execute(&pool)
        .await
        .unwrap();

        assert!(
            retire_resolved_batches(&pool).await.unwrap().is_empty(),
            "本批的作品还躺在废纸篓里等还原，批次不许退休"
        );

        // 作品还原回去之后，这一批才轮到退休。
        sqlx::query("DELETE FROM trash_items")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(retire_resolved_batches(&pool).await.unwrap().batches, 1);
    }

    // 主路径：全部任务落在 pass/rej 且废纸篓已清 → 批次连同它消耗掉的提示词与分组一起消失，
    // 而作品一张不掉（accepted_works.task_id 是 ON DELETE SET NULL）。
    #[tokio::test]
    async fn resolved_batch_retires_with_its_prompts_but_keeps_works() {
        let (pool, _d) = test_pool().await;
        let (bid, ids) = seed(&pool, 2).await;
        sqlx::query(
            "INSERT INTO accepted_works (task_id, image_path, thumb_path, prompt_id, prompt_text,
                group_id, ref_image_id, batch_id, accepted_at, prompt_code, group_name)
             VALUES (?1,'/o.jpg','/t.jpg',1,'t',1,1,?2,0,'GG-0001','g')",
        )
        .bind(ids[0])
        .bind(bid)
        .execute(&pool)
        .await
        .unwrap();
        set_status(&pool, ids[0], "pass").await.unwrap();
        set_status(&pool, ids[1], "rej").await.unwrap();

        let r = retire_resolved_batches(&pool).await.unwrap();
        assert_eq!(
            r,
            RetireReport {
                batches: 1,
                prompts: 1,
                groups: 1
            }
        );
        assert_eq!(count_of(&pool, "batches").await, 0);
        assert_eq!(count_of(&pool, "tasks").await, 0, "任务随批次级联删除");
        assert_eq!(count_of(&pool, "prompts").await, 0, "提示词是消耗品");
        assert_eq!(count_of(&pool, "prompt_groups").await, 0);
        assert_eq!(
            count_of(&pool, "accepted_works").await,
            1,
            "作品是长期资产，绝不能跟着上游一起消失"
        );
        // 编号**不**回收：它已经印在输出文件名与作品行上，是花掉的。
        assert_eq!(count_of(&pool, "id_recycled").await, 0);
    }

    // 未通过的结果还躺在废纸篓里 → 不许退休。删了批次那条任务就没了，
    // 「还原回待验收」会指向一个不存在的任务，误删从此不可撤回。
    #[tokio::test]
    async fn batch_waits_while_its_rejects_sit_in_trash() {
        let (pool, _d) = test_pool().await;
        let (_bid, ids) = seed(&pool, 1).await;
        set_status(&pool, ids[0], "rej").await.unwrap();
        trash_task(&pool, ids[0]).await;

        assert!(retire_resolved_batches(&pool).await.unwrap().is_empty());
        assert_eq!(count_of(&pool, "batches").await, 1);

        // 清空废纸篓之后才轮到它。
        sqlx::query("DELETE FROM trash_items")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(retire_resolved_batches(&pool).await.unwrap().batches, 1);
    }

    // 失败任务不算了结：它还等着人决定重试还是删掉。
    #[tokio::test]
    async fn failed_task_keeps_the_batch_alive() {
        let (pool, _d) = test_pool().await;
        let (_bid, ids) = seed(&pool, 2).await;
        set_status(&pool, ids[0], "pass").await.unwrap();
        set_status(&pool, ids[1], "fail").await.unwrap();
        assert!(retire_resolved_batches(&pool).await.unwrap().is_empty());
    }

    // 同一份 txt 可以被跑两次：第一批了结时，第二批还在用的提示词一条都不能删，
    // 否则 tasks.prompt_id 的 ON DELETE CASCADE 会把还在跑的任务顺手带走。
    #[tokio::test]
    async fn prompts_still_used_by_another_batch_survive() {
        let (pool, _d) = test_pool().await;
        let (_b1, ids) = seed(&pool, 1).await;
        let mut tx = pool.begin().await.unwrap();
        let b2 = create_batch(&mut tx, "/out", "{}").await.unwrap();
        let live = insert_task(&mut tx, b2, 1, 1, "t", 1).await.unwrap();
        tx.commit().await.unwrap();
        set_status(&pool, ids[0], "pass").await.unwrap();

        let r = retire_resolved_batches(&pool).await.unwrap();
        assert_eq!(r.batches, 1, "只有第一批了结");
        assert_eq!(r.prompts, 0, "第二批还在用这条提示词");
        assert_eq!(count_of(&pool, "prompt_groups").await, 1);
        let alive: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE id = ?1")
            .bind(live)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(alive, 1, "第二批的任务不得被级联删掉");
    }

    // 分组删掉不影响号池：号池按前缀存 next_seq，同名 txt 再导入一次编号接着往下发，
    // 不会退回去撞上已经发出去的编号。
    #[tokio::test]
    async fn retiring_a_group_does_not_reset_its_number_pool() {
        let (pool, _d) = test_pool().await;
        let (_bid, ids) = seed(&pool, 1).await;
        sqlx::query("INSERT INTO id_pools (prefix, next_seq) VALUES ('GG', 42)")
            .execute(&pool)
            .await
            .unwrap();
        set_status(&pool, ids[0], "pass").await.unwrap();
        retire_resolved_batches(&pool).await.unwrap();
        let next: i64 = sqlx::query_scalar("SELECT next_seq FROM id_pools WHERE prefix='GG'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(next, 42, "号池按前缀活着，与分组的生死无关");
    }

    // 「中止」只掐排队态；在途任务一条都不动 —— 与在途 worker 抢同一行，
    // 会让那份已经花了钱的图无处可写。
    #[tokio::test]
    async fn cancel_only_touches_queued_tasks() {
        let (pool, _d) = test_pool().await;
        let (bid, ids) = seed(&pool, 3).await;
        set_status(&pool, ids[1], "run").await.unwrap();
        set_status(&pool, ids[2], "rev").await.unwrap();

        let (n, batches) = delete_tasks_where(&pool, &ids, &["q"]).await.unwrap();
        assert_eq!(n, 1, "只有那一条 q 被中止");
        assert_eq!(batches, vec![bid], "受影响批次要报出来供重估归档");
        let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(left, 2);
    }
}
