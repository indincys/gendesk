//! Deliver a committed GenDesk review to the Codex chat that submitted the sample.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{Sqlite, SqlitePool, Transaction};
use tauri::{AppHandle, Manager as _};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader, Lines};
use tokio::process::{ChildStdin, ChildStdout};
use tokio::task::JoinSet;

use crate::db::now_unix;
use crate::intake::Plan;
use crate::{AppError, AppResult};

const JOB_PREFIX: &str = "codex-review-";
const MAX_DELIVERIES: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Target {
    pub thread_id: String,
    pub cwd: PathBuf,
}

impl Target {
    pub fn validate(&self) -> Result<(), String> {
        let id = self.thread_id.as_bytes();
        if id.len() != 36
            || id.iter().enumerate().any(|(i, c)| {
                if [8, 13, 18, 23].contains(&i) {
                    *c != b'-'
                } else {
                    !c.is_ascii_hexdigit()
                }
            })
        {
            return Err("codexReview.threadId 必须是原 Codex 聊天的 UUID".into());
        }
        if !self.cwd.is_absolute() || !self.cwd.is_dir() {
            return Err("codexReview.cwd 必须是原 Codex 聊天的工作目录绝对路径".into());
        }
        Ok(())
    }
}

#[derive(sqlx::FromRow)]
struct Review {
    task_id: i64,
    batch_id: i64,
    sample_job_id: String,
    poster_group_id: String,
    thread_id: String,
    cwd: String,
    decision: Option<String>,
    next_attempt_at: i64,
    attempts: i64,
    delivered_at: Option<i64>,
    last_error: Option<String>,
}

impl Review {
    fn next_job(&self) -> Option<String> {
        Some(format!(
            "{JOB_PREFIX}{}-{}",
            self.task_id,
            self.decision.as_deref()?
        ))
    }

    fn prompt(&self, next_job: &str) -> String {
        let action = if self.decision.as_deref() == Some("pass") {
            "试样已通过。沿用试样实际上传的同一份桌景裁切和布置，按配额直接续作。正式清单的 posterBatch.final 写 true，并省略 codexReview。"
        } else {
            "试样已退回。保留该试样的同一张产品原片，从 GenDesk 换一张本批未试过的桌景，按技能重新取景并只投一张试样。清单的 posterBatch.final 写 false，并保留 codexReview 登记这个原聊天。投单后交 GenDesk 审图。"
        };
        format!(
            concat!(
                "GenDesk 验收事件。请按本聊天的桌面产品场景提示词技能恢复。\n",
                "试样工单 {}，GenDesk 批次 #{}，任务 #{}，原 Poster 批次 {}。\n",
                "{}\n",
                "续作必须使用工单 ID {}。先运行 gendesk status {} 核对。已收录时沿用回执，",
                "中断或失败按原 ID 恢复，待确认按 GenDesk 的既有流程处理。",
                "GenDesk 命令用本轮 gendesk_review 工具提交 args 数组，",
                "例如 [\"status\",\"工单 ID\"]。本轮 shell 用于本地写稿和读图。",
                "新清单必须沿用该 Poster 批次，只处理这条验收结果。",
                "若提交了新试样，由 GenDesk 的下一次验收事件唤醒，结束本轮。"
            ),
            self.sample_job_id,
            self.batch_id,
            self.task_id,
            self.poster_group_id,
            action,
            next_job,
            next_job
        )
    }
}

fn invalid(message: &str) -> AppError {
    AppError::InvalidInput(message.into())
}

/// Only the latest trial can authorize the next intake, using one stable job ID.
pub async fn check(pool: &SqlitePool, plan: &Plan) -> AppResult<()> {
    let Some(group) = &plan.poster_batch else {
        return if plan.job_id.starts_with(JOB_PREFIX) {
            Err(invalid(
                "该工单 ID 由 GenDesk 验收事件保留，必须沿用原 Poster 批次",
            ))
        } else {
            Ok(())
        };
    };
    let current: Option<Review> = sqlx::query_as(
        "SELECT * FROM codex_reviews WHERE poster_group_id=? ORDER BY task_id DESC LIMIT 1",
    )
    .bind(&group.id)
    .fetch_optional(pool)
    .await?;
    let Some(current) = current else {
        if plan.job_id.starts_with(JOB_PREFIX) {
            return Err(invalid("找不到该工单对应的 GenDesk 验收事件"));
        }
        if plan.codex_review.is_some() {
            let exists: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM poster_intake_groups WHERE id=?)")
                    .bind(&group.id)
                    .fetch_one(pool)
                    .await?;
            if exists {
                return Err(invalid("初次登记 codexReview 需要新的 Poster 批次 ID"));
            }
        }
        return Ok(());
    };
    if current.next_job().as_deref() != Some(&plan.job_id) {
        return Err(invalid("请使用本批最新 GenDesk 验收事件指定的续作工单 ID"));
    }
    if current.decision.as_deref() == Some("pass") {
        if !group.final_import || plan.codex_review.is_some() {
            return Err(invalid("试样通过后的正式工单须 final=true，不再登记试样"));
        }
    } else if group.final_import
        || !plan.codex_review.as_ref().is_some_and(|target| {
            target.thread_id == current.thread_id && target.cwd == Path::new(&current.cwd)
        })
    {
        return Err(invalid("试样退回后只接受原聊天登记的下一张试样"));
    }
    Ok(())
}

pub async fn register(
    tx: &mut Transaction<'_, Sqlite>,
    plan: &Plan,
    batch_id: i64,
) -> AppResult<()> {
    if let (Some(target), Some(group)) = (&plan.codex_review, &plan.poster_batch) {
        sqlx::query(
            "INSERT INTO codex_reviews(task_id,batch_id,sample_job_id,poster_group_id,thread_id,cwd)
             SELECT id,batch_id,?1,?2,?3,?4 FROM tasks WHERE batch_id=?5",
        )
        .bind(&plan.job_id)
        .bind(&group.id)
        .bind(&target.thread_id)
        .bind(target.cwd.to_string_lossy().as_ref())
        .bind(batch_id)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

/// Called in the same transaction that records the human's review.
pub async fn decide(
    tx: &mut Transaction<'_, Sqlite>,
    task_id: i64,
    decision: &str,
    at: i64,
) -> AppResult<bool> {
    let result = sqlx::query(
        "UPDATE codex_reviews SET decision=?1,decided_at=?2,next_attempt_at=?2
         WHERE task_id=?3 AND decision IS NULL",
    )
    .bind(decision)
    .bind(at)
    .bind(task_id)
    .execute(&mut **tx)
    .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn status(pool: &SqlitePool, job_id: &str) -> AppResult<serde_json::Value> {
    let row: Option<Review> = sqlx::query_as("SELECT * FROM codex_reviews WHERE sample_job_id=?")
        .bind(job_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map_or(Value::Null, |row| {
        json!({
            "taskId":row.task_id, "batchId":row.batch_id, "decision":row.decision,
            "deliveredAt":row.delivered_at,"attempts":row.attempts,"lastError":row.last_error,
            "nextJobId":row.next_job()
        })
    }))
}

fn executable(name: &str) -> PathBuf {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" });
    if let Some(home) = home {
        let candidate = PathBuf::from(home).join(".local/bin").join(name);
        if candidate.is_file() {
            return candidate;
        }
    }
    ["/opt/homebrew/bin", "/usr/local/bin"]
        .into_iter()
        .map(|dir| Path::new(dir).join(name))
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| PathBuf::from(name))
}

fn review_cli() -> PathBuf {
    #[cfg(debug_assertions)]
    if crate::debug_data_dir_override().is_some() {
        if let Some(cli) = std::env::var_os("GENDESK_REVIEW_CLI") {
            return cli.into();
        }
    }
    executable("gendesk")
}

async fn resume(review: &Review, next_job: &str, binary: &Path) -> AppResult<()> {
    let mut command = tokio::process::Command::new(binary);
    let mut paths = Vec::new();
    if let Some(home) = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }) {
        paths.push(PathBuf::from(home).join(".local/bin"));
    }
    if cfg!(unix) {
        paths.extend([
            PathBuf::from("/opt/homebrew/bin"),
            PathBuf::from("/usr/local/bin"),
        ]);
    }
    if let Some(path) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&path));
    }
    if let Ok(path) = std::env::join_paths(paths) {
        command.env("PATH", path);
    }
    let tools = json!([
        "__review-mcp",
        review.cwd,
        review.sample_job_id,
        next_job,
        review.poster_group_id
    ]);
    let mut child = command
        .arg("-c")
        .arg(format!(
            "mcp_servers.gendesk_review.command={}",
            json!(review_cli())
        ))
        .arg("-c")
        .arg(format!("mcp_servers.gendesk_review.args={tools}"))
        .args(["-c", "mcp_servers.gendesk_review.required=true"])
        .args([
            "-c",
            "mcp_servers.gendesk_review.tools.gendesk.approval_mode=\"approve\"",
        ])
        .arg("-c")
        .arg(format!(
            "mcp_servers.gendesk_review.env.GENDESK_HANDOFF_DIR={}",
            json!(crate::intake::default_root())
        ))
        .args(["app-server", "--listen", "stdio://"])
        .current_dir(&review.cwd)
        .env_remove("CODEX_THREAD_ID")
        .env_remove("CODEX_SESSION_ID")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| invalid("Codex 的输入通道不可用"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| invalid("Codex 的输出通道不可用"))?;
    let mut lines = BufReader::new(stdout).lines();
    rpc(
        &mut stdin,
        &mut lines,
        1,
        "initialize",
        json!({
            "clientInfo":{"name":"gendesk","version":env!("CARGO_PKG_VERSION")}
        }),
    )
    .await?;
    send(&mut stdin, json!({"method":"initialized"})).await?;
    let opened = rpc(
        &mut stdin,
        &mut lines,
        2,
        "thread/resume",
        json!({
            "threadId":review.thread_id,"excludeTurns":true
        }),
    )
    .await?;
    if opened["thread"]["id"].as_str() != Some(&review.thread_id) {
        return Err(invalid("Codex 返回了不同的聊天 ID"));
    }
    if opened["thread"]["status"]["type"] == "active" {
        return Err(invalid("原 Codex 聊天仍在运行，稍后恢复"));
    }
    let started = rpc(
        &mut stdin,
        &mut lines,
        3,
        "turn/start",
        json!({
            "threadId":review.thread_id,"cwd":review.cwd,
        "approvalPolicy":"never",
            "input":[{"type":"text","text":review.prompt(next_job)}]
        }),
    )
    .await?;
    let turn_id = started["turn"]["id"]
        .as_str()
        .ok_or_else(|| invalid("Codex 没有返回续作回合 ID"))?;
    loop {
        let event = receive(&mut lines).await?;
        if event["method"] == "turn/completed"
            && event["params"]["threadId"] == review.thread_id
            && event["params"]["turn"]["id"] == turn_id
        {
            let turn = &event["params"]["turn"];
            drop(stdin);
            if tokio::time::timeout(Duration::from_secs(5), child.wait())
                .await
                .is_err()
            {
                child.kill().await?;
            }
            return if turn["status"] == "completed" {
                Ok(())
            } else {
                Err(AppError::Internal(format!(
                    "Codex 恢复回合未完成：{}",
                    turn["error"]
                )))
            };
        }
    }
}

async fn send(stdin: &mut ChildStdin, value: Value) -> AppResult<()> {
    stdin.write_all(value.to_string().as_bytes()).await?;
    stdin.write_all(b"\n").await?;
    Ok(())
}

async fn receive(lines: &mut Lines<BufReader<ChildStdout>>) -> AppResult<Value> {
    let line = lines
        .next_line()
        .await?
        .ok_or_else(|| invalid("Codex App Server 提前断开"))?;
    Ok(serde_json::from_str(&line)?)
}

async fn rpc(
    stdin: &mut ChildStdin,
    lines: &mut Lines<BufReader<ChildStdout>>,
    id: i64,
    method: &str,
    params: Value,
) -> AppResult<Value> {
    send(stdin, json!({"id":id,"method":method,"params":params})).await?;
    loop {
        let reply = receive(lines).await?;
        if reply["id"] == id {
            if let Some(message) = reply["error"]["message"].as_str() {
                return Err(AppError::Internal(format!("Codex {method}：{message}")));
            }
            return Ok(reply["result"].clone());
        }
    }
}

async fn deliver(pool: &SqlitePool, review: Review, binary: &Path) -> AppResult<()> {
    let Some(job_id) = review.next_job() else {
        return Ok(());
    };
    // A process can die after intake committed but before the Codex turn returned.
    let submitted: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM intake_jobs WHERE job_id=? AND status='done')",
    )
    .bind(&job_id)
    .fetch_one(pool)
    .await?;
    let result = if submitted {
        Ok(())
    } else {
        match tokio::time::timeout(Duration::from_secs(3600), resume(&review, &job_id, binary))
            .await
        {
            Ok(result) => result,
            Err(_) => Err(invalid("Codex 恢复回合超时")),
        }
    };
    if let Err(error) = result {
        let delay = (15_i64 << review.attempts.min(6)).min(900);
        tracing::warn!(task_id = review.task_id, %error, "Codex 验收事件待补投");
        sqlx::query(
            "UPDATE codex_reviews SET attempts=attempts+1,next_attempt_at=?,last_error=? WHERE task_id=?",
        )
        .bind(now_unix() + delay)
        .bind(error.to_string())
        .bind(review.task_id)
        .execute(pool)
        .await?;
    } else {
        sqlx::query(
            "UPDATE codex_reviews SET attempts=attempts+1,delivered_at=?,last_error=NULL WHERE task_id=?",
        )
        .bind(now_unix())
        .bind(review.task_id)
        .execute(pool)
        .await?;
    }
    Ok(())
}

pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let state = app.state::<crate::state::AppState>();
        dispatch_pending(&state.db, &state.codex_review_wake, &executable("codex")).await;
    });
}

async fn dispatch_pending(pool: &SqlitePool, wake: &tokio::sync::Notify, binary: &Path) {
    let mut running = JoinSet::new();
    let mut active = std::collections::HashSet::new();
    loop {
        let mut deadline = None;
        if running.len() < MAX_DELIVERIES {
            let pending: Result<Vec<Review>, _> = sqlx::query_as(
                "SELECT * FROM codex_reviews
                     WHERE decision IS NOT NULL AND delivered_at IS NULL
                     ORDER BY next_attempt_at,task_id",
            )
            .fetch_all(pool)
            .await;
            match pending {
                Ok(events) => {
                    for event in events {
                        if running.len() >= MAX_DELIVERIES {
                            break;
                        }
                        if active.contains(&event.thread_id) {
                            continue;
                        }
                        if event.next_attempt_at > now_unix() {
                            deadline = Some(Duration::from_secs(
                                (event.next_attempt_at - now_unix()).max(0) as u64,
                            ));
                            break;
                        }
                        let thread = event.thread_id.clone();
                        active.insert(thread.clone());
                        let pool = pool.clone();
                        let binary = binary.to_path_buf();
                        running
                            .spawn(async move { (thread, deliver(&pool, event, &binary).await) });
                    }
                }
                Err(error) => {
                    tracing::warn!(%error, "读取待投递的 Codex 验收事件失败");
                    deadline = Some(Duration::from_secs(30));
                }
            }
        }
        let timer = async {
            match deadline {
                Some(delay) => tokio::time::sleep(delay).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            _ = wake.notified() => {}
            _ = timer => {}
            result = running.join_next(), if !running.is_empty() => {
                match result {
                    Some(Ok((thread, result))) => {
                        active.remove(&thread);
                        if let Err(error) = result {
                            tracing::warn!(%error, "记录 Codex 验收事件投递失败");
                            tokio::time::sleep(Duration::from_secs(5)).await;
                        }
                    }
                    Some(Err(error)) => {
                        tracing::error!(%error, "Codex 验收投递任务意外终止");
                        running.abort_all();
                        while running.join_next().await.is_some() {}
                        active.clear();
                    }
                    None => {}
                }
            }
        }
    }
}

#[cfg(all(test, unix))]
#[allow(clippy::unwrap_used, clippy::expect_used)] // Test assertions may fail with fixture details.
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;

    #[tokio::test]
    async fn pending_delivery_restarts_and_completed_intake_does_not_launch_codex_twice() {
        let data = tempfile::tempdir().unwrap();
        let db_file = data.path().join("gendesk.db");
        let pool = crate::db::connect(&db_file).await.unwrap();
        let cwd = data.path().to_str().unwrap();
        sqlx::query(
            "INSERT INTO codex_reviews(task_id,batch_id,sample_job_id,poster_group_id,thread_id,cwd)
             VALUES(1,1,'trial','poster','0194388e-f401-7f64-b86b-aac993e27140',?)",
        )
        .bind(cwd)
        .execute(&pool)
        .await
        .unwrap();
        let mut tx = pool.begin().await.unwrap();
        assert!(decide(&mut tx, 1, "reject", now_unix()).await.unwrap());
        tx.commit().await.unwrap();

        let first: Review = sqlx::query_as("SELECT * FROM codex_reviews")
            .fetch_one(&pool)
            .await
            .unwrap();
        deliver(&pool, first, &data.path().join("missing-codex"))
            .await
            .unwrap();
        let before = status(&pool, "trial").await.unwrap();
        assert!(before["deliveredAt"].is_null());
        assert_eq!(before["attempts"], 1);
        assert!(before["lastError"].is_string());
        pool.close().await;

        let pool = crate::db::connect(&db_file).await.unwrap();
        let bin = data.path().join("fake-codex");
        std::fs::write(&bin, r#"#!/bin/sh
read -r init
printf '%s\n' '{"id":1,"result":{}}'
read -r initialized
read -r resume
case "$resume" in *0194388e-f401-7f64-b86b-aac993e27140*) ;; *) exit 4 ;; esac
printf '%s\n' '{"id":2,"result":{"thread":{"id":"0194388e-f401-7f64-b86b-aac993e27140"}}}'
read -r start
case "$start" in *codex-review-1-reject*) ;; *) exit 5 ;; esac
printf '%s\n' '{"id":3,"result":{"turn":{"id":"1"}}}'
printf '%s\n' '{"method":"turn/completed","params":{"threadId":"0194388e-f401-7f64-b86b-aac993e27140","turn":{"id":"1","status":"completed"}}}'
printf '%s\n' sent >> delivered.txt
"#).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o700)).unwrap();
        sqlx::query("UPDATE codex_reviews SET next_attempt_at=0")
            .execute(&pool)
            .await
            .unwrap();
        let worker_pool = pool.clone();
        let worker = tokio::spawn(async move {
            dispatch_pending(&worker_pool, &tokio::sync::Notify::new(), &bin).await;
        });
        for _ in 0..100 {
            if status(&pool, "trial").await.unwrap()["deliveredAt"].is_number() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        worker.abort();
        assert_eq!(status(&pool, "trial").await.unwrap()["attempts"], 2);
        assert!(status(&pool, "trial").await.unwrap()["deliveredAt"].is_number());
        assert_eq!(
            std::fs::read_to_string(data.path().join("delivered.txt")).unwrap(),
            "sent\n"
        );

        // Simulate a process that died after the next intake committed and before marking delivery.
        sqlx::query("UPDATE codex_reviews SET delivered_at=NULL")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO intake_jobs(job_id,dir_name,status,created_at,updated_at)
             VALUES('codex-review-1-reject','next','done',0,0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let recovered: Review = sqlx::query_as("SELECT * FROM codex_reviews")
            .fetch_one(&pool)
            .await
            .unwrap();
        deliver(&pool, recovered, &data.path().join("missing-codex"))
            .await
            .unwrap();
        let after = status(&pool, "trial").await.unwrap();
        assert!(after["deliveredAt"].is_number());
        assert!(after["lastError"].is_null());
    }
}
