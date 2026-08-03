//! tasks 域命令（列表/详情/失败恢复）。

use serde::Serialize;
use specta::Type;
use tauri::State;

use crate::db::repo::tasks as repo;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// 任务视图（含参考图名/提示词编号/分组名/Key 别名，供任务表直接渲染）。
#[derive(Debug, Clone, Serialize, Type, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct TaskView {
    pub id: i64,
    pub batch_id: i64,
    pub status: String,
    pub ref_image_id: i64,
    pub ref_name: String,
    pub prompt_id: i64,
    pub prompt_code: String,
    pub prompt_title: Option<String>,
    pub group_name: String,
    pub api_key_id: Option<i64>,
    pub key_alias: Option<String>,
    pub error_type: Option<String>,
    pub error_message: Option<String>,
    pub retry_count: i64,
    pub result_thumb_path: Option<String>,
    pub prompt_text_snapshot: String,
}

const TASK_SELECT: &str = "SELECT t.id, t.batch_id, t.status, t.ref_image_id,
        COALESCE(r.name, '') AS ref_name, t.prompt_id,
        COALESCE(p.code, '') AS prompt_code, p.title AS prompt_title,
        COALESCE(g.name, '') AS group_name,
        t.api_key_id, k.name AS key_alias, t.error_type, t.error_message,
        t.retry_count, t.result_thumb_path, t.prompt_text_snapshot
    FROM tasks t
    JOIN batches b ON b.id = t.batch_id
    LEFT JOIN ref_images r ON r.id = t.ref_image_id
    LEFT JOIN prompts p ON p.id = t.prompt_id
    LEFT JOIN prompt_groups g ON g.id = p.group_id
    LEFT JOIN api_keys k ON k.id = t.api_key_id";

/// 列出任务，可按 5 视觉组筛选：all/pending/running/failed/review/done。
///
/// `batch_id = None` = **全部批次**，这是现在的常态：批次不再是可切换的对象
/// （v0.21.0），任务队列答的是「现在还有哪些活」而不是「第 N 批做到哪了」。
/// 批次内保持生成序，批次之间新的在前——与验收页、作品库同一排序。
#[tauri::command]
#[specta::specta]
pub async fn list_tasks(
    state: State<'_, AppState>,
    batch_id: Option<i64>,
    status_group: Option<String>,
    page: Option<i64>,
) -> AppResult<Vec<TaskView>> {
    Ok(query_tasks(&state.db, batch_id, status_group.as_deref(), page).await?)
}

async fn query_tasks(
    pool: &sqlx::SqlitePool,
    batch_id: Option<i64>,
    status_group: Option<&str>,
    page: Option<i64>,
) -> Result<Vec<TaskView>, sqlx::Error> {
    let statuses: &[&str] = match status_group {
        Some("pending") => &["q"],
        Some("running") => &["run", "retry"],
        Some("failed") => &["fail"],
        Some("review") => &["rev"],
        Some("done") => &["pass", "rej"],
        _ => &["q", "run", "retry", "rev", "pass", "rej", "fail"],
    };
    let placeholders = statuses.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    // 当前任务页传 page=None，语义是“全部在制任务”。旧实现仍硬截 500，520/585 张批次
    // 会凭空少一截，汇总与进度也一起算错。仅显式传页码的旧调用保留 500/页。
    let (limit, offset) = match page {
        Some(page) => (500i64, page.max(0) * 500),
        None => (-1i64, 0), // SQLite: LIMIT -1 = 不限
    };
    let batch_cond = if batch_id.is_some() {
        "t.batch_id = ? AND "
    } else {
        ""
    };
    let sql = format!(
        "{TASK_SELECT} WHERE (b.status != 'archived' OR t.status = 'fail')
         AND {batch_cond}t.status IN ({placeholders})
         ORDER BY t.batch_id DESC, t.id ASC LIMIT ? OFFSET ?"
    );
    let mut q = sqlx::query_as::<_, TaskView>(&sql);
    if let Some(b) = batch_id {
        q = q.bind(b);
    }
    for s in statuses {
        q = q.bind(*s);
    }
    let rows = q.bind(limit).bind(offset).fetch_all(pool).await?;
    Ok(rows)
}

/// 批量操作回执：做成了几个、跳过了几个。
///
/// **跳过必须报出来**，不能只报成功数：中止/删除会静默放过在途任务，
/// 而「我选了 30 个，怎么只没了 22 个」如果没人解释，下一步就是再点一次。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct BulkTaskResult {
    pub affected: i64,
    pub skipped: i64,
}

/// 恢复单个无输出的失败任务，可先修改提示词。
#[tauri::command]
#[specta::specta]
pub async fn recover_task(
    state: State<'_, AppState>,
    id: i64,
    edited_prompt: Option<String>,
) -> AppResult<BulkTaskResult> {
    let current: Option<(String, Option<String>, String)> =
        sqlx::query_as("SELECT status, error_type, prompt_text_snapshot FROM tasks WHERE id = ?1")
            .bind(id)
            .fetch_optional(&state.db)
            .await?;
    if let Some((status, Some(error_type), prompt)) = current.as_ref() {
        let prompt_unchanged = match edited_prompt.as_deref() {
            Some(edited) => edited.trim() == prompt.trim(),
            None => true,
        };
        if status == "fail" && error_type == "ContentPolicy" && prompt_unchanged {
            return Err(AppError::InvalidInput(
                "违规任务必须修改提示词后才能恢复".into(),
            ));
        }
    }
    if current
        .as_ref()
        .is_some_and(|(status, _, _)| status == "fail")
    {
        ensure_usable_key(&state)?;
    }
    let affected = i64::from(repo::recover(&state.db, id, edited_prompt.as_deref()).await?);
    if affected > 0 {
        revive_batches(&state, &[id]).await?;
    }
    Ok(BulkTaskResult {
        affected,
        skipped: 1 - affected,
    })
}

/// 恢复全部失败任务。默认排除 ContentPolicy：原提示词再次提交不会改变结果，
/// 这类必须逐条改词后恢复。
#[tauri::command]
#[specta::specta]
pub async fn recover_failed_tasks(state: State<'_, AppState>) -> AppResult<BulkTaskResult> {
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE status='fail'")
        .fetch_one(&state.db)
        .await?;
    let ids: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM tasks WHERE status = 'fail' \
         AND (error_type IS NULL OR error_type != 'ContentPolicy')",
    )
    .fetch_all(&state.db)
    .await?;
    let done = recover_and_revive(&state, &ids).await?;
    Ok(BulkTaskResult {
        affected: done.len() as i64,
        skipped: total - done.len() as i64,
    })
}

/// 回队 + 把它们所属的批次从 archived 拉回 running。
///
/// 归档是「这批没活了」的标记，回队等于又有活了。不改回来的话，退休扫描
/// 会看见一个 archived 却还有 q 态任务的批次——虽然退休条件本身挡得住，
/// 但那个状态本身就是在说谎。
async fn recover_and_revive(state: &State<'_, AppState>, ids: &[i64]) -> AppResult<Vec<i64>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    ensure_usable_key(state)?;
    let done = repo::recover_many(&state.db, ids).await?;
    if done.is_empty() {
        return Ok(done);
    }
    revive_batches(state, &done).await?;
    Ok(done)
}

async fn revive_batches(state: &State<'_, AppState>, done: &[i64]) -> AppResult<()> {
    let ph = done.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!(
        "UPDATE batches SET status = 'running', archived_at = NULL
         WHERE id IN (SELECT DISTINCT batch_id FROM tasks WHERE id IN ({ph}))"
    );
    let mut q = sqlx::query(&sql);
    for id in done {
        q = q.bind(id);
    }
    q.execute(&state.db).await?;
    // “恢复”本身就是用户确认上游已经可再试。清掉上一波并发失败留下的 Key 冷却，
    // 并仅在暂停来自全局自动熔断时自动继续；用户手工暂停仍原样保留。
    state.engine.prepare_manual_retry();
    Ok(())
}

fn ensure_usable_key(state: &State<'_, AppState>) -> AppResult<()> {
    if state.engine.has_usable_key() {
        Ok(())
    } else {
        Err(AppError::InvalidInput(
            "当前没有可用的 API Key。请先到设置中补全或恢复 Key，再恢复任务；任务仍保留在失败列表中。"
                .into(),
        ))
    }
}

/// 批量恢复所选。只有无输出的 fail 参与，其余全部计入 skipped。
#[tauri::command]
#[specta::specta]
pub async fn recover_tasks(state: State<'_, AppState>, ids: Vec<i64>) -> AppResult<BulkTaskResult> {
    let total = ids.len() as i64;
    let done = recover_and_revive(&state, &ids).await?;
    Ok(BulkTaskResult {
        affected: done.len() as i64,
        skipped: total - done.len() as i64,
    })
}

/// 批量删除所选。生成中/重试中的任务拒绝删除（与在途 worker 抢同一行会让那份图
/// 谁也找不到），计入 skipped。
#[tauri::command]
#[specta::specta]
pub async fn delete_tasks(state: State<'_, AppState>, ids: Vec<i64>) -> AppResult<BulkTaskResult> {
    const DELETABLE: &[&str] = &["q", "rev", "pass", "rej", "fail"];
    let total = ids.len() as i64;
    let (n, batches) = repo::delete_tasks_where(&state.db, &ids, DELETABLE).await?;
    settle(&state, &batches).await;
    Ok(BulkTaskResult {
        affected: n,
        skipped: total - n,
    })
}

/// 批量中止所选：只掐掉**还没开跑**的排队任务。
///
/// 已经发出去的请求中止不了——钱在请求发出的那一刻就花了，硬把行删掉只会让结果
/// 回来时无处可写。故在途任务一律计入 skipped 并如实说明，而不是假装中止成功。
#[tauri::command]
#[specta::specta]
pub async fn cancel_tasks(state: State<'_, AppState>, ids: Vec<i64>) -> AppResult<BulkTaskResult> {
    let total = ids.len() as i64;
    let (n, batches) = repo::delete_tasks_where(&state.db, &ids, &["q"]).await?;
    settle(&state, &batches).await;
    Ok(BulkTaskResult {
        affected: n,
        skipped: total - n,
    })
}

/// 任务被删/被中止之后的收尾：重估归档 → 补发汇总 → 跑一遍退休扫描。
/// 顺序不能反，退休会把批次删掉，而 emit_summary 要对着还在的批次算数。
async fn settle(state: &State<'_, AppState>, batches: &[i64]) {
    for b in batches {
        let _ = repo::archive_if_all_terminal(&state.db, *b).await;
        state.engine.emit_summary(*b).await;
    }
    if !batches.is_empty() {
        crate::commands::batches::retire_batches_quietly(&state.db).await;
    }
}

/// 恢复全部因中断而失败的任务（error_type=Interrupted）。
#[tauri::command]
#[specta::specta]
pub async fn recover_interrupted_tasks(state: State<'_, AppState>) -> AppResult<BulkTaskResult> {
    let ids: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM tasks WHERE status = 'fail' AND error_type = 'Interrupted'",
    )
    .fetch_all(&state.db)
    .await?;
    let total = ids.len() as i64;
    let done = recover_and_revive(&state, &ids).await?;
    Ok(BulkTaskResult {
        affected: done.len() as i64,
        skipped: total - done.len() as i64,
    })
}

/// 删除单个任务（「不需要了」）。生成中/重试中的任务不允许删除，避免与在途 worker 竞争；
/// 删除会级联清除 task_attempts（外键 ON DELETE CASCADE）。删除后重估批次归档并补发汇总。
#[tauri::command]
#[specta::specta]
pub async fn delete_task(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    let Some(batch_id) = repo::delete_task(&state.db, id).await? else {
        return Err(AppError::InvalidInput(
            "任务不存在或正在生成中，无法删除".into(),
        ));
    };
    settle(&state, &[batch_id]).await;
    Ok(())
}

/// 删除全部失败任务（批量「不需要了」）。跨全部批次。返回删除数。
#[tauri::command]
#[specta::specta]
pub async fn delete_failed_tasks(state: State<'_, AppState>) -> AppResult<i64> {
    let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM tasks WHERE status = 'fail'")
        .fetch_all(&state.db)
        .await?;
    let (n, batches) = repo::delete_tasks_where(&state.db, &ids, &["fail"]).await?;
    settle(&state, &batches).await;
    Ok(n)
}

/// 统计当前中断任务数（前端 banner 用）。
#[tauri::command]
#[specta::specta]
pub async fn count_interrupted(state: State<'_, AppState>) -> AppResult<i64> {
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM tasks WHERE status = 'fail' AND error_type = 'Interrupted'",
    )
    .fetch_one(&state.db)
    .await?;
    Ok(n)
}

#[cfg(test)]
// 测试断言失败即测试失败，允许直接 unwrap/expect 保持夹具清晰。
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::db::test_support::test_pool;

    #[tokio::test]
    async fn completed_archived_batches_do_not_reappear_as_zombie_queue_rows() {
        let (pool, _dir) = test_pool().await;
        sqlx::query(
            "INSERT INTO prompt_groups (id,name,prefix,scene,is_temp,created_at)
             VALUES (1,'g','GG','',0,0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO prompts (id,group_id,code,text,status,source,created_at,updated_at)
             VALUES (1,1,'GG-0001','t','active','library',0,0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO ref_images (id,name,file_path,thumb_path,width,height,file_size,created_at)
             VALUES (1,'r','/r','/t',1,1,1,0)",
        )
        .execute(&pool)
        .await
        .unwrap();

        for (batch_id, batch_status, task_status) in [
            (1, "archived", "pass"),
            (2, "archived", "fail"),
            (3, "running", "q"),
        ] {
            sqlx::query(
                "INSERT INTO batches (id,created_at,output_dir,params_json,status)
                 VALUES (?1,0,'/out','{}',?2)",
            )
            .bind(batch_id)
            .bind(batch_status)
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO tasks
                 (id,batch_id,ref_image_id,prompt_id,prompt_text_snapshot,status,created_at,updated_at)
                 VALUES (?1,?1,1,1,'t',?2,0,0)",
            )
            .bind(batch_id)
            .bind(task_status)
            .execute(&pool)
            .await
            .unwrap();
        }

        // 把运行中批次扩到 501 条，钉住 page=None 不能再被旧的 500 行上限截断。
        sqlx::query(
            "WITH RECURSIVE seq(id) AS (
               SELECT 4 UNION ALL SELECT id + 1 FROM seq WHERE id < 503
             )
             INSERT INTO tasks
               (id,batch_id,ref_image_id,prompt_id,prompt_text_snapshot,status,created_at,updated_at)
             SELECT id,3,1,1,'t','q',0,0 FROM seq",
        )
        .execute(&pool)
        .await
        .unwrap();

        let rows = query_tasks(&pool, None, None, None).await.unwrap();
        let ids: Vec<i64> = rows.into_iter().map(|row| row.id).collect();
        assert_eq!(ids.len(), 502, "501 条在制 + 1 条真实失败必须全部返回");
        assert_eq!(ids.first(), Some(&3));
        assert_eq!(ids.last(), Some(&2));

        let page = query_tasks(&pool, None, None, Some(0)).await.unwrap();
        assert_eq!(page.len(), 500, "显式分页仍保留 500/页");
    }
}
