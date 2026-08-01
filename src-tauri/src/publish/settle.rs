//! 回执收回与关单结算。

use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;
use sqlx::SqlitePool;

use crate::db::now_unix;
use crate::error::{AppError, AppResult};
use crate::publish::{paths, receipt};

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ReceiptImportResult {
    pub applied: i64,
    pub done: i64,
    pub failed: i64,
    pub pending: i64,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CloseResult {
    pub deleted_files: i64,
    pub delete_failures: Vec<String>,
    pub report_json: String,
}

type ExistingTaskResult = (String, Option<String>, Option<String>, Option<i64>);

pub async fn import_receipts(pool: &SqlitePool, sheet_id: i64) -> AppResult<ReceiptImportResult> {
    let row: Option<(String, String, Option<String>)> =
        sqlx::query_as("SELECT status,export_dir,export_token FROM task_sheets WHERE id=?1")
            .bind(sheet_id)
            .fetch_optional(pool)
            .await?;
    let Some((status, export_dir, export_token)) = row else {
        return Err(AppError::InvalidInput("任务单不存在".into()));
    };
    if export_token.is_some() {
        return Err(AppError::InvalidInput(
            "任务单仍在导出收尾中，请稍后再收取回执".into(),
        ));
    }
    if !matches!(status.as_str(), "exported" | "reconciling") {
        return Err(AppError::InvalidInput("只有已导出任务单可收回回执".into()));
    }
    let content = std::fs::read_to_string(Path::new(&export_dir).join(paths::RECEIPT_JSONL))?;
    let receipts = receipt::parse_jsonl(&content).map_err(AppError::InvalidInput)?;
    if receipts.is_empty() {
        let (done, failed, pending): (i64, i64, i64) = sqlx::query_as(
            "SELECT COALESCE(SUM(status='done'),0),COALESCE(SUM(status='failed'),0),
                    COALESCE(SUM(status='pending'),0) FROM publish_tasks WHERE sheet_id=?1",
        )
        .bind(sheet_id)
        .fetch_one(pool)
        .await?;
        return Ok(ReceiptImportResult {
            applied: 0,
            done,
            failed,
            pending,
        });
    }
    let mut tx = pool.begin().await?;
    let mut applied = 0;
    for line in receipts {
        let status_code = if line.status == "已完成" {
            "done"
        } else {
            "failed"
        };
        let result_time =
            chrono::NaiveDateTime::parse_from_str(&line.finished_at, "%Y-%m-%d %H:%M")
                .ok()
                .and_then(|value| value.and_local_timezone(chrono::Local).single())
                .map(|value| value.timestamp());
        let existing: Option<ExistingTaskResult> = sqlx::query_as(
            "SELECT status,fail_kind,result_msg,result_time FROM publish_tasks
                 WHERE sheet_id=?1 AND task_code=?2",
        )
        .bind(sheet_id)
        .bind(&line.task_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((existing_status, existing_kind, existing_message, existing_time)) = existing
        else {
            return Err(AppError::InvalidInput(format!(
                "回执 taskId 不属于本单：{}",
                line.task_id
            )));
        };
        if existing_status != "pending" {
            let same_terminal = existing_status == status_code
                && existing_kind == line.fail_kind
                && existing_message.as_deref() == Some(line.message.as_str())
                && existing_time == result_time;
            if same_terminal {
                continue;
            }
            return Err(AppError::InvalidInput(format!(
                "回执 taskId {} 已有终态，拒绝重放或改写",
                line.task_id
            )));
        }
        let changed = sqlx::query(
            "UPDATE publish_tasks SET status=?3,fail_kind=?4,result_msg=?5,result_time=?6,updated_at=?7
             WHERE sheet_id=?1 AND task_code=?2 AND status='pending'",
        )
        .bind(sheet_id)
        .bind(&line.task_id)
        .bind(status_code)
        .bind(line.fail_kind)
        .bind(line.message)
        .bind(result_time)
        .bind(now_unix())
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if changed != 1 {
            return Err(AppError::InvalidInput(format!(
                "回执 taskId {} 被并发更新，请重新收取",
                line.task_id
            )));
        }
        applied += 1;
    }
    let post_ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM posts WHERE sheet_id=?1")
        .bind(sheet_id)
        .fetch_all(&mut *tx)
        .await?;
    for post_id in post_ids {
        let (done, pending): (i64, i64) = sqlx::query_as(
            "SELECT COALESCE(SUM(status='done'),0),COALESCE(SUM(status='pending'),0)
             FROM publish_tasks WHERE post_id=?1",
        )
        .bind(post_id)
        .fetch_one(&mut *tx)
        .await?;
        if done == 0 && pending == 0 {
            // 失败帖仍由 title_text/body_text 保存快照。释放库存前断开外键，避免这条
            // free 文案之后无法在文案库删除。
            sqlx::query("UPDATE posts SET title_id=NULL,body_id=NULL WHERE id=?1")
                .bind(post_id)
                .execute(&mut *tx)
                .await?;
            sqlx::query("UPDATE image_assets SET state='free',post_id=NULL,updated_at=?2 WHERE post_id=?1 AND state='used'")
                .bind(post_id)
                .bind(now_unix())
                .execute(&mut *tx)
                .await?;
            sqlx::query(
                "UPDATE text_items SET state='free',post_id=NULL WHERE post_id=?1 AND state='used'",
            )
            .bind(post_id)
            .execute(&mut *tx)
            .await?;
        }
    }
    let advanced = sqlx::query(
        "UPDATE task_sheets SET status='reconciling',updated_at=?2
         WHERE id=?1 AND status IN ('exported','reconciling') AND export_token IS NULL",
    )
    .bind(sheet_id)
    .bind(now_unix())
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if advanced != 1 {
        return Err(AppError::InvalidInput(
            "任务单状态已被并发更新，回执未写入，请刷新后重试".into(),
        ));
    }
    let (done, failed, pending): (i64, i64, i64) = sqlx::query_as(
        "SELECT COALESCE(SUM(status='done'),0),COALESCE(SUM(status='failed'),0),
                COALESCE(SUM(status='pending'),0) FROM publish_tasks WHERE sheet_id=?1",
    )
    .bind(sheet_id)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(ReceiptImportResult {
        applied,
        done,
        failed,
        pending,
    })
}

pub async fn close(pool: &SqlitePool, root: &Path, sheet_id: i64) -> AppResult<CloseResult> {
    let pending: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM publish_tasks WHERE sheet_id=?1 AND status='pending'",
    )
    .bind(sheet_id)
    .fetch_one(pool)
    .await?;
    if pending > 0 {
        return Err(AppError::InvalidInput(format!(
            "仍有 {pending} 条待执行，不能关单"
        )));
    }
    let state: Option<(String, Option<String>)> =
        sqlx::query_as("SELECT status,export_token FROM task_sheets WHERE id=?1")
            .bind(sheet_id)
            .fetch_optional(pool)
            .await?;
    let Some((status, export_token)) = state else {
        return Err(AppError::InvalidInput("任务单不存在".into()));
    };
    if export_token.is_some() {
        return Err(AppError::InvalidInput(
            "任务单仍在导出收尾中，请稍后再关闭".into(),
        ));
    }
    if !matches!(status.as_str(), "reconciling" | "exported") {
        return Err(AppError::InvalidInput("任务单当前状态不能关单".into()));
    }
    let failures: Vec<(Option<String>, i64)> = sqlx::query_as(
        "SELECT fail_kind,COUNT(*) FROM publish_tasks WHERE sheet_id=?1 AND status='failed' GROUP BY fail_kind",
    )
    .bind(sheet_id)
    .fetch_all(pool)
    .await?;
    let mut report = BTreeMap::new();
    for (kind, count) in failures {
        report.insert(kind.unwrap_or_else(|| "其他".into()), count);
    }
    let report_json = serde_json::to_string(&report)?;
    let assets: Vec<(i64, String)> = sqlx::query_as(
        "SELECT DISTINCT a.id,a.path_rel FROM image_assets a
         JOIN post_images pi ON pi.asset_id=a.id JOIN posts p ON p.id=pi.post_id
         WHERE p.sheet_id=?1 AND a.state='used'
           AND EXISTS(SELECT 1 FROM publish_tasks t WHERE t.post_id=p.id AND t.status='done')",
    )
    .bind(sheet_id)
    .fetch_all(pool)
    .await?;
    let mut deleted_files = 0;
    let mut delete_failures = Vec::new();
    let mut cleanup_ids = Vec::new();
    for (asset_id, rel) in &assets {
        let path = paths::RelPath::new(rel).to_local(root);
        match std::fs::remove_file(&path) {
            Ok(()) => {
                cleanup_ids.push(*asset_id);
                deleted_files += 1;
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                cleanup_ids.push(*asset_id);
            }
            Err(err) => delete_failures.push(format!("{}：{err}", path.display())),
        }
    }
    // 文件删除有任何失败时不推进状态、不删库存行；已成功删除的文件下次会按
    // NotFound 处理，因此用户修复权限后可安全重试，不会卡在 closed 死角。
    if !delete_failures.is_empty() {
        return Ok(CloseResult {
            deleted_files,
            delete_failures,
            report_json,
        });
    }

    let mut tx = pool.begin().await?;
    for asset_id in cleanup_ids {
        sqlx::query("DELETE FROM post_images WHERE asset_id=?1")
            .bind(asset_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM image_assets WHERE id=?1 AND state='used'")
            .bind(asset_id)
            .execute(&mut *tx)
            .await?;
    }
    // 帖子保留 title_text/body_text 快照即可。文件全部处理成功后，才在一个事务中
    // 淘汰库存并推进 closed；事务失败时状态仍可重试。
    sqlx::query(
        "UPDATE posts SET title_id=NULL,body_id=NULL WHERE sheet_id=?1 AND EXISTS(
           SELECT 1 FROM publish_tasks t WHERE t.post_id=posts.id AND t.status='done'
         )",
    )
    .bind(sheet_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "DELETE FROM text_items WHERE state='used' AND post_id IN (
           SELECT DISTINCT p.id FROM posts p JOIN publish_tasks t ON t.post_id=p.id
           WHERE p.sheet_id=?1 AND t.status='done'
         )",
    )
    .bind(sheet_id)
    .execute(&mut *tx)
    .await?;
    let closed = sqlx::query(
        "UPDATE task_sheets SET status='closed',report_json=?2,closed_at=?3,updated_at=?3
         WHERE id=?1 AND status IN ('exported','reconciling') AND export_token IS NULL",
    )
    .bind(sheet_id)
    .bind(&report_json)
    .bind(now_unix())
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if closed != 1 {
        return Err(AppError::InvalidInput("任务单状态已变化，请重试".into()));
    }
    tx.commit().await?;
    Ok(CloseResult {
        deleted_files,
        delete_failures,
        report_json,
    })
}
