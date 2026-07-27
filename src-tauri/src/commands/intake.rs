//! 生图工单收件域命令：设置、工单台账、手动扫描/重试。
//!
//! 收录本身是**自动**的（watcher + 启动补跑），这里的命令只回答两件事：
//! 「投单该往哪个目录放」与「刚才那几份工单怎么样了」。

use serde::{Deserialize, Serialize};
use specta::Type;
use sqlx::SqlitePool;
use tauri::{AppHandle, State};
use tauri_specta::Event;

use crate::db::repo::intake as repo;
use crate::db::repo::settings as settings_repo;
use crate::error::{AppError, AppResult};
use crate::intake::{self, ingest::Ctx, JobView};
use crate::state::AppState;

const KEY: &str = "intake";

/// `intake://changed` —— 一轮扫描处理掉了工单（跳过的不发）。
///
/// 收录是自动发生的，用户没有按任何按钮，所以它**尤其**需要出声：
/// 一个批次凭空出现在任务页，和一份工单静默失败，都需要一句解释。
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct IntakeChanged {
    pub jobs: Vec<JobView>,
}

/// 收件设置（`settings` 表 key='intake' 单行 JSON）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct IntakeSettings {
    /// 关掉后不再监听、不再自动收录（排查问题或暂时不想让它自己跑时用）。
    #[serde(default = "d_true")]
    pub enabled: bool,
    /// 交接根目录。skill 侧把它写死才能做到「什么都不用输入」，故默认值必须可预测。
    #[serde(default = "d_root")]
    pub root: String,
    /// 自动开跑阈值：任务数（= 出图张数）超过它就转「待确认」。`0` = 不限。
    ///
    /// 判定放在 Rust 而不是投单那一侧：投单的是另一个模型，它可以忘记检查、也可以
    /// 被绕过；而「超过多少张就得问一句」是花钱的闸门，必须是机制而不是自觉。
    #[serde(default = "d_threshold")]
    pub task_threshold: i64,
}

fn d_true() -> bool {
    true
}
fn d_root() -> String {
    intake::default_root().to_string_lossy().to_string()
}
fn d_threshold() -> i64 {
    500
}

impl Default for IntakeSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            root: d_root(),
            task_threshold: d_threshold(),
        }
    }
}

impl IntakeSettings {
    pub fn root_path(&self) -> std::path::PathBuf {
        if self.root.trim().is_empty() {
            intake::default_root()
        } else {
            std::path::PathBuf::from(self.root.trim())
        }
    }
}

/// 读设置（缺失/损坏都回默认值，绝不让整页打不开）。
pub async fn load_settings(pool: &SqlitePool) -> AppResult<IntakeSettings> {
    let raw = settings_repo::get_by_key(pool, KEY).await?;
    Ok(raw
        .and_then(|j| serde_json::from_str::<IntakeSettings>(&j).ok())
        .unwrap_or_default())
}

#[tauri::command]
#[specta::specta]
pub async fn get_intake_settings(state: State<'_, AppState>) -> AppResult<IntakeSettings> {
    load_settings(&state.db).await
}

#[tauri::command]
#[specta::specta]
pub async fn update_intake_settings(
    state: State<'_, AppState>,
    app: AppHandle,
    settings: IntakeSettings,
) -> AppResult<IntakeSettings> {
    let json = serde_json::to_string(&settings)
        .map_err(|e| AppError::Internal(format!("设置序列化失败：{e}")))?;
    settings_repo::set_by_key(&state.db, KEY, &json).await?;
    // 目录可能被改到别处 / 刚打开开关 / 刚调高阈值 → 立刻在新位置按新阈值扫一次，
    // 别让用户干等下一次事件。
    if settings.enabled {
        let ctx = ctx_of(&state, settings.task_threshold);
        scan_and_emit(&ctx, &settings.root_path(), &app).await;
    }
    load_settings(&state.db).await
}

/// 收件目录的绝对路径（设置页直接显示「skill 该往这里投单」）。
#[tauri::command]
#[specta::specta]
pub async fn intake_pending_dir(state: State<'_, AppState>) -> AppResult<String> {
    let s = load_settings(&state.db).await?;
    Ok(intake::pending_dir(&s.root_path())
        .to_string_lossy()
        .to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn list_intake_jobs(state: State<'_, AppState>, limit: i64) -> AppResult<Vec<JobView>> {
    Ok(repo::list_recent(&state.db, limit.clamp(1, 200)).await?)
}

/// 手动扫一次（刚投完单不想等 watcher，或 watcher 没起来时的兜底）。
#[tauri::command]
#[specta::specta]
pub async fn scan_intake_now(
    state: State<'_, AppState>,
    app: AppHandle,
) -> AppResult<Vec<JobView>> {
    let s = load_settings(&state.db).await?;
    let ctx = ctx_of(&state, s.task_threshold);
    let jobs = intake::ingest::scan(&ctx, &s.root_path()).await?;
    emit(&app, &jobs);
    Ok(jobs)
}

/// 重试：删掉台账那行，让下一次扫描重新收录它。
///
/// **只对失败的工单开放**：成功的工单目录已经移走了，删掉记录不会让它重跑，
/// 只会让台账少一行历史。
#[tauri::command]
#[specta::specta]
pub async fn retry_intake_job(
    state: State<'_, AppState>,
    app: AppHandle,
    id: i64,
) -> AppResult<Vec<JobView>> {
    let job = repo::get(&state.db, id).await?;
    if job.status == "done" {
        return Err(AppError::InvalidInput(
            "这份工单已经成功建批，无需重试；要再跑一次请重新投单".into(),
        ));
    }
    repo::delete(&state.db, id).await?;
    scan_intake_now(state, app).await
}

// ───────────────────────── 开跑前的可视化确认 ─────────────────────────

/// 一份待确认工单的完整对应关系（提示词组 ↔ 参考图 ↔ 参数）。
///
/// **存在的理由**：超阈值的工单是自动收录链路上唯一一处「停下来等人点头」的地方，
/// 而在此之前那句「XX 张，去设置页确认」并不足以让人做出判断 —— 真正要看的是
/// **哪个组配了哪几张图**。配错的代价是整批图跑出来全是错的，且要到验收时才发现，
/// 那时钱已经花完了。所以这里给的是生成页那张「已经挂好靠」的图，而不是一个数字。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct JobPreview {
    pub id: i64,
    pub job_id: String,
    pub dir_name: String,
    /// 工单目录绝对路径（「在访达里打开」用）。
    pub dir: String,
    pub groups: Vec<JobPreviewGroup>,
    pub task_count: i64,
    pub batch_count: i64,
    /// 当前阈值（前端说明「超过它才要确认」）。
    pub threshold: i64,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct JobPreviewGroup {
    pub name: String,
    pub prefix: Option<String>,
    pub purposes: Vec<String>,
    /// 本组全部提示词正文（人要能逐条读，不只是数一个条数）。
    pub prompts: Vec<String>,
    /// 挂靠到本组的参考图。
    pub refs: Vec<JobPreviewRef>,
    /// 本组生效的参数快照与实际进 multipart 的字段。
    pub params_json: String,
    pub wire_json: String,
    pub draws: i64,
    pub task_count: i64,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct JobPreviewRef {
    pub file_name: String,
    /// 内联 data: URI 缩略图。
    ///
    /// **不能走 asset 协议**：它的 scope 限定在 `$APPDATA/$APPLOCALDATA/$PICTURE`，
    /// 而工单目录在交接根下（默认 `~/GenDesk交接/`）。为了给一张预览图去放宽
    /// 整个应用的文件读取范围，代价与收益完全不成比例。
    pub thumb_data_uri: Option<String>,
}

/// 预览一份待确认工单。**只读**：与真正收录走的是同一个 `intake::plan`，
/// 故这里看见的对应关系就是确认之后会发生的那一份，不存在两套解析各说各话。
#[tauri::command]
#[specta::specta]
pub async fn preview_intake_job(state: State<'_, AppState>, id: i64) -> AppResult<JobPreview> {
    let job = repo::get(&state.db, id).await?;
    let s = load_settings(&state.db).await?;
    let dir = intake::pending_dir(&s.root_path()).join(&job.dir_name);
    if !dir.is_dir() {
        return Err(AppError::InvalidInput(format!(
            "工单目录已不在：{}",
            dir.display()
        )));
    }
    // 解析 + 读图都是纯 IO/CPU，别占着 IPC 的异步执行器。
    let threshold = s.task_threshold;
    let job_id = job.job_id.clone();
    let dir_name = job.dir_name.clone();
    tokio::task::spawn_blocking(move || build_preview(id, &job_id, &dir_name, &dir, threshold))
        .await
        .map_err(|e| AppError::Internal(format!("预览工单失败：{e}")))?
}

fn build_preview(
    id: i64,
    job_id: &str,
    dir_name: &str,
    dir: &std::path::Path,
    threshold: i64,
) -> AppResult<JobPreview> {
    let plan = intake::plan(dir, dir_name).map_err(AppError::InvalidInput)?;
    let groups = plan
        .groups
        .iter()
        .map(|g| JobPreviewGroup {
            name: g.parsed.name.clone(),
            prefix: g.parsed.prefix.clone(),
            purposes: g
                .parsed
                .tags
                .iter()
                .filter(|t| crate::purpose::is_purpose(t))
                .cloned()
                .collect(),
            prompts: g.parsed.prompts.iter().map(|p| p.text.clone()).collect(),
            refs: g
                .refs
                .iter()
                .map(|p| JobPreviewRef {
                    file_name: p
                        .file_name()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_default(),
                    thumb_data_uri: preview_thumb(p),
                })
                .collect(),
            params_json: g.params_json.clone(),
            wire_json: g.wire_json.clone(),
            draws: g.draws,
            task_count: g.task_count(),
        })
        .collect();
    Ok(JobPreview {
        id,
        job_id: job_id.to_string(),
        dir_name: dir_name.to_string(),
        dir: dir.to_string_lossy().to_string(),
        groups,
        task_count: plan.task_count(),
        batch_count: plan.batch_count() as i64,
        threshold,
    })
}

/// 预览缩略图长边像素。够看清「是不是这张图」，又不至于让一份 30 张图的工单
/// 把几十 MB base64 塞进一次 IPC 回包。
const PREVIEW_EDGE: u32 = 240;

/// 参考图 → `data:image/jpeg;base64,…`。读不出来就返回 None（前端画占位框）——
/// 一张图预览失败不该让整份确认卡打不开。
fn preview_thumb(path: &std::path::Path) -> Option<String> {
    use base64::Engine as _;
    use image::codecs::jpeg::JpegEncoder;

    let img = image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?
        .thumbnail(PREVIEW_EDGE, PREVIEW_EDGE)
        .to_rgb8();
    let mut buf: Vec<u8> = Vec::new();
    JpegEncoder::new_with_quality(&mut buf, 72)
        .encode(
            &img,
            img.width(),
            img.height(),
            image::ExtendedColorType::Rgb8,
        )
        .ok()?;
    Some(format!(
        "data:image/jpeg;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&buf)
    ))
}

/// 确认开跑一份超阈值的工单。
///
/// 做的事就两件：**在工单目录里写下 `确认.txt`**，然后删掉台账那行让它重新收录。
/// 之所以走文件而不是在库里加一个「已确认」标志位——`确认.txt` 是确认的**唯一表达**，
/// 你在 Claude Code 里 `touch` 一下和在这里点一下走的是同一段代码，
/// 不可能出现「一条路对、另一条路错」。
#[tauri::command]
#[specta::specta]
pub async fn confirm_intake_job(
    state: State<'_, AppState>,
    app: AppHandle,
    id: i64,
) -> AppResult<Vec<JobView>> {
    let job = repo::get(&state.db, id).await?;
    if job.status != "hold" {
        return Err(AppError::InvalidInput(
            "只有「待确认」的工单需要确认".into(),
        ));
    }
    let s = load_settings(&state.db).await?;
    let dir = intake::pending_dir(&s.root_path()).join(&job.dir_name);
    if !dir.is_dir() {
        return Err(AppError::InvalidInput(format!(
            "工单目录已不在：{}",
            dir.display()
        )));
    }
    std::fs::write(
        dir.join(intake::CONFIRM_FILE),
        format!("已在 GenDesk 设置页确认开跑（{}）\n", crate::db::now_unix()),
    )
    .map_err(|e| AppError::Io(format!("写确认文件失败：{e}")))?;
    repo::delete(&state.db, id).await?;
    scan_intake_now(state, app).await
}

/// 在系统文件管理器打开收件目录。
#[tauri::command]
#[specta::specta]
pub async fn open_intake_dir(state: State<'_, AppState>, app: AppHandle) -> AppResult<()> {
    use tauri_plugin_opener::OpenerExt;
    let s = load_settings(&state.db).await?;
    let dir = intake::pending_dir(&s.root_path());
    std::fs::create_dir_all(&dir).map_err(|e| AppError::Io(e.to_string()))?;
    app.opener()
        .open_path(dir.to_string_lossy().to_string(), None::<&str>)
        .map_err(|e| AppError::Io(e.to_string()))
}

/// 选交接根目录（与图生视频那个是同一个根，故此处复用它的选择器语义）。
#[tauri::command]
#[specta::specta]
pub async fn pick_intake_root(app: AppHandle) -> AppResult<Option<String>> {
    use tauri_plugin_dialog::DialogExt;
    let picked = app.dialog().file().blocking_pick_folder();
    Ok(picked.and_then(|p| p.into_path().ok().map(|p| p.to_string_lossy().to_string())))
}

/// 后台侧入口：扫一轮并推事件。失败只记日志——扫描是自动的，报错不该弹到用户脸上，
/// 但**必须**留痕（台账里的 error 行才是给人看的那份）。
pub async fn scan_and_emit(ctx: &Ctx, root: &std::path::Path, app: &AppHandle) {
    match intake::ingest::scan(ctx, root).await {
        Ok(jobs) => emit(app, &jobs),
        Err(e) => tracing::warn!(error = %e, "扫描生图收件目录失败"),
    }
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
/// **这条链路比别处更需要它**：投单那一刻你人在 Claude Code 里，GenDesk 在后台甚至
/// 刚被拉起来——应用内的 toast 你根本看不到。而这里要说的三件事都需要立刻知道：
/// 开始花钱了、卡在等你确认、或者整单没跑成。
///
/// 走 `TauriSink::notify` 而不是自己再调一次插件：系统通知只该有一条发送路径
/// （批次完成、Key 熔断、全局熔断都走它），否则「哪些事会弹通知」会随着调用点分叉。
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
        // 待确认不是错误，是在等人表态——文案必须把「下一步做什么」说出来。
        "hold" => (
            "工单待确认".to_string(),
            format!("{} · {}，去 GenDesk 设置页确认开跑", j.job_id, j.message),
        ),
        _ => (
            "工单收录失败".to_string(),
            format!("{} · {}", j.job_id, j.message),
        ),
    };
    sink.notify(title, body);
}

fn ctx_of(state: &State<'_, AppState>, threshold: i64) -> Ctx {
    Ctx {
        pool: state.db.clone(),
        dirs: state.dirs.clone(),
        engine: state.engine.clone(),
        threshold,
    }
}
