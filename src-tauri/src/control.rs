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
    "reference-library",
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
        "intake.submit" => {
            let job = intake::submit(app, arg(&args, "dir")?).await?;
            with_picks(&state, json!({ "job": job })).await
        }
        "intake.confirm" => {
            let job = intake::confirm(app, arg(&args, "jobId")?).await?;
            with_picks(&state, json!({ "job": job })).await
        }
        "intake.retry" => {
            let job = intake::retry(app, arg(&args, "jobId")?).await?;
            with_picks(&state, json!({ "job": job })).await
        }
        "intake.status" => {
            let job =
                crate::db::repo::intake::get_by_job_id(&state.db, arg(&args, "jobId")?).await?;
            with_picks(&state, json!({ "job": job })).await
        }
        library if library.starts_with("library.") => {
            library_command(app, &state, library, &args).await
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

/// 工单回执附上参考图库抽到的图（没有图库引用的单不加这个字段）。
async fn with_picks(state: &AppState, mut reply: Value) -> AppResult<Value> {
    let job_id = reply
        .pointer("/job/jobId")
        .and_then(Value::as_str)
        .map(str::to_string);
    if let Some(job_id) = job_id {
        let picks = crate::ref_library::picks_for_job(&state.db, &job_id).await?;
        if !picks.is_empty() {
            reply["libraryPicks"] = serde_json::to_value(picks)?;
        }
    }
    Ok(reply)
}

fn opt_text<'a>(args: &'a Value, key: &str) -> &'a str {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
}

fn ids(args: &Value) -> AppResult<Vec<i64>> {
    let list = args
        .get("ids")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::InvalidInput("缺少参数 ids".into()))?;
    list.iter()
        .map(|v| {
            v.as_i64()
                .filter(|n| *n > 0)
                .ok_or_else(|| AppError::InvalidInput(format!("图片编号不对：{v}")))
        })
        .collect()
}

/// 参考图库的 CLI 命令。名字在这里解析成编号，规则全在 `ref_library`。
async fn library_command(
    app: &AppHandle,
    state: &AppState,
    cmd: &str,
    args: &Value,
) -> AppResult<Value> {
    use crate::ref_library as lib;
    let db = &state.db;
    let now = crate::db::now_unix();
    let style = |required: bool| async move {
        let name = opt_text(args, "style");
        if name.is_empty() {
            return if required {
                Err(AppError::InvalidInput("缺少参数 style".into()))
            } else {
                Ok(None)
            };
        }
        lib::find_style(db, name, opt_text(args, "product"))
            .await
            .map(Some)
    };
    let category = |required: bool| async move {
        let name = opt_text(args, "category");
        if name.is_empty() {
            return if required {
                Err(AppError::InvalidInput("缺少参数 category".into()))
            } else {
                Ok(None)
            };
        }
        lib::find_category(db, name).await.map(Some)
    };
    let required = |v: Option<i64>, key: &str| {
        v.ok_or_else(|| AppError::InvalidInput(format!("缺少参数 {key}")))
    };
    match cmd {
        "library.overview" => Ok(serde_json::to_value(lib::overview(db, now).await?)?),
        "library.images" => {
            let style_id = style(false).await?.map(|s| s.id);
            let category_id = category(false).await?.map(|c| c.id);
            let include = args
                .get("includeRemoved")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let images = lib::list_images(db, style_id, category_id, include, now).await?;
            Ok(json!({ "images": images }))
        }
        "library.style.add" => {
            let id = lib::save_style(
                db,
                None,
                arg(args, "name")?,
                opt_text(args, "product"),
                opt_text(args, "note"),
            )
            .await?;
            Ok(json!({ "id": id }))
        }
        "library.style.rename" => {
            let found = style(true).await?;
            let found = found.ok_or_else(|| AppError::InvalidInput("缺少参数 style".into()))?;
            let product = args
                .get("newProduct")
                .and_then(Value::as_str)
                .unwrap_or(&found.product);
            let id = lib::save_style(db, Some(found.id), arg(args, "name")?, product, &found.note)
                .await?;
            Ok(json!({ "id": id }))
        }
        "library.category.add" | "library.category.set" => {
            let name = arg(args, "name")?;
            let cooldown = args.get("cooldownDays").and_then(Value::as_i64);
            let id = if cmd.ends_with(".add") {
                lib::save_category(db, None, name, cooldown.unwrap_or(7), None).await?
            } else {
                let found = lib::find_category(db, name).await?;
                let rename = opt_text(args, "rename");
                lib::save_category(
                    db,
                    Some(found.id),
                    if rename.is_empty() { name } else { rename },
                    cooldown.unwrap_or(found.cooldown_days),
                    None,
                )
                .await?
            };
            Ok(json!({ "id": id }))
        }
        "library.import" => {
            let style_id = required(style(true).await?.map(|s| s.id), "style")?;
            let category_id = required(category(true).await?.map(|c| c.id), "category")?;
            let paths: Vec<String> = args
                .get("paths")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            if paths.is_empty() {
                return Err(AppError::InvalidInput("没有要导入的图片".into()));
            }
            let emitter = app.clone();
            let progress = move |done: i64, total: i64, name: &str, failed: i64| {
                crate::commands::refs::emit_progress(
                    &emitter, done, total, name, "running", failed,
                );
            };
            let report =
                lib::import_files(db, &state.dirs, style_id, category_id, &paths, &progress)
                    .await?;
            crate::commands::refs::emit_progress(
                app,
                paths.len() as i64,
                paths.len() as i64,
                "",
                "done",
                report.failed.len() as i64,
            );
            Ok(serde_json::to_value(report)?)
        }
        "library.pick" => {
            let style_id = required(style(true).await?.map(|s| s.id), "style")?;
            let category_id = required(category(true).await?.map(|c| c.id), "category")?;
            let count = args.get("count").and_then(Value::as_u64).unwrap_or(1) as usize;
            let mark = args
                .get("markUsed")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let result = lib::pick(
                db,
                style_id,
                category_id,
                count,
                mark,
                now,
                &mut lib::os_rng,
            )
            .await?;
            Ok(serde_json::to_value(result)?)
        }
        "library.update" => {
            let category_id = category(false).await?.map(|c| c.id);
            let patch = lib::LibraryImagePatch {
                category_id,
                enabled: args.get("enabled").and_then(Value::as_bool),
                note: args.get("note").and_then(Value::as_str).map(str::to_string),
                removed: args.get("removed").and_then(Value::as_bool),
            };
            Ok(json!({ "changed": lib::update_images(db, &ids(args)?, &patch).await? }))
        }
        "library.mark-used" => Ok(json!({ "marked": lib::mark_used(db, &ids(args)?, now).await? })),
        "library.check" => {
            let slots: Vec<lib::LibrarySlot> =
                serde_json::from_value(args.get("slots").cloned().unwrap_or(Value::Null))
                    .map_err(|e| AppError::InvalidInput(format!("图库引用格式不对：{e}")))?;
            Ok(json!({ "slots": lib::check_slots(db, &slots).await? }))
        }
        "library.usage" => {
            if let Some(id) = args.get("id").and_then(Value::as_i64) {
                return Ok(json!({ "usage": lib::usage_of_image(db, id).await? }));
            }
            Ok(json!({ "picks": lib::picks_for_job(db, arg(args, "jobId")?).await? }))
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
