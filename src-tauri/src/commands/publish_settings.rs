//! 发布与同步设置域命令（发布模块执行计划 §3 设置项 / 4.1 publish_settings）。
//!
//! 沿用 settings key/value_json 表，单行 key='publish' 存整份 JSON。
//! 保存 root_local 时校验并创建四分区目录（资产库/收件箱/任务包）。

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;

use crate::db::repo::settings as repo;
use crate::error::{AppError, AppResult};
use crate::publish::paths;
use crate::publish::platform::{platform_infos, PlatformInfo};
use crate::state::AppState;

const KEY: &str = "publish";

/// 平台矩阵（全局启用开关）。字段即五平台 code。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PlatformMatrix {
    pub douyin: bool,
    pub xhs: bool,
    pub kuaishou: bool,
    pub shipinhao: bool,
    pub bilibili: bool,
}

impl Default for PlatformMatrix {
    fn default() -> Self {
        Self {
            douyin: true,
            xhs: true,
            kuaishou: true,
            shipinhao: true,
            bilibili: true,
        }
    }
}

/// 分层频率规则（扁平化 §3 的 `{hot:{daily},warm:{weekly},cold:{weeklyRotate}}`）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TierRules {
    /// 热款：每天次数（× 平台集）。
    pub hot_daily: i64,
    /// 温款：每周次数。
    pub warm_weekly: i64,
    /// 冷款：轮播池每周轮出个数。
    pub cold_weekly_rotate: i64,
}

impl Default for TierRules {
    fn default() -> Self {
        Self {
            hot_daily: 1,
            warm_weekly: 3,
            cold_weekly_rotate: 5,
        }
    }
}

/// 发布与同步设置。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PublishSettings {
    /// 本机根目录（空 = 未配置）。
    #[serde(default)]
    pub root_local: String,
    /// 执行机根目录（空 = 未配置；同机模式一键同本机）。
    #[serde(default)]
    pub root_exec: String,
    /// 执行机路径风格（windows|unix）。
    #[serde(default = "default_path_style")]
    pub path_style: String,
    /// 查重窗口天数。
    #[serde(default = "d_dedup")]
    pub dedup_days: i64,
    /// 回执超时小时数。
    #[serde(default = "d_receipt")]
    pub receipt_timeout_hours: i64,
    /// 每日生成时间（HH:MM）。
    #[serde(default = "d_autogen")]
    pub autogen_time: String,
    /// 素材余量预警阈值。
    #[serde(default = "d_warn_material")]
    pub warn_material: i64,
    /// 标题余量预警阈值。
    #[serde(default = "d_warn_title")]
    pub warn_title: i64,
    /// 正文余量预警阈值（仅对有图集包的 SKU 生效）。
    #[serde(default = "d_warn_body")]
    pub warn_body: i64,
    /// 账号默认日上限。
    #[serde(default = "d_daily_limit")]
    pub account_daily_limit_default: i64,
    /// 同平台多账号最小间隔（分钟）。
    #[serde(default = "d_min_gap")]
    pub min_gap_minutes: i64,
    #[serde(default)]
    pub platform_matrix: PlatformMatrix,
    #[serde(default)]
    pub tier_rules: TierRules,
    /// 时段模板（`HH:MM-HH:MM`）。
    #[serde(default = "d_time_slots")]
    pub time_slots: Vec<String>,
}

fn default_path_style() -> String {
    "windows".into()
}
fn d_dedup() -> i64 {
    30
}
fn d_receipt() -> i64 {
    4
}
fn d_autogen() -> String {
    "22:00".into()
}
fn d_warn_material() -> i64 {
    2
}
fn d_warn_title() -> i64 {
    3
}
fn d_warn_body() -> i64 {
    1
}
fn d_daily_limit() -> i64 {
    3
}
fn d_min_gap() -> i64 {
    60
}
fn d_time_slots() -> Vec<String> {
    vec![
        "11:30-13:00".into(),
        "18:00-20:00".into(),
        "21:00-22:30".into(),
    ]
}

impl Default for PublishSettings {
    fn default() -> Self {
        Self {
            root_local: String::new(),
            root_exec: String::new(),
            path_style: default_path_style(),
            dedup_days: d_dedup(),
            receipt_timeout_hours: d_receipt(),
            autogen_time: d_autogen(),
            warn_material: d_warn_material(),
            warn_title: d_warn_title(),
            warn_body: d_warn_body(),
            account_daily_limit_default: d_daily_limit(),
            min_gap_minutes: d_min_gap(),
            platform_matrix: PlatformMatrix::default(),
            tier_rules: TierRules::default(),
            time_slots: d_time_slots(),
        }
    }
}

impl PublishSettings {
    fn sanitize(&mut self) {
        if self.path_style != "windows" && self.path_style != "unix" {
            self.path_style = default_path_style();
        }
        self.dedup_days = self.dedup_days.clamp(1, 3650);
        self.receipt_timeout_hours = self.receipt_timeout_hours.clamp(1, 240);
        self.warn_material = self.warn_material.clamp(0, 100);
        self.warn_title = self.warn_title.clamp(0, 100);
        self.warn_body = self.warn_body.clamp(0, 100);
        self.account_daily_limit_default = self.account_daily_limit_default.clamp(1, 100);
        self.min_gap_minutes = self.min_gap_minutes.clamp(0, 1440);
    }
}

/// 设置补丁（部分更新）。
#[derive(Debug, Clone, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PublishSettingsPatch {
    pub root_local: Option<String>,
    pub root_exec: Option<String>,
    pub path_style: Option<String>,
    pub dedup_days: Option<i64>,
    pub receipt_timeout_hours: Option<i64>,
    pub autogen_time: Option<String>,
    pub warn_material: Option<i64>,
    pub warn_title: Option<i64>,
    pub warn_body: Option<i64>,
    pub account_daily_limit_default: Option<i64>,
    pub min_gap_minutes: Option<i64>,
    pub platform_matrix: Option<PlatformMatrix>,
    pub tier_rules: Option<TierRules>,
    pub time_slots: Option<Vec<String>>,
}

/// 从连接池加载发布设置（供 watcher/ticker 读取）。
pub async fn load(pool: &sqlx::SqlitePool) -> AppResult<PublishSettings> {
    match repo::get_by_key(pool, KEY).await? {
        Some(json) => {
            let mut s: PublishSettings = serde_json::from_str(&json).unwrap_or_default();
            s.sanitize();
            Ok(s)
        }
        None => Ok(PublishSettings::default()),
    }
}

async fn save(pool: &sqlx::SqlitePool, s: &PublishSettings) -> AppResult<()> {
    let json = serde_json::to_string(s)?;
    repo::set_by_key(pool, KEY, &json).await?;
    Ok(())
}

/// 已配置的本机根目录（未配置报明确错误，两新页据此显示空态引导）。
pub async fn root_local(pool: &sqlx::SqlitePool) -> AppResult<std::path::PathBuf> {
    let s = load(pool).await?;
    if s.root_local.is_empty() {
        return Err(AppError::InvalidInput(
            "尚未配置本机根目录，请到设置页「发布与同步」配置".into(),
        ));
    }
    let root = std::path::PathBuf::from(&s.root_local);
    ensure_partitions(&root)?;
    Ok(root)
}

/// 在给定根目录下创建四分区（资产库/收件箱/任务包），幂等。
pub fn ensure_partitions(root: &std::path::Path) -> AppResult<()> {
    for p in paths::PARTITIONS {
        std::fs::create_dir_all(root.join(p))?;
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn get_publish_settings(state: State<'_, AppState>) -> AppResult<PublishSettings> {
    load(&state.db).await
}

#[tauri::command]
#[specta::specta]
pub async fn update_publish_settings(
    state: State<'_, AppState>,
    patch: PublishSettingsPatch,
) -> AppResult<PublishSettings> {
    let mut s = load(&state.db).await?;
    if let Some(v) = patch.root_local {
        s.root_local = v;
    }
    if let Some(v) = patch.root_exec {
        s.root_exec = v;
    }
    if let Some(v) = patch.path_style {
        s.path_style = v;
    }
    if let Some(v) = patch.dedup_days {
        s.dedup_days = v;
    }
    if let Some(v) = patch.receipt_timeout_hours {
        s.receipt_timeout_hours = v;
    }
    if let Some(v) = patch.autogen_time {
        s.autogen_time = v;
    }
    if let Some(v) = patch.warn_material {
        s.warn_material = v;
    }
    if let Some(v) = patch.warn_title {
        s.warn_title = v;
    }
    if let Some(v) = patch.warn_body {
        s.warn_body = v;
    }
    if let Some(v) = patch.account_daily_limit_default {
        s.account_daily_limit_default = v;
    }
    if let Some(v) = patch.min_gap_minutes {
        s.min_gap_minutes = v;
    }
    if let Some(v) = patch.platform_matrix {
        s.platform_matrix = v;
    }
    if let Some(v) = patch.tier_rules {
        s.tier_rules = v;
    }
    if let Some(v) = patch.time_slots {
        s.time_slots = v;
    }
    s.sanitize();

    // 配置了本机根目录 → 校验/创建四分区（缺失即重建，残余风险默认处置）。
    if !s.root_local.is_empty() {
        ensure_partitions(std::path::Path::new(&s.root_local))?;
    }

    save(&state.db, &s).await?;
    Ok(s)
}

/// 选择本机根目录（dialog）；返回所选路径，取消返回 None。
#[tauri::command]
#[specta::specta]
pub async fn pick_publish_root(app: AppHandle) -> AppResult<Option<String>> {
    let picked = app.dialog().file().blocking_pick_folder();
    Ok(picked
        .and_then(|p| p.into_path().ok())
        .map(|p| p.to_string_lossy().to_string()))
}

/// 拓扑 B「同本机」一键：把执行机根路径设为本机根路径。
#[tauri::command]
#[specta::specta]
pub async fn use_local_as_exec_root(state: State<'_, AppState>) -> AppResult<PublishSettings> {
    let mut s = load(&state.db).await?;
    s.root_exec = s.root_local.clone();
    save(&state.db, &s).await?;
    Ok(s)
}

/// 五平台 `{code, zh}`（前端矩阵/选择器单点数据源）。
#[tauri::command]
#[specta::specta]
pub async fn publish_platforms() -> AppResult<Vec<PlatformInfo>> {
    Ok(platform_infos())
}
