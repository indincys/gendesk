//! batches / tasks 域命令（执行计划 2.1 引擎）。

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::State;

use crate::db::repo::tasks as repo;
use crate::engine::{self, RefMapping, TaskSpec};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

#[derive(Debug, Clone, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RefMappingInput {
    pub ref_image_id: i64,
    pub prompt_group_id: i64,
    #[serde(default)]
    pub attachments: Vec<i64>,
}

#[derive(Debug, Clone, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CreateBatchInput {
    pub refs: Vec<RefMappingInput>,
    #[serde(default)]
    pub prompt_edits: Vec<PromptEdit>,
    pub params_json: String,
}

#[derive(Debug, Clone, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PromptEdit {
    pub id: i64,
    pub text: Option<String>,
    pub excluded: bool,
}

/// 建批回执。**批次不再是一个可管理的对象**（v0.21.0）：它没有列表、没有切换器、
/// 没有重命名，也不能「按此配置再来一批」——跑完就退出历史（`retire_resolved_batches`）。
/// 剩下的只是「这一次点下去产生了什么」，故这个结构只回答那一句。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct BatchView {
    pub id: i64,
    pub created_at: i64,
    pub status: String,
    pub task_count: i64,
    /// 批次生效的生成参数快照（E16 / D1）。
    pub params_json: String,
}

/// 跑一遍「已了结的批次退出历史」。
///
/// 只记日志、不打断调用方：它是每次验收/删除/清废纸篓之后**顺手**做的收尾，
/// 失败了下一次还会再扫（条件是幂等的），不该让一次验收因此报错。
pub async fn retire_batches_quietly(pool: &sqlx::SqlitePool) {
    match repo::retire_resolved_batches(pool).await {
        Ok(r) if !r.is_empty() => tracing::info!(
            batches = r.batches,
            prompts = r.prompts,
            groups = r.groups,
            "批次已了结，退出历史（提示词随之消耗）"
        ),
        Ok(_) => {}
        Err(e) => tracing::warn!(error = %e, "批次退休扫描失败"),
    }
}

/// 组合展开创建批次，调度器自动开跑。返回批次视图（含任务总数）。
#[tauri::command]
#[specta::specta]
pub async fn create_batch(
    state: State<'_, AppState>,
    input: CreateBatchInput,
) -> AppResult<BatchView> {
    if input.refs.is_empty() {
        return Err(AppError::InvalidInput("未选择任何参考图挂靠".into()));
    }
    // 花钱之前的本地预检：受控取值/尺寸边长/压缩区间。端点的拒绝发生在计费之后，
    // 一批 20 个任务会连报 20 次同一个错。严格解析同时挡住「键类型不对 → 整份参数
    // 静默退化成空 → 选了 9:16 却一个字段都没发出去」。
    crate::provider::GenParams::parse_checked(&input.params_json)
        .map_err(AppError::InvalidInput)?;
    crate::library::Organization::from_params(&input.params_json)?;
    let params_snapshot = input.params_json.clone();
    let output_dir = state.dirs.outputs().to_string_lossy().to_string();
    let (batch_id, count) = if input.prompt_edits.is_empty()
        && input.refs.iter().all(|item| item.attachments.is_empty())
    {
        let mappings: Vec<_> = input
            .refs
            .iter()
            .map(|item| RefMapping {
                ref_image_id: item.ref_image_id,
                prompt_group_id: item.prompt_group_id,
                params_json: None,
            })
            .collect();
        engine::create_batch(&state.db, &output_dir, &input.params_json, &mappings).await?
    } else {
        create_edited_batch(&state.db, &output_dir, input).await?
    };
    state.engine.kick();
    Ok(BatchView {
        id: batch_id,
        created_at: crate::db::now_unix(),
        status: "running".into(),
        task_count: count,
        params_json: params_snapshot,
    })
}

async fn create_edited_batch(
    pool: &sqlx::SqlitePool,
    output_dir: &str,
    input: CreateBatchInput,
) -> AppResult<(i64, i64)> {
    let edits: std::collections::HashMap<_, _> = input
        .prompt_edits
        .iter()
        .map(|edit| (edit.id, edit))
        .collect();
    let mut tx = pool.begin().await?;
    let mut specs = Vec::new();
    for mapping in input.refs {
        let mut params: serde_json::Value = serde_json::from_str(&input.params_json)?;
        let ids: Vec<_> = std::iter::once(mapping.ref_image_id)
            .chain(mapping.attachments)
            .collect();
        params["referenceImageIds"] = serde_json::to_value(&ids)?;
        let params_json = params.to_string();
        engine::reference_ids(&params_json, mapping.ref_image_id)?;
        for id in ids {
            let path: Option<String> = sqlx::query_scalar(
                "SELECT file_path FROM ref_images WHERE id=? AND deleted_at IS NULL",
            )
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
            if !path
                .as_deref()
                .is_some_and(|path| std::path::Path::new(path).is_file())
            {
                return Err(AppError::InvalidInput(format!("参考图片 #{id} 已失效")));
            }
        }
        let prompts: Vec<(i64, String)> = sqlx::query_as(
            "SELECT id,text FROM prompts WHERE group_id=? AND status='active' ORDER BY id",
        )
        .bind(mapping.prompt_group_id)
        .fetch_all(&mut *tx)
        .await?;
        let before = specs.len();
        for (id, text) in prompts {
            let edit = edits.get(&id).copied();
            if edit.is_some_and(|edit| edit.excluded) {
                continue;
            }
            let text = edit
                .and_then(|edit| edit.text.as_deref())
                .unwrap_or(&text)
                .trim();
            if text.is_empty() {
                return Err(AppError::InvalidInput(format!("提示词 #{id} 为空")));
            }
            specs.push(TaskSpec {
                ref_image_id: mapping.ref_image_id,
                prompt_id: id,
                prompt_group_id: mapping.prompt_group_id,
                prompt_text: text.into(),
                params_json: params_json.clone(),
            });
        }
        if specs.len() == before {
            return Err(AppError::InvalidInput(format!(
                "提示词组 #{} 没有本次可用的条目",
                mapping.prompt_group_id
            )));
        }
    }
    let result =
        engine::create_batch_from_specs_in_tx(&mut tx, output_dir, &input.params_json, &specs)
            .await?;
    tx.commit().await?;
    Ok(result)
}

/// 历史单张生成均值秒数（E31 确认摘要 ETA 估算）；无成功历史返回 None。
#[tauri::command]
#[specta::specta]
pub async fn estimate_task_seconds(state: State<'_, AppState>) -> AppResult<Option<f64>> {
    // 近 50 次成功尝试的平均耗时。
    let avg: Option<f64> = sqlx::query_scalar(
        "SELECT AVG(duration_ms) FROM (
            SELECT duration_ms FROM task_attempts
            WHERE outcome = 'success' AND duration_ms IS NOT NULL
            ORDER BY id DESC LIMIT 50)",
    )
    .fetch_one(&state.db)
    .await?;
    Ok(avg.map(|ms| ms / 1000.0))
}

/// 在系统文件管理器打开输出根目录 `outputs/`。
///
/// **这里只剩历史文件了**：验收通过的图现在交付到交付根（`commands::delivery`），
/// `outputs/{批次}/{分组}/` 已停止写入（W6 D4）。留着这个入口是因为存量还在里面，
/// 而删掉它等于让那些图从界面上消失。
#[tauri::command]
#[specta::specta]
pub async fn open_outputs_dir(state: State<'_, AppState>, app: tauri::AppHandle) -> AppResult<()> {
    use tauri_plugin_opener::OpenerExt;
    let dir = crate::library::root(&state.db).await?;
    std::fs::create_dir_all(&dir).map_err(|e| AppError::Io(e.to_string()))?;
    app.opener()
        .open_path(dir.to_string_lossy().to_string(), None::<&str>)
        .map_err(|e| AppError::Io(e.to_string()))
}

#[tauri::command]
#[specta::specta]
pub async fn pause_queue(state: State<'_, AppState>) -> AppResult<()> {
    state.engine.pause();
    persist_paused(&state.db, true).await
}

#[tauri::command]
#[specta::specta]
pub async fn resume_queue(state: State<'_, AppState>) -> AppResult<()> {
    state.engine.resume();
    persist_paused(&state.db, false).await
}

/// 把暂停态写回 settings（持久化，重启沿用）。
async fn persist_paused(pool: &sqlx::SqlitePool, paused: bool) -> AppResult<()> {
    let current = crate::db::repo::settings::get_raw(pool).await?;
    let mut v: serde_json::Value = current
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    v["paused"] = serde_json::Value::Bool(paused);
    crate::db::repo::settings::set_raw(pool, &v.to_string()).await?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即失败
mod tests {
    use super::*;

    #[tokio::test]
    async fn edited_batch_preserves_source_prompts_and_real_attachment_order() {
        let (pool, dir) = crate::db::test_support::test_pool().await;
        let path = dir.path().join("ref.png");
        std::fs::write(&path, b"fixture").unwrap();
        for id in 1..=3 {
            sqlx::query("INSERT INTO ref_images(id,name,file_path,thumb_path,width,height,file_size,created_at) VALUES(?,'r',?,'',1,1,7,0)")
                .bind(id).bind(path.to_str()).execute(&pool).await.unwrap();
        }
        sqlx::query("INSERT INTO prompt_groups(id,name,prefix,created_at) VALUES(1,'测试','QA',0)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO prompts(id,group_id,code,text,created_at,updated_at) VALUES(1,1,'QA-1','原提示词',0,0),(2,1,'QA-2','要排除',0,0)")
            .execute(&pool).await.unwrap();
        let input = CreateBatchInput {
            refs: vec![RefMappingInput {
                ref_image_id: 1,
                prompt_group_id: 1,
                attachments: vec![3, 2],
            }],
            prompt_edits: vec![
                PromptEdit {
                    id: 1,
                    text: Some("本批用词".into()),
                    excluded: false,
                },
                PromptEdit {
                    id: 2,
                    text: None,
                    excluded: true,
                },
            ],
            params_json: "{}".into(),
        };
        let (batch, count) = create_edited_batch(&pool, "", input.clone()).await.unwrap();
        assert_eq!(count, 1);
        let (prompt, params): (String, String) =
            sqlx::query_as("SELECT prompt_text_snapshot,params_json FROM tasks WHERE batch_id=?")
                .bind(batch)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(prompt, "本批用词");
        assert_eq!(engine::reference_ids(&params, 1).unwrap(), [1, 3, 2]);
        let original: String = sqlx::query_scalar("SELECT text FROM prompts WHERE id=1")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(original, "原提示词");
        let mut invalid = input;
        invalid.refs[0].attachments.push(3);
        assert!(create_edited_batch(&pool, "", invalid).await.is_err());
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM batches")
                .fetch_one(&pool)
                .await
                .unwrap(),
            1
        );
    }
}
