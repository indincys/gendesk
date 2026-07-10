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
}

/// 验收结果。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AcceptResult {
    pub accepted: i64,
    /// 本次因验收通过而转正的临时分组名（前端 toast）
    pub promoted_groups: Vec<String>,
}

const REVIEW_SELECT: &str = "SELECT t.id, t.batch_id, COALESCE(r.name,'') AS ref_name,
        COALESCE(p.code,'') AS prompt_code, COALESCE(g.name,'') AS group_name,
        k.name AS key_alias, t.result_image_path, t.result_thumb_path, t.prompt_text_snapshot AS prompt_text
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
    let (sql, bind) = match batch_id {
        Some(_) => (
            format!("{REVIEW_SELECT} AND t.batch_id = ? ORDER BY t.id ASC"),
            batch_id,
        ),
        None => (format!("{REVIEW_SELECT} ORDER BY t.id ASC"), None),
    };
    let mut q = sqlx::query_as::<_, ReviewItemView>(&sql);
    if let Some(b) = bind {
        q = q.bind(b);
    }
    Ok(q.fetch_all(&state.db).await?)
}

/// 验收通过内部承载。
#[derive(FromRow)]
struct AcceptRow {
    id: i64,
    batch_id: i64,
    ref_image_id: i64,
    prompt_id: i64,
    prompt_text_snapshot: String,
    result_image_path: Option<String>,
    result_thumb_path: Option<String>,
    ref_name: String,
    prompt_code: String,
    prompt_text: String,
    group_id: Option<i64>,
    is_temp: i64,
    group_name: String,
}

const ACCEPT_SELECT: &str = "SELECT t.id, t.batch_id, t.ref_image_id, t.prompt_id,
        t.prompt_text_snapshot, t.result_image_path, t.result_thumb_path,
        COALESCE(r.name,'') AS ref_name, COALESCE(p.code,'') AS prompt_code,
        COALESCE(p.text,'') AS prompt_text, p.group_id,
        COALESCE(g.is_temp,0) AS is_temp, COALESCE(g.name,'') AS group_name
    FROM tasks t
    LEFT JOIN ref_images r ON r.id = t.ref_image_id
    LEFT JOIN prompts p ON p.id = t.prompt_id
    LEFT JOIN prompt_groups g ON g.id = p.group_id
    WHERE t.id = ? AND t.status = 'rev'";

/// 通过所选：输出原图 + 写作品快照 + 微调写回(R8) + 临时组转正(R7)，单事务。
#[tauri::command]
#[specta::specta]
pub async fn accept_tasks(
    state: State<'_, AppState>,
    task_ids: Vec<i64>,
) -> AppResult<AcceptResult> {
    let mut promoted: Vec<String> = Vec::new();
    let mut accepted = 0i64;
    let mut batches: Vec<i64> = Vec::new();
    let date = files::date_yymmdd(now_unix());

    for tid in task_ids {
        let Some(row) = sqlx::query_as::<_, AcceptRow>(ACCEPT_SELECT)
            .bind(tid)
            .fetch_optional(&state.db)
            .await?
        else {
            continue;
        };
        let Some(src) = row.result_image_path.clone() else {
            continue; // 无结果图，跳过
        };

        // 输出到 outputs/{批次}/参考图名_YYMMDD_编号.JPG
        let out_dir = state.dirs.outputs().join(row.batch_id.to_string());
        std::fs::create_dir_all(&out_dir)?;
        let filename = files::output_filename(&row.ref_name, &row.prompt_code, &date);
        let out_path = out_dir.join(&filename);
        // 拷贝失败必须上报：否则会记录 pass + works 指向不存在的输出文件（磁盘满/源丢失）。
        std::fs::copy(&src, &out_path)?;

        let thumb = row.result_thumb_path.clone().unwrap_or_else(|| src.clone());

        // 事务：写作品 + 微调写回 + 临时组转正 + 状态迁移。
        let mut tx = state.db.begin().await?;
        work_repo::insert(
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
        task_repo::set_status(&state.db, row.id, "pass").await?;
        accepted += 1;
        if !batches.contains(&row.batch_id) {
            batches.push(row.batch_id);
        }
    }

    for b in &batches {
        let _ = task_repo::archive_if_all_terminal(&state.db, *b).await;
        // 验收改变了任务态：补发批次汇总，驱动侧栏「待验收」徽章即时更新。
        state.engine.emit_summary(*b).await;
    }
    promoted.dedup();
    Ok(AcceptResult {
        accepted,
        promoted_groups: promoted,
    })
}

/// 不通过所选：置 rej + 原图删除（留缩略图）+ 进废纸篓，单事务。
#[tauri::command]
#[specta::specta]
pub async fn reject_tasks(state: State<'_, AppState>, task_ids: Vec<i64>) -> AppResult<i64> {
    let mut rejected = 0i64;
    let mut batches: Vec<i64> = Vec::new();

    for tid in task_ids {
        let Some(row) = sqlx::query_as::<_, AcceptRow>(ACCEPT_SELECT)
            .bind(tid)
            .fetch_optional(&state.db)
            .await?
        else {
            continue;
        };

        // 原图物理删除，缩略图留存进废纸篓。
        if let Some(img) = &row.result_image_path {
            let _ = files::purge(&PathBuf::from(img));
        }
        let mut tx = state.db.begin().await?;
        trash_repo::insert(
            &mut tx,
            &trash_repo::NewTrashItem {
                entity_type: "task".into(),
                ref_id: Some(row.id),
                thumb_path: row.result_thumb_path.clone(),
                prompt_text: Some(row.prompt_text_snapshot.clone()),
                code: Some(row.prompt_code.clone()),
                source_label: "验收未通过".into(),
                file_paths: row.result_thumb_path.iter().cloned().collect(),
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
        let row = sqlx::query_as::<_, AcceptRow>(ACCEPT_SELECT)
            .bind(1)
            .fetch_optional(&pool)
            .await
            .unwrap();
        assert!(row.is_none(), "已通过任务不应被验收命令再次选中");

        sqlx::query("UPDATE tasks SET status='rev' WHERE id=1")
            .execute(&pool)
            .await
            .unwrap();
        let row = sqlx::query_as::<_, AcceptRow>(ACCEPT_SELECT)
            .bind(1)
            .fetch_optional(&pool)
            .await
            .unwrap();
        assert!(row.is_some(), "rev 待验收任务应被选中");
    }
}
