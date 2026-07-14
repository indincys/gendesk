//! 账号档案域命令（发布模块执行计划 4.1 accounts 域）。

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::State;

use crate::commands::publish_settings;
use crate::db::repo::accounts as repo;
use crate::error::{AppError, AppResult};
use crate::publish::platform::Platform;
use crate::state::AppState;

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AccountView {
    pub id: i64,
    pub platform: String,
    pub platform_zh: String,
    pub name: String,
    pub daily_limit: i64,
    pub slots: Option<Vec<String>>,
    pub status: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CreateAccountInput {
    pub platform: String,
    pub name: String,
    pub daily_limit: Option<i64>,
    pub slots: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AccountPatch {
    pub name: Option<String>,
    pub daily_limit: Option<i64>,
    /// `Some(None)` = 清除（跟随全局时段模板）；`Some(Some)` = 设置。
    pub slots: Option<Option<Vec<String>>>,
}

fn to_view(r: repo::AccountRow) -> AccountView {
    let platform_zh = Platform::from_code(&r.platform)
        .map(|p| p.zh().to_string())
        .unwrap_or_else(|| r.platform.clone());
    AccountView {
        id: r.id,
        platform: r.platform,
        platform_zh,
        name: r.name,
        daily_limit: r.daily_limit,
        slots: r.slots_json.and_then(|s| serde_json::from_str(&s).ok()),
        status: r.status,
        created_at: r.created_at,
    }
}

#[tauri::command]
#[specta::specta]
pub async fn list_accounts(state: State<'_, AppState>) -> AppResult<Vec<AccountView>> {
    Ok(repo::list(&state.db)
        .await?
        .into_iter()
        .map(to_view)
        .collect())
}

#[tauri::command]
#[specta::specta]
pub async fn create_account(
    state: State<'_, AppState>,
    input: CreateAccountInput,
) -> AppResult<i64> {
    if Platform::from_code(&input.platform).is_none() {
        return Err(AppError::InvalidInput(format!(
            "未知平台：{}",
            input.platform
        )));
    }
    let name = input.name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::InvalidInput("账号名称不能为空".into()));
    }
    let daily_limit = match input.daily_limit {
        Some(n) => n,
        None => {
            publish_settings::load(&state.db)
                .await?
                .account_daily_limit_default
        }
    };
    let slots_json = match input.slots {
        Some(s) => Some(serde_json::to_string(&s)?),
        None => None,
    };
    let id = repo::insert(
        &state.db,
        &repo::NewAccount {
            platform: input.platform,
            name,
            daily_limit: daily_limit.clamp(1, 100),
            slots_json,
        },
    )
    .await?;
    Ok(id)
}

#[tauri::command]
#[specta::specta]
pub async fn update_account(
    state: State<'_, AppState>,
    id: i64,
    patch: AccountPatch,
) -> AppResult<()> {
    let slots_arg: Option<Option<String>> = match &patch.slots {
        None => None,
        Some(None) => Some(None),
        Some(Some(s)) => Some(Some(serde_json::to_string(s)?)),
    };
    repo::update_fields(
        &state.db,
        id,
        patch.name.as_deref(),
        patch.daily_limit.map(|n| n.clamp(1, 100)),
        slots_arg.as_ref().map(|o| o.as_deref()),
    )
    .await?;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn set_account_status(
    state: State<'_, AppState>,
    id: i64,
    status: String,
) -> AppResult<()> {
    if status != "active" && status != "disabled" {
        return Err(AppError::InvalidInput(format!("非法状态：{status}")));
    }
    repo::set_status(&state.db, id, &status).await?;
    Ok(())
}
