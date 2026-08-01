//! 发布与同步设置域命令（发布模块执行计划 §3 设置项 / 4.1 publish_settings）。
//!
//! 沿用 settings key/value_json 表，单行 key='publish' 存整份 JSON。
//! 保存 root_local 时校验并创建三分区目录（图片素材库/收件箱/任务包）。

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;

use crate::db::repo::settings as repo;
use crate::error::{AppError, AppResult};
use crate::publish::paths;
use crate::publish::platform::{platform_infos, PlatformInfo};
use crate::publish::schedule;
use crate::state::AppState;

const KEY: &str = "publish";

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
    /// 每日生成时间（HH:MM）。
    #[serde(default = "d_autogen")]
    pub autogen_time: String,
    /// 暂停排期（节假日）：ticker 不再自动生成草稿，但**超时扫描与对账照常**
    /// ——回收闭环不能停，否则暂停期间已导出的单永远收不回来。
    #[serde(default)]
    pub schedule_paused: bool,
}

fn default_path_style() -> String {
    "windows".into()
}
fn d_autogen() -> String {
    "22:00".into()
}

impl Default for PublishSettings {
    fn default() -> Self {
        Self {
            root_local: String::new(),
            root_exec: String::new(),
            path_style: default_path_style(),
            autogen_time: d_autogen(),
            schedule_paused: false,
        }
    }
}

impl PublishSettings {
    /// 读路径的兜底：数值夹到合法区间；非法时间字符串（历史库/手改 JSON）落日志后回退默认，
    /// **不静默留着**——写路径由 [`validate_times`] 直接拒绝，故正常不会走到这里。
    fn sanitize(&mut self) {
        if self.path_style != "windows" && self.path_style != "unix" {
            self.path_style = default_path_style();
        }
        if schedule::parse_hhmm(&self.autogen_time).is_none() {
            tracing::warn!(value = %self.autogen_time, "每日生成时间非法，回退默认");
            self.autogen_time = d_autogen();
        }
    }
}

/// 写路径校验：非法时间直接报错（而非静默回退成另一个值——用户以为存进去了，
/// 引擎却按 22:00 跑，是最难查的那类不一致）。
fn validate_times(s: &PublishSettings) -> AppResult<()> {
    if schedule::parse_hhmm(&s.autogen_time).is_none() {
        return Err(AppError::InvalidInput(format!(
            "每日生成时间格式应为 HH:MM（00:00–23:59），收到「{}」",
            s.autogen_time
        )));
    }
    Ok(())
}

fn validate_exec_root(s: &PublishSettings) -> AppResult<()> {
    if s.root_exec.trim().is_empty() {
        return Ok(());
    }
    let style = paths::PathStyle::from_str_or_default(&s.path_style);
    if !paths::is_exec_root_absolute(&s.root_exec, style) {
        return Err(AppError::InvalidInput(
            "执行机根路径必须是所选路径风格下的绝对路径".into(),
        ));
    }
    Ok(())
}

/// 设置补丁（部分更新）。
#[derive(Debug, Clone, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PublishSettingsPatch {
    pub root_local: Option<String>,
    pub root_exec: Option<String>,
    pub path_style: Option<String>,
    pub autogen_time: Option<String>,
    pub schedule_paused: Option<bool>,
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

/// 在给定根目录下创建三分区（图片素材库/收件箱/任务包），幂等。
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
    publish_state: State<'_, crate::publish::PublishState>,
    patch: PublishSettingsPatch,
) -> AppResult<PublishSettings> {
    let before_root = load(&state.db).await?.root_local;
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
    if let Some(v) = patch.autogen_time {
        s.autogen_time = v;
    }
    if let Some(v) = patch.schedule_paused {
        s.schedule_paused = v;
    }
    // 时间字段先拒后清：非法输入报错回前端，不静默改成别的值。
    validate_times(&s)?;
    s.sanitize();
    validate_exec_root(&s)?;

    // 配置了本机根目录 → 校验/创建三分区（缺失即重建，残余风险默认处置）。
    if !s.root_local.is_empty() {
        ensure_partitions(std::path::Path::new(&s.root_local))?;
    }

    save(&state.db, &s).await?;

    // 本机根目录变更 → 热重启收件箱监听。
    if s.root_local != before_root && !s.root_local.is_empty() {
        if let Err(err) = publish_state.restart(std::path::PathBuf::from(&s.root_local)) {
            tracing::warn!(error = %err, "重启收件箱监听失败");
        }
    } else if s.root_local.is_empty() {
        publish_state.stop();
    }
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
/// 路径风格同步跟随本机——执行机就是本机，还留着 `windows` 会拼出 `\` 分隔的假路径。
#[tauri::command]
#[specta::specta]
pub async fn use_local_as_exec_root(state: State<'_, AppState>) -> AppResult<PublishSettings> {
    let mut s = load(&state.db).await?;
    s.root_exec = s.root_local.clone();
    s.path_style = if cfg!(windows) { "windows" } else { "unix" }.into();
    save(&state.db, &s).await?;
    Ok(s)
}

/// 四平台 `{code, zh}`（前端矩阵/选择器单点数据源）。
#[tauri::command]
#[specta::specta]
pub async fn publish_platforms() -> AppResult<Vec<PlatformInfo>> {
    Ok(platform_infos())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即测试失败，是期望行为
mod tests {
    use super::*;

    // B4：非法时间在写路径被**拒绝**，而不是静默回退（回退会让存储值与用户所见不一致）。
    #[test]
    fn invalid_times_are_rejected_not_silently_defaulted() {
        let mut s = PublishSettings::default();
        assert!(validate_times(&s).is_ok());

        s.autogen_time = "25:00".into();
        let err = validate_times(&s).unwrap_err().to_string();
        assert!(err.contains("每日生成时间"), "{err}");

        s.autogen_time = "22:00".into();
        assert!(validate_times(&s).is_ok());
    }

    // 读路径兜底：历史库里已存的非法值回退默认并剔除，不把坏数据带进引擎。
    #[test]
    fn sanitize_repairs_legacy_bad_values() {
        let mut s = PublishSettings {
            autogen_time: "bad".into(),
            ..PublishSettings::default()
        };
        s.sanitize();
        assert_eq!(s.autogen_time, "22:00");
    }

    #[test]
    fn remote_root_uses_the_selected_platforms_absolute_path_rules() {
        let mut settings = PublishSettings {
            root_exec: r"D:\GenDesk".into(),
            path_style: "windows".into(),
            ..PublishSettings::default()
        };
        assert!(validate_exec_root(&settings).is_ok());
        settings.path_style = "unix".into();
        assert!(validate_exec_root(&settings).is_err());
        settings.root_exec = "/srv/gendesk".into();
        assert!(validate_exec_root(&settings).is_ok());
    }
}
