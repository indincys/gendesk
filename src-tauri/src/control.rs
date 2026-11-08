//! Agent CLI 控制通道。
//!
//! 应用启动时在 `127.0.0.1` 的随机端口监听，把端口与本次启动的随机令牌写进
//! `~/GenDesk交接/连接.json`（仅当前用户可读写）。`scripts/gendesk.mjs` 读它、连上来，
//! 一次连接 = 一行 JSON 请求 + 一行 JSON 响应。
//!
//! 这是外部 Agent 进入 GenDesk 的唯一入口：没有目录监听、没有启动补扫。
//! 业务仍全部在 Rust：CLI 只投递已打包的工单目录名、jobId 或改写行，
//! 收录、建批、调度与审稿都由应用内的同一套代码完成。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// 控制协议版本。CLI 与应用不一致时直接拒绝，不尝试兼容旧文件信箱。
pub const PROTOCOL: i64 = 2;

/// 应用能力声明，`gendesk doctor` 原样展示。
pub const FEATURES: &[&str] = &[
    "ordered-references",
    "generation-quality",
    "content-organization",
    "intake-preflight",
    "resumable-intake",
    "background-launch",
    "v2v-rewrite",
];

/// 单条请求上限。改写结果按行携带全文，几百条也远小于此。
const MAX_REQUEST: u64 = 16 * 1024 * 1024;

#[derive(Debug, Deserialize)]
struct Request {
    token: String,
    cmd: String,
    #[serde(default)]
    args: Value,
}

#[derive(Debug, Serialize)]
struct Response {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// 本次启动实际监听的端口；监听失败时为空（设置页据此显示状态）。
static LISTENING: std::sync::OnceLock<u16> = std::sync::OnceLock::new();

/// CLI 控制通道是否已就绪。
pub fn listening_port() -> Option<u16> {
    LISTENING.get().copied()
}

/// 连接文件位置（固定，CLI 写死）。
pub fn connection_path() -> PathBuf {
    crate::intake::default_root().join("连接.json")
}

/// 启动监听并写连接文件。失败只影响 CLI，不阻断应用启动。
pub fn start(app: AppHandle, publish: bool) -> AppResult<()> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    listener.set_nonblocking(true)?;
    let port = listener.local_addr()?.port();
    let token = crate::poster::new_id();
    if publish {
        write_connection(port, &token)?;
    }
    let _ = LISTENING.set(port);
    tauri::async_runtime::spawn(async move {
        let listener = match tokio::net::TcpListener::from_std(listener) {
            Ok(l) => l,
            Err(error) => {
                tracing::error!(%error, "CLI 控制通道监听失败");
                return;
            }
        };
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let app = app.clone();
                    let token = token.clone();
                    tauri::async_runtime::spawn(async move {
                        if let Err(error) = serve(stream, &app, &token).await {
                            tracing::warn!(%error, "CLI 请求处理失败");
                        }
                    });
                }
                Err(error) => {
                    tracing::warn!(%error, "CLI 连接接受失败");
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                }
            }
        }
    });
    Ok(())
}

fn write_connection(port: u16, token: &str) -> AppResult<()> {
    let value = json!({
        "version": 2,
        "protocol": PROTOCOL,
        "appVersion": env!("CARGO_PKG_VERSION"),
        "features": FEATURES,
        "port": port,
        "token": token,
        "pid": std::process::id(),
        "appPath": std::env::current_exe().ok(),
        "root": crate::intake::default_root(),
        "taskThreshold": crate::commands::intake::TASK_THRESHOLD,
    });
    let path = connection_path();
    crate::intake::write_json(&path, &value)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

async fn serve(stream: tokio::net::TcpStream, app: &AppHandle, token: &str) -> AppResult<()> {
    let (read, mut write) = stream.into_split();
    let mut line = String::new();
    BufReader::new(read.take(MAX_REQUEST))
        .read_line(&mut line)
        .await?;
    let response = match serde_json::from_str::<Request>(line.trim()) {
        Err(e) => failure(format!("请求不是有效 JSON：{e}")),
        Ok(req) if req.token != token => failure("令牌不匹配，请重新读取连接文件".into()),
        Ok(req) => match dispatch(app, &req.cmd, req.args).await {
            Ok(data) => Response {
                ok: true,
                data: Some(data),
                error: None,
            },
            Err(e) => failure(e.to_string()),
        },
    };
    let mut body = serde_json::to_vec(&response)?;
    body.push(b'\n');
    write.write_all(&body).await?;
    write.shutdown().await?;
    Ok(())
}

fn failure(error: String) -> Response {
    Response {
        ok: false,
        data: None,
        error: Some(error),
    }
}

fn arg<'a>(args: &'a Value, key: &str) -> AppResult<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::InvalidInput(format!("缺少参数 {key}")))
}

async fn dispatch(app: &AppHandle, cmd: &str, args: Value) -> AppResult<Value> {
    use crate::commands::intake;
    let state = app.state::<AppState>();
    match cmd {
        "hello" => Ok(json!({
            "protocol": PROTOCOL,
            "appVersion": env!("CARGO_PKG_VERSION"),
            "features": FEATURES,
            "pid": std::process::id(),
            "root": crate::intake::default_root(),
            "taskThreshold": intake::TASK_THRESHOLD,
        })),
        "intake.submit" => Ok(json!({ "job": intake::submit(app, arg(&args, "dir")?).await? })),
        "intake.confirm" => Ok(json!({ "job": intake::confirm(app, arg(&args, "jobId")?).await? })),
        "intake.retry" => Ok(json!({ "job": intake::retry(app, arg(&args, "jobId")?).await? })),
        "intake.status" => {
            let job =
                crate::db::repo::intake::get_by_job_id(&state.db, arg(&args, "jobId")?).await?;
            Ok(json!({ "job": job }))
        }
        "overview" => overview(&state).await,
        "v2v.pending" => Ok(serde_json::to_value(
            crate::v2v::rewrite::pending(&state.db).await?,
        )?),
        "v2v.rewrite" => {
            let lines: Vec<crate::v2v::rewrite::RewriteLine> =
                serde_json::from_value(args.get("lines").cloned().unwrap_or(Value::Null))
                    .map_err(|e| AppError::InvalidInput(format!("改写结果格式不对：{e}")))?;
            if lines.is_empty() {
                return Err(AppError::InvalidInput("没有改写结果".into()));
            }
            let sum = crate::commands::v2v::apply_rewrites(&state, app, &lines).await?;
            Ok(serde_json::to_value(sum)?)
        }
        other => Err(AppError::InvalidInput(format!("未知命令：{other}"))),
    }
}

/// 生产概览：任务各态数量与视频待办，供 Agent 判断「跑完没有、该不该提醒人审稿」。
async fn overview(state: &AppState) -> AppResult<Value> {
    let rows: Vec<(String, i64)> =
        sqlx::query_as("SELECT status, COUNT(*) FROM tasks GROUP BY status")
            .fetch_all(&state.db)
            .await?;
    let count = |names: &[&str]| -> i64 {
        rows.iter()
            .filter(|(s, _)| names.contains(&s.as_str()))
            .map(|(_, n)| n)
            .sum()
    };
    let v2v = crate::v2v::runner::counts(&state.db).await?;
    Ok(json!({
        "tasks": {
            "queued": count(&["q"]),
            "running": count(&["run", "retry"]),
            "review": count(&["rev"]),
            "failed": count(&["fail"]),
        },
        "v2v": v2v,
        "paused": state.engine.is_paused(),
    }))
}
