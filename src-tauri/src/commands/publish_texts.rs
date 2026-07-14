//! 标题池 / 正文池域命令（发布模块执行计划 4.1 texts 域）。

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::State;

use crate::db::repo::texts as repo;
use crate::error::{AppError, AppResult};
use crate::publish::platform::{self, Platform};
use crate::state::AppState;

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TextItemView {
    pub id: i64,
    pub sku_id: i64,
    pub kind: String,
    pub text: String,
    /// 平台标签 code（douyin…|general）。
    pub platform: String,
    /// 平台中文名（UI 展示）。
    pub platform_zh: String,
    pub source: String,
    pub enabled: bool,
    pub use_count: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AddTextItemInput {
    pub sku_id: i64,
    pub kind: String,
    pub text: String,
    pub platform: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TextItemPatch {
    pub text: Option<String>,
    pub platform: Option<String>,
}

fn platform_zh(code: &str) -> String {
    Platform::from_code(code)
        .map(|p| p.zh().to_string())
        .unwrap_or_else(|| "通用".to_string())
}

fn to_view(r: repo::TextItemRow) -> TextItemView {
    let platform_zh = platform_zh(&r.platform);
    TextItemView {
        id: r.id,
        sku_id: r.sku_id,
        kind: r.kind,
        text: r.text,
        platform: r.platform,
        platform_zh,
        source: r.source,
        enabled: r.enabled != 0,
        use_count: r.use_count,
        created_at: r.created_at,
    }
}

#[tauri::command]
#[specta::specta]
pub async fn list_text_items(
    state: State<'_, AppState>,
    sku_id: i64,
    kind: String,
) -> AppResult<Vec<TextItemView>> {
    let rows = repo::list(&state.db, sku_id, &kind).await?;
    Ok(rows.into_iter().map(to_view).collect())
}

#[tauri::command]
#[specta::specta]
pub async fn add_text_item(state: State<'_, AppState>, input: AddTextItemInput) -> AppResult<i64> {
    if input.kind != "title" && input.kind != "body" {
        return Err(AppError::InvalidInput(format!(
            "非法文本类型：{}",
            input.kind
        )));
    }
    let text = input.text.trim().to_string();
    if text.is_empty() {
        return Err(AppError::InvalidInput("文本不能为空".into()));
    }
    let platform = input
        .platform
        .map(|p| platform::text_platform_tag(&p))
        .unwrap_or_else(|| platform::GENERAL_TAG.to_string());
    let id = repo::insert(
        &state.db,
        &repo::NewTextItem {
            sku_id: input.sku_id,
            kind: input.kind,
            text,
            platform,
            source: "manual".into(),
        },
    )
    .await?;
    Ok(id)
}

#[tauri::command]
#[specta::specta]
pub async fn update_text_item(
    state: State<'_, AppState>,
    id: i64,
    patch: TextItemPatch,
) -> AppResult<()> {
    let platform = patch.platform.map(|p| platform::text_platform_tag(&p));
    repo::update_fields(&state.db, id, patch.text.as_deref(), platform.as_deref()).await?;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn set_text_item_enabled(
    state: State<'_, AppState>,
    id: i64,
    enabled: bool,
) -> AppResult<()> {
    repo::set_enabled(&state.db, id, enabled).await?;
    Ok(())
}

/// 删除文本条目。**被日内容套装引用的不可删**——任务单/台账里那些行 `JOIN text_items`
/// 会查不到标题正文，历史整行消失。已用过的条目请改用「停用」（不再被选中，历史仍在）。
#[tauri::command]
#[specta::specta]
pub async fn delete_text_item(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    let used: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM daily_sets WHERE title_id = ?1 OR body_id = ?1")
            .bind(id)
            .fetch_one(&state.db)
            .await?;
    if used > 0 {
        return Err(AppError::InvalidInput(
            "该文本已被任务单使用，删除会让历史记录失去内容；请改用「停用」".into(),
        ));
    }
    repo::delete(&state.db, id).await?;
    Ok(())
}
