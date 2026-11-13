//! 生图工单收件：CLI 经控制通道提交、确认、重试，应用串行收录并发出通知。
//!
//! 没有设置项、没有目录监听：交接根固定为 `~/GenDesk交接`，阈值固定为
//! [`TASK_THRESHOLD`]。界面只在收录发生时弹通知与 toast。

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, Manager};
use tauri_specta::Event;

use crate::error::AppResult;
use crate::intake::{self, ingest::Ctx, JobView};
use crate::state::AppState;

/// 自动开跑阈值：任务数（= 出图张数）超过它就转「待确认」，等 `gendesk confirm`。
///
/// 判定放在 Rust 而不是投单那一侧：投单的是另一个模型，它可以忘记检查、也可以
/// 被绕过；而「超过多少张就得问一句」是花钱的闸门，必须是机制而不是自觉。
pub const TASK_THRESHOLD: i64 = 100;

/// `intake://changed` —— 有工单被收录、转待确认或失败。
///
/// 收录由 Agent 触发，用户没有按任何按钮，所以它**尤其**需要出声：
/// 一个批次凭空出现在任务页，和一份工单静默失败，都需要一句解释。
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct IntakeChanged {
    pub jobs: Vec<JobView>,
}

/// CLI 提交一份已写好 READY 的工单。
pub async fn submit(app: &AppHandle, dir_name: &str) -> AppResult<Option<JobView>> {
    let state = app.state::<AppState>();
    let job = intake::ingest::submit(&ctx_of(&state), &intake::default_root(), dir_name).await?;
    emit(app, job.as_slice());
    Ok(job)
}

/// CLI 放行超阈值工单（用户已明确同意）。
pub async fn confirm(app: &AppHandle, job_id: &str) -> AppResult<Option<JobView>> {
    let state = app.state::<AppState>();
    let job = intake::ingest::confirm(&ctx_of(&state), &intake::default_root(), job_id).await?;
    emit(app, job.as_slice());
    Ok(job)
}

/// CLI 重试失败或中断的工单：沿原台账和持久检查点恢复，只补未完成步骤。
pub async fn retry(app: &AppHandle, job_id: &str) -> AppResult<Option<JobView>> {
    let state = app.state::<AppState>();
    let job = intake::ingest::retry(&ctx_of(&state), &intake::default_root(), job_id).await?;
    emit(app, job.as_slice());
    Ok(job)
}

fn emit(app: &AppHandle, jobs: &[JobView]) {
    if jobs.is_empty() {
        return;
    }
    for j in jobs {
        notify_job(app, j);
    }
    let _ = IntakeChanged {
        jobs: jobs.to_vec(),
    }
    .emit(app);
}

/// 系统通知（macOS 通知中心 / Windows 操作中心）。
///
/// 投单那一刻人在 Agent 里，GenDesk 可能只是个后台图标——应用内的 toast 看不到。
/// 走 `TauriSink::notify`：系统通知只该有一条发送路径。
fn notify_job(app: &AppHandle, j: &JobView) {
    use crate::engine::events::EventSink;
    let sink = crate::engine::events::TauriSink::new(app.clone());
    let (title, body) = match j.status.as_str() {
        "done" => {
            let b = j
                .batch_ids
                .iter()
                .map(|x| format!("#{x}"))
                .collect::<Vec<_>>()
                .join(" ");
            (
                "工单已开跑".to_string(),
                format!("{} · 批次 {b} · {} 张", j.job_id, j.task_count),
            )
        }
        "hold" => (
            "工单待确认".to_string(),
            format!("{} · {}，需用户同意后 gendesk confirm", j.job_id, j.message),
        ),
        _ => (
            "工单收录失败".to_string(),
            format!("{} · {}", j.job_id, j.message),
        ),
    };
    sink.notify(title, body);
}

/// 投单时按当前绑定与 Poster 商品目录检查归属。读不到 Poster（未安装、接口超时）时
/// 只记日志放行：那是暂时状态，审稿后的导入会自行重试；只有确定对不上的款式才拦。
struct PosterCheck {
    pool: sqlx::SqlitePool,
}

#[async_trait::async_trait]
impl intake::ingest::PosterGate for PosterCheck {
    async fn unresolved(&self, keys: &[(String, String)]) -> Vec<String> {
        match crate::poster::unresolved(&self.pool, keys).await {
            Ok(missing) => missing,
            Err(error) => {
                tracing::warn!(%error, "投单时读不到 Poster 商品，跳过归属检查");
                Vec::new()
            }
        }
    }

    async fn unresolved_types(&self, labels: &[String]) -> Vec<String> {
        match crate::poster::unresolved_types(&self.pool, labels).await {
            Ok(missing) => missing,
            Err(error) => {
                tracing::warn!(%error, "投单时读不到 Poster 图片类型，跳过类型检查");
                Vec::new()
            }
        }
    }
}

fn ctx_of(state: &AppState) -> Ctx {
    Ctx {
        pool: state.db.clone(),
        dirs: state.dirs.clone(),
        engine: state.engine.clone(),
        progress: Arc::new(intake::ingest::NoProgress),
        threshold: TASK_THRESHOLD,
        poster: Arc::new(PosterCheck {
            pool: state.db.clone(),
        }),
        // **必须是 AppState 里那一把**：并发的 CLI 调用各自构造 Ctx，各拿一把新锁就等于没有锁。
        lock: state.intake_lock.clone(),
    }
}

/// 设置页「Agent CLI」分区显示的连接信息。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AgentCliInfo {
    /// 交接根（工单包、回执、连接文件都在这里）。
    pub root: String,
    /// 控制通道是否在监听；false 时 CLI 连不上，需看日志。
    pub listening: bool,
    pub protocol: i64,
    pub features: Vec<String>,
    pub task_threshold: i64,
}

#[tauri::command]
#[specta::specta]
pub async fn get_agent_cli_info() -> AppResult<AgentCliInfo> {
    Ok(AgentCliInfo {
        root: intake::default_root().to_string_lossy().into_owned(),
        listening: crate::control::listening_port().is_some(),
        protocol: crate::control::PROTOCOL,
        features: crate::control::FEATURES
            .iter()
            .map(|f| f.to_string())
            .collect(),
        task_threshold: TASK_THRESHOLD,
    })
}

/// 在 Finder / 资源管理器里打开交接根。
#[tauri::command]
#[specta::specta]
pub async fn open_agent_root(app: AppHandle) -> AppResult<()> {
    use tauri_plugin_opener::OpenerExt;
    let root = intake::default_root();
    std::fs::create_dir_all(&root)?;
    app.opener()
        .open_path(root.to_string_lossy().as_ref(), None::<&str>)
        .map_err(|e| crate::error::AppError::Io(e.to_string()))
}
