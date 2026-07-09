//! api_keys 域命令（执行计划 2.1 / 1.4）。Key 本体进钥匙串，视图仅脱敏后 4 位。

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::State;

use crate::db::now_unix;
use crate::db::repo::api_keys as repo;
use crate::error::{AppError, AppResult};
use crate::secrets::mask;
use crate::state::AppState;

/// 成功率统计窗口（近 50 次尝试）。
const RATE_WINDOW: i64 = 50;

/// API Key 脱敏视图（Key 本体永不出 Rust）。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ApiKeyView {
    pub id: i64,
    pub name: String,
    /// 脱敏 Key：`****后4位`
    pub masked_key: String,
    pub base_url: String,
    pub model: String,
    pub concurrency_limit: i64,
    pub enabled: bool,
    /// 近 50 次成功率（0.0–1.0）
    pub success_rate: f64,
    /// 成功率样本量
    pub sample_count: i64,
    /// 当前占用并发（运行时；M2 引擎接入后填充，M1 恒为 0）
    pub used_concurrency: i64,
}

#[derive(Debug, Clone, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AddApiKeyInput {
    pub alias: String,
    pub key: String,
    pub base_url: String,
    pub model: String,
    pub concurrency_limit: i64,
}

#[derive(Debug, Clone, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UpdateApiKeyPatch {
    pub name: Option<String>,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub concurrency_limit: Option<i64>,
}

/// base_url 尾部 `/` 归一化（R6：约定已含 /v1）。
fn normalize_base_url(url: &str) -> String {
    url.trim().trim_end_matches('/').to_string()
}

async fn to_view(state: &AppState, row: repo::ApiKeyRow) -> AppResult<ApiKeyView> {
    let masked = state
        .secrets
        .get(&row.keyring_account)?
        .map(|k| mask(&k))
        .unwrap_or_else(|| "****".to_string());
    let (rate, count) = repo::success_rate(&state.db, row.id, RATE_WINDOW).await?;
    Ok(ApiKeyView {
        id: row.id,
        name: row.name,
        masked_key: masked,
        base_url: row.base_url,
        model: row.model,
        concurrency_limit: row.concurrency_limit,
        enabled: row.enabled != 0,
        success_rate: rate,
        sample_count: count,
        used_concurrency: 0,
    })
}

#[tauri::command]
#[specta::specta]
pub async fn list_api_keys(state: State<'_, AppState>) -> AppResult<Vec<ApiKeyView>> {
    let rows = repo::list(&state.db).await?;
    let mut views = Vec::with_capacity(rows.len());
    for row in rows {
        views.push(to_view(&state, row).await?);
    }
    Ok(views)
}

#[tauri::command]
#[specta::specta]
pub async fn add_api_key(
    state: State<'_, AppState>,
    input: AddApiKeyInput,
) -> AppResult<ApiKeyView> {
    if input.key.trim().is_empty() {
        return Err(AppError::InvalidInput("API Key 不能为空".into()));
    }
    let concurrency = input.concurrency_limit.clamp(1, 10);
    let account = format!("apikey-{}-{}", now_unix(), nano_suffix());

    // 先写钥匙串，再写库；库中仅存引用。
    state.secrets.set(&account, input.key.trim())?;

    let new = repo::NewApiKey {
        name: input.alias.trim().to_string(),
        keyring_account: account.clone(),
        base_url: normalize_base_url(&input.base_url),
        model: input.model.trim().to_string(),
        concurrency_limit: concurrency,
    };
    let id = match repo::insert(&state.db, &new).await {
        Ok(id) => id,
        Err(e) => {
            // 回滚钥匙串，避免孤儿密钥。
            let _ = state.secrets.delete(&account);
            return Err(e.into());
        }
    };

    let row = repo::get(&state.db, id)
        .await?
        .ok_or_else(|| AppError::Database("刚插入的 Key 未找到".into()))?;
    to_view(&state, row).await
}

#[tauri::command]
#[specta::specta]
pub async fn update_api_key(
    state: State<'_, AppState>,
    id: i64,
    patch: UpdateApiKeyPatch,
) -> AppResult<ApiKeyView> {
    let base = patch.base_url.map(|u| normalize_base_url(&u));
    let concurrency = patch.concurrency_limit.map(|c| c.clamp(1, 10));
    repo::update_fields(
        &state.db,
        id,
        patch.name.as_deref(),
        base.as_deref(),
        patch.model.as_deref(),
        concurrency,
    )
    .await?;
    let row = repo::get(&state.db, id)
        .await?
        .ok_or_else(|| AppError::InvalidInput("Key 不存在".into()))?;
    to_view(&state, row).await
}

#[tauri::command]
#[specta::specta]
pub async fn set_api_key_enabled(
    state: State<'_, AppState>,
    id: i64,
    enabled: bool,
) -> AppResult<()> {
    repo::set_enabled(&state.db, id, enabled).await?;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn delete_api_key(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    if let Some(account) = repo::delete(&state.db, id).await? {
        let _ = state.secrets.delete(&account);
    }
    Ok(())
}

/// 纳秒级后缀，用于生成唯一 keyring 账户名。
fn nano_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}
