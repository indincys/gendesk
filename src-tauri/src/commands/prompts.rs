//! prompts 域：txt 两段式导入（执行计划 2.1 / 1.6 / R7）。

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::State;

use crate::db::repo::prompts as repo;
use crate::error::{AppError, AppResult};
use crate::ids;
use crate::importer::{self, ParsedGroup};
use crate::state::AppState;

/// 导入预览（parse 阶段产物，不落库）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ImportPreview {
    pub encoding: String,
    pub groups: Vec<ImportPreviewGroup>,
    pub total: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ImportPreviewGroup {
    pub name: String,
    pub prefix: String,
    pub scene: String,
    pub tags: Vec<String>,
    pub count: i64,
    /// 预分配编号区间预览，如 "DZ-0001 ~ DZ-0024"（忽略回收池，仅供参考）
    pub code_range: String,
    pub is_new_group: bool,
    /// 提示词正文（commit 阶段回传落库；UI 列表不展示）
    pub prompts: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    pub group_ids: Vec<i64>,
    pub inserted: i64,
    /// 是否新建了临时分组（ctx=generate）
    pub temp: bool,
}

/// 从 name 生成候选前缀：取 ASCII 字母/数字前 2 位大写，缺省 "IM"。
fn gen_prefix_from_name(name: &str) -> String {
    let letters: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(2)
        .collect::<String>()
        .to_uppercase();
    if letters.len() >= 2 {
        letters
    } else {
        "IM".to_string()
    }
}

/// 提示词视图（编号网格 / 详情）。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PromptView {
    pub id: i64,
    pub group_id: i64,
    pub code: String,
    pub text: String,
    pub favorite: bool,
    pub edited: bool,
}

fn to_prompt_view(r: repo::PromptRow) -> PromptView {
    PromptView {
        id: r.id,
        group_id: r.group_id,
        code: r.code,
        text: r.text,
        favorite: r.favorite != 0,
        edited: r.edited != 0,
    }
}

#[tauri::command]
#[specta::specta]
pub async fn list_prompts(state: State<'_, AppState>, group_id: i64) -> AppResult<Vec<PromptView>> {
    let rows = repo::list_by_group(&state.db, group_id).await?;
    Ok(rows.into_iter().map(to_prompt_view).collect())
}

#[tauri::command]
#[specta::specta]
pub async fn search_prompts(
    state: State<'_, AppState>,
    query: String,
) -> AppResult<Vec<PromptView>> {
    let rows = repo::search(&state.db, query.trim()).await?;
    Ok(rows.into_iter().map(to_prompt_view).collect())
}

#[tauri::command]
#[specta::specta]
pub async fn get_prompt(state: State<'_, AppState>, id: i64) -> AppResult<PromptView> {
    let row = repo::get(&state.db, id)
        .await?
        .ok_or_else(|| AppError::InvalidInput("提示词不存在".into()))?;
    Ok(to_prompt_view(row))
}

#[tauri::command]
#[specta::specta]
pub async fn update_prompt_text(
    state: State<'_, AppState>,
    id: i64,
    text: String,
) -> AppResult<()> {
    repo::apply_edit(&state.db, id, &text).await?;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn toggle_prompt_favorite(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    repo::toggle_favorite(&state.db, id).await?;
    Ok(())
}

/// 删除提示词 → 进废纸篓（编号在清理时回收）。
#[tauri::command]
#[specta::specta]
pub async fn trash_prompt(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    if let Some((code, _gid)) = repo::set_trash(&state.db, id).await? {
        let mut tx = state.db.begin().await?;
        crate::db::repo::trash::insert(
            &mut tx,
            &crate::db::repo::trash::NewTrashItem {
                entity_type: "prompt".into(),
                ref_id: Some(id),
                thumb_path: None,
                prompt_text: None,
                code: Some(code),
                source_label: "手动删除".into(),
                file_paths: Vec::new(),
            },
        )
        .await?;
        tx.commit().await?;
    }
    Ok(())
}

/// 分组视图（生成页 / 提示词库列表）。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct GroupView {
    pub id: i64,
    pub name: String,
    pub prefix: String,
    pub scene: String,
    pub is_temp: bool,
    pub count: i64,
}

/// 列出全部提示词分组（含 active 提示词数）。
#[tauri::command]
#[specta::specta]
pub async fn list_prompt_groups(state: State<'_, AppState>) -> AppResult<Vec<GroupView>> {
    let groups = repo::list_groups(&state.db).await?;
    let mut out = Vec::with_capacity(groups.len());
    for g in groups {
        let count = repo::count_in_group(&state.db, g.id).await?;
        out.push(GroupView {
            id: g.id,
            name: g.name,
            prefix: g.prefix,
            scene: g.scene,
            is_temp: g.is_temp != 0,
            count,
        });
    }
    Ok(out)
}

/// 第一步：解析 txt，构建预览（不落库）。
#[tauri::command]
#[specta::specta]
pub async fn parse_prompt_txt(
    state: State<'_, AppState>,
    path: String,
) -> AppResult<ImportPreview> {
    let bytes = std::fs::read(&path)?;
    let parsed = importer::parse(&bytes);
    if parsed.groups.is_empty() {
        return Err(AppError::InvalidInput("未从文件解析出任何提示词".into()));
    }

    let mut used_prefixes: HashSet<String> = HashSet::new();
    let mut groups = Vec::with_capacity(parsed.groups.len());
    for g in &parsed.groups {
        let (prefix, is_new) = resolve_prefix(&state, g, &mut used_prefixes).await?;
        let count = g.prompts.len() as i64;
        let start = ids::peek_next(&state.db, &prefix).await.unwrap_or(1);
        let end = start + count - 1;
        let code_range = format!(
            "{} ~ {}",
            ids::format_code(&prefix, start),
            ids::format_code(&prefix, end)
        );
        groups.push(ImportPreviewGroup {
            name: g.name.clone(),
            prefix,
            scene: g.scene.clone(),
            tags: g.tags.clone(),
            count,
            code_range,
            is_new_group: is_new,
            prompts: g.prompts.clone(),
        });
    }

    let total = parsed.total_prompts() as i64;
    Ok(ImportPreview {
        encoding: parsed.encoding,
        total,
        groups,
    })
}

/// 解析前缀：显式前缀优先；否则由名字生成并保证（本次导入 + DB）唯一。
async fn resolve_prefix(
    state: &AppState,
    g: &ParsedGroup,
    used: &mut HashSet<String>,
) -> AppResult<(String, bool)> {
    // 显式前缀：若 DB 已有同前缀分组 → 复用（追加），否则新建。
    if let Some(p) = &g.prefix {
        let exists = repo::find_group_by_prefix(&state.db, p).await?.is_some();
        used.insert(p.clone());
        return Ok((p.clone(), !exists));
    }
    // 生成唯一前缀
    let base = gen_prefix_from_name(&g.name);
    let mut candidate = base.clone();
    let mut n = 1;
    loop {
        let db_taken = repo::find_group_by_prefix(&state.db, &candidate)
            .await?
            .is_some();
        if !used.contains(&candidate) && !db_taken {
            used.insert(candidate.clone());
            return Ok((candidate, true));
        }
        n += 1;
        candidate = format!("{base}{n}");
    }
}

/// 第二步：落库（ctx=generate 时建临时分组）。号池发放与写入同事务。
#[tauri::command]
#[specta::specta]
pub async fn commit_prompt_import(
    state: State<'_, AppState>,
    preview: ImportPreview,
    ctx: String,
) -> AppResult<ImportResult> {
    let is_temp = ctx == "generate";
    let source = if is_temp { "temp_import" } else { "library" };

    let mut tx = state.db.begin().await?;
    let mut group_ids = Vec::new();
    let mut inserted = 0i64;

    for pg in &preview.groups {
        // 复用已有同前缀分组，或新建。
        let group_id = match repo::find_group_by_prefix(&state.db, &pg.prefix).await? {
            Some(existing) => existing.id,
            None => repo::create_group(&mut tx, &pg.name, &pg.prefix, &pg.scene, is_temp).await?,
        };
        group_ids.push(group_id);

        for text in &pg.prompts {
            let number = ids::allocate(&mut tx, &pg.prefix).await?;
            let code = ids::format_code(&pg.prefix, number);
            repo::insert_prompt(&mut tx, group_id, &code, text, source).await?;
            inserted += 1;
        }

        // 分组级标签绑定（V1：entity_type='prompt_group'）。
        for tag in &pg.tags {
            sqlx::query("INSERT INTO tags (name) VALUES (?1) ON CONFLICT(name) DO NOTHING")
                .bind(tag)
                .execute(&mut *tx)
                .await?;
            let tag_id: i64 = sqlx::query_scalar("SELECT id FROM tags WHERE name = ?1")
                .bind(tag)
                .fetch_one(&mut *tx)
                .await?;
            sqlx::query(
                "INSERT OR IGNORE INTO tag_bindings (tag_id, entity_type, entity_id)
                 VALUES (?1, 'prompt_group', ?2)",
            )
            .bind(tag_id)
            .bind(group_id)
            .execute(&mut *tx)
            .await?;
        }
    }

    tx.commit().await?;
    Ok(ImportResult {
        group_ids,
        inserted,
        temp: is_temp,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即失败
mod tests {
    use super::*;

    #[test]
    fn gen_prefix_takes_ascii_or_default() {
        assert_eq!(gen_prefix_from_name("DZ 电商"), "DZ");
        assert_eq!(gen_prefix_from_name("ab123"), "AB");
        assert_eq!(gen_prefix_from_name("纯中文"), "IM");
    }
}
