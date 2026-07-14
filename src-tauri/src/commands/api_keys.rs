//! api_keys 域命令（执行计划 2.1 / 1.4）。Key 本体进钥匙串，视图仅脱敏后 4 位。

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::State;

use crate::db::now_unix;
use crate::db::repo::api_keys as repo;
use crate::error::{AppError, AppResult};
use crate::secrets::{mask, SecretStore};
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
    /// 每分钟请求上限（E18）；None = 不限速。
    pub rpm_limit: Option<i64>,
    /// 是否已被自动熔断（E18）：连续 Auth/欠费失败导致停用，可在设置页恢复。
    pub circuit_broken: bool,
    /// 近 50 次成功率（0.0–1.0）
    pub success_rate: f64,
    /// 成功率样本量
    pub sample_count: i64,
    /// 当前占用并发（运行时；M2 引擎接入后填充，M1 恒为 0）
    pub used_concurrency: i64,
    /// 密钥本体在密钥存储中找不到（迁移被拒绝 / 密钥文件损坏自愈后重建）。
    /// 此时引擎会静默跳过这个 Key（任务永远排不到它），UI 必须显式提示重填 —— 否则
    /// `masked_key` 只是普通的 `****`，用户看不出这个 Key 已经是空壳。
    pub secret_missing: bool,
}

#[derive(Debug, Clone, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AddApiKeyInput {
    pub alias: String,
    pub key: String,
    pub base_url: String,
    pub model: String,
    pub concurrency_limit: i64,
    /// 每分钟请求上限（E18）；None/<=0 = 不限速。
    pub rpm_limit: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UpdateApiKeyPatch {
    pub name: Option<String>,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub concurrency_limit: Option<i64>,
    /// None = 不改；Some(n>0) = 设为 n；Some(n<=0) = 清除限速（不限）。
    pub rpm_limit: Option<i64>,
    /// 轮换密钥：None/空串 = 保持原 Key 不变；Some(非空) = 覆写钥匙串中的密钥。
    /// 编辑弹窗里留空即不改 Key，只改元数据。
    pub key: Option<String>,
}

/// base_url 尾部 `/` 归一化（R6：约定已含 /v1）。
fn normalize_base_url(url: &str) -> String {
    url.trim().trim_end_matches('/').to_string()
}

async fn to_view(state: &AppState, row: repo::ApiKeyRow) -> AppResult<ApiKeyView> {
    let secret = state.secrets.get(&row.keyring_account)?;
    let masked = secret
        .as_deref()
        .map(mask)
        .unwrap_or_else(|| "****".to_string());
    let (rate, count) = repo::success_rate(&state.db, row.id, RATE_WINDOW).await?;
    Ok(ApiKeyView {
        id: row.id,
        name: row.name,
        masked_key: masked,
        secret_missing: secret.is_none(),
        base_url: row.base_url,
        model: row.model,
        concurrency_limit: row.concurrency_limit,
        enabled: row.enabled != 0,
        rpm_limit: row.rpm_limit,
        circuit_broken: row.circuit_broken != 0,
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
        rpm_limit: input.rpm_limit.filter(|n| *n > 0),
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
    let view = to_view(&state, row).await?;
    let _ = state
        .engine
        .reload_keys(&state.db, state.secrets.as_ref())
        .await;
    Ok(view)
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
    // rpm_limit：None 不改；Some(n>0) 设值；Some(n<=0) 清除。→ repo 的 Option<Option>。
    let rpm: Option<Option<i64>> = patch.rpm_limit.map(|n| (n > 0).then_some(n));
    repo::update_fields(
        &state.db,
        id,
        patch.name.as_deref(),
        base.as_deref(),
        patch.model.as_deref(),
        concurrency,
        rpm,
    )
    .await?;
    let row = repo::get(&state.db, id)
        .await?
        .ok_or_else(|| AppError::InvalidInput("Key 不存在".into()))?;
    // 密钥轮换：仅当传入非空 Key 时覆写钥匙串（沿用原 keyring 账户名）。
    if let Some(new_key) = patch
        .key
        .as_deref()
        .map(str::trim)
        .filter(|k| !k.is_empty())
    {
        state.secrets.set(&row.keyring_account, new_key)?;
    }
    let view = to_view(&state, row).await?;
    let _ = state
        .engine
        .reload_keys(&state.db, state.secrets.as_ref())
        .await;
    Ok(view)
}

/// 恢复被熔断的 Key（E18）：清熔断位 + 重新启用 + 重载调度器。
#[tauri::command]
#[specta::specta]
pub async fn recover_api_key(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    repo::recover_circuit(&state.db, id).await?;
    let _ = state
        .engine
        .reload_keys(&state.db, state.secrets.as_ref())
        .await;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn set_api_key_enabled(
    state: State<'_, AppState>,
    id: i64,
    enabled: bool,
) -> AppResult<()> {
    repo::set_enabled(&state.db, id, enabled).await?;
    let _ = state
        .engine
        .reload_keys(&state.db, state.secrets.as_ref())
        .await;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn delete_api_key(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    if let Some(account) = repo::delete(&state.db, id).await? {
        let _ = state.secrets.delete(&account);
        // 迁移被用户拒绝过的 Key，密钥还留在系统钥匙串里。DB 行一删，启动迁移的名单里
        // 就再也不会有它 —— 不在这里顺手清掉，那条明文密钥会永久孤儿化在钥匙串中。
        // best-effort：钥匙串里没有该条目属正常（已迁移过），失败也不影响删除本身。
        let _ = crate::secrets::KeyringStore.delete(&account);
    }
    let _ = state
        .engine
        .reload_keys(&state.db, state.secrets.as_ref())
        .await;
    Ok(())
}

/// 探活：GET {base_url}/models 校验 Key + Base URL 可用性（E11，最廉价的探活方式，
/// 不消耗生图额度）。成功返回 Ok，失败返回人话错误（Auth/Other）。
async fn probe(base_url: &str, api_key: &str) -> AppResult<()> {
    let base = normalize_base_url(base_url);
    if base.is_empty() || api_key.trim().is_empty() {
        return Err(AppError::InvalidInput("Base URL 与 Key 均不能为空".into()));
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let resp = client
        .get(format!("{base}/models"))
        .bearer_auth(api_key)
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                AppError::Internal("连接超时：请检查 Base URL 与网络".into())
            } else if e.is_connect() {
                AppError::Internal(format!("无法连接到 {base}：请检查 Base URL 是否正确"))
            } else {
                AppError::Internal(format!("请求失败：{e}"))
            }
        })?;
    let code = resp.status().as_u16();
    match code {
        200..=299 => Ok(()),
        401 | 403 => Err(AppError::InvalidInput("Key 无效或已过期".into())),
        404 => Err(AppError::Internal(
            "端点 /models 不存在：请确认 Base URL 已包含 /v1".into(),
        )),
        429 => Err(AppError::Internal(
            "被限流（429）：Key 可用，但当前请求过多".into(),
        )),
        _ => Err(AppError::Internal(format!("测试失败：HTTP {code}"))),
    }
}

/// 测试一组连接参数（E11：添加/编辑弹窗，raw key 在前端）。
#[tauri::command]
#[specta::specta]
pub async fn test_api_key(base_url: String, api_key: String) -> AppResult<()> {
    probe(&base_url, &api_key).await
}

/// 测试已保存的 Key（E11：Key 行内测试，从钥匙串取密钥）。
#[tauri::command]
#[specta::specta]
pub async fn test_api_key_saved(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    let row = repo::get(&state.db, id)
        .await?
        .ok_or_else(|| AppError::InvalidInput("Key 不存在".into()))?;
    let secret = state
        .secrets
        .get(&row.keyring_account)?
        .ok_or_else(|| AppError::Internal("钥匙串中未找到该 Key 的凭据".into()))?;
    probe(&row.base_url, &secret).await
}

/// 纳秒级后缀，用于生成唯一 keyring 账户名。
fn nano_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即失败
mod tests {
    use super::probe;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // E11：正确配置探活通过。
    #[tokio::test]
    async fn probe_ok_on_200() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        assert!(probe(&server.uri(), "sk-xxx").await.is_ok());
    }

    // E11：401 → 可读的 Key 无效错误。
    #[tokio::test]
    async fn probe_reports_auth_error_on_401() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let err = probe(&server.uri(), "sk-bad").await.unwrap_err();
        assert!(
            err.to_string().contains("Key 无效"),
            "应给出可读的 Key 无效错误，实际：{err}"
        );
    }

    // E11：错误 Base URL（连不上）→ 可读的连接错误。
    #[tokio::test]
    async fn probe_reports_connection_error_on_bad_base_url() {
        // 未监听的地址：立即连接失败。
        let err = probe("http://127.0.0.1:1/v1", "sk-xxx").await.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("无法连接") || msg.contains("请求失败") || msg.contains("超时"),
            "应给出可读的连接错误，实际：{msg}"
        );
    }
}
