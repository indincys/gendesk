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
    /// 行号级诊断（E37），非致命，仅提示。
    pub warnings: Vec<ImportWarning>,
}

/// 导入诊断（E37：缺分组标记 / 悬空小标题等，含行号）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ImportWarning {
    pub line: i64,
    pub message: String,
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
    /// 组名是猜的（文档没有显式分组标记，按行的形态推断）→ UI 标「疑似」并请用户确认。
    pub inferred: bool,
    /// 受控用途（当前只有「图生视频」）。**导入这一刻就该定下来**：一份 txt 是为一个用途
    /// 写的，这是唯一 100% 知道答案的时刻；等到验收后再回提示词库补标，等于把活推给以后。
    ///
    /// 刻意**不加** `#[serde(default)]`：specta 会把带默认值的字段导成可选（`purposes?`），
    /// 于是前端每一处读它都要先判 undefined，而后端其实永远都序列化它。
    /// 预览结构是前后端整体往返的，缺字段只可能是手写调用，那本就该报错。
    pub purposes: Vec<String>,
    /// 用途是关键词预猜出来的（组名含 B-Roll/分镜/首帧…）→ UI 标琥珀「疑似」。
    /// 与 `inferred` 分开：一个说的是组名的来源，一个说的是用途的来源，可以各自为真。
    pub purpose_inferred: bool,
    /// 前缀是文件里写死的或用户手改的 → `repreview_import` 不再按组名重算。
    pub prefix_explicit: bool,
    /// 提示词（正文 + 可选小标题；commit 阶段回传落库）
    pub prompts: Vec<ImportPreviewPrompt>,
}

/// 导入预览中的单条提示词（正文 + 可选小标题）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ImportPreviewPrompt {
    pub title: Option<String>,
    pub text: String,
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

/// 规整用户手填的前缀：只留 ASCII 字母数字、大写、最长 6 位；剩不下东西则返回 None。
fn sanitize_prefix(raw: &str) -> Option<String> {
    let s: String = raw
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(6)
        .collect::<String>()
        .to_uppercase();
    (!s.is_empty()).then_some(s)
}

/// 提示词视图（编号网格 / 详情）。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PromptView {
    pub id: i64,
    pub group_id: i64,
    pub code: String,
    pub title: Option<String>,
    pub text: String,
    pub favorite: bool,
    pub edited: bool,
}

fn to_prompt_view(r: repo::PromptRow) -> PromptView {
    PromptView {
        id: r.id,
        group_id: r.group_id,
        code: r.code,
        title: r.title,
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
    if let Some((code, title, _gid)) = repo::set_trash(&state.db, id).await? {
        let mut tx = state.db.begin().await?;
        crate::db::repo::trash::insert(
            &mut tx,
            &crate::db::repo::trash::NewTrashItem {
                entity_type: "prompt".into(),
                ref_id: Some(id),
                thumb_path: None,
                prompt_text: None,
                code: Some(code),
                title,
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
    /// 分组绑定的标签（E20 按标签筛选）。
    pub tags: Vec<String>,
    /// 已归档（0016）：批次开跑后自动置位，生成页选择器默认折起，库页仍可见可恢复。
    pub archived: bool,
}

/// 列出全部提示词分组（含 active 提示词数 + 标签）。
#[tauri::command]
#[specta::specta]
pub async fn list_prompt_groups(state: State<'_, AppState>) -> AppResult<Vec<GroupView>> {
    let groups = repo::list_groups(&state.db).await?;
    let mut out = Vec::with_capacity(groups.len());
    for g in groups {
        let count = repo::count_in_group(&state.db, g.id).await?;
        let tags = repo::group_tags(&state.db, g.id).await?;
        out.push(GroupView {
            id: g.id,
            name: g.name,
            prefix: g.prefix,
            scene: g.scene,
            is_temp: g.is_temp != 0,
            count,
            tags,
            archived: g.archived_at.is_some(),
        });
    }
    Ok(out)
}

/// 归档 / 取消归档分组（0016）。批次开跑后由 `engine::create_batch` 自动归档；
/// 此命令供库页手动恢复（或手动归档一个用不上的旧组）。
#[tauri::command]
#[specta::specta]
pub async fn set_prompt_group_archived(
    state: State<'_, AppState>,
    id: i64,
    archived: bool,
) -> AppResult<()> {
    if !repo::set_group_archived(&state.db, id, archived).await? {
        return Err(AppError::InvalidInput("分组不存在".into()));
    }
    Ok(())
}

/// 设置分组的受控「用途」，返回该组的最终标签集合。
///
/// 标签此前只有导入 txt 里写 `标签: xxx` 一条写入路径——而实测用户的 txt 从不带任何语法标记
/// （v0.12.0 的形态推断就是为此而生），于是全库 tags 表长期一条记录都没有：机制建好了，
/// 但入口只开在一个没人走的地方。此命令补上第二条、也是实际会走的那条路径。
///
/// **只替换用途标签，保留该组从 txt 导入的自由标签**：用途选择器不该顺手抹掉用户
/// 在 txt 里写的 `标签: 白底,3C`。
///
/// 取值在此**强制校验**，不只靠 UI 只给选择器：命令是公开边界，一旦放进自由字符串，
/// 「图生视频 / 图转视频 / v2v」三种拼法就会同时进库，下游按名字筛选各漏一半。
#[tauri::command]
#[specta::specta]
pub async fn set_prompt_group_purposes(
    state: State<'_, AppState>,
    id: i64,
    purposes: Vec<String>,
) -> AppResult<Vec<String>> {
    if repo::get_group(&state.db, id).await?.is_none() {
        return Err(AppError::InvalidInput("分组不存在".into()));
    }
    for p in &purposes {
        if !crate::purpose::is_purpose(p) {
            return Err(AppError::InvalidInput(format!("未知用途：{p}")));
        }
    }
    let existing = repo::group_tags(&state.db, id).await?;
    let merged = crate::purpose::merge_purposes(&existing, &purposes);

    let mut tx = state.db.begin().await?;
    repo::set_group_tags(&mut tx, id, &merged).await?;
    tx.commit().await?;
    // 回读而非回显入参：去空白/去重/排序后的真实落库值才是前端该显示的。
    Ok(repo::group_tags(&state.db, id).await?)
}

/// 按组名批量补标用途的一条命中。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PurposeHit {
    pub group_id: i64,
    pub name: String,
    /// 该组下已验收通过的作品数（让人判断这一条值不值得标）。
    pub work_count: i64,
    pub purposes: Vec<String>,
}

/// 批量补标结果。`apply=false` 时只预览（applied=0）。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PurposeBackfill {
    pub hits: Vec<PurposeHit>,
    pub applied: i64,
    /// 全库分组总数（让人看清 33/187 这个比例，而不是只看到一个绝对数字）。
    pub scanned: i64,
}

/// 一个存量分组该补哪些用途（纯规则，便于测试）。空 = 跳过它。
///
/// **已标过用途的组一律跳过**：人可能刚刚手动取消过，下一轮重扫又给它加回去是最气人的
/// 那种「软件比我懂」。补标只解决「从来没标过」这一种情况。
fn backfill_candidates(existing: &[String], name: &str, scene: &str) -> Vec<String> {
    if existing.iter().any(|t| crate::purpose::is_purpose(t)) {
        return Vec::new();
    }
    crate::purpose::infer_purposes(name, scene, existing)
}

/// 按组名/场景/标签给**存量分组**批量补标用途（先预览，再确认应用）。
///
/// 为什么必须有这条：用途只在导入预览里选，那只覆盖**以后**导入的 txt。而实测存量
/// `tags` 表一条记录都没有（机制建好了但入口从来没人走），187 个分组里 33 个组名带
/// `B-Roll`/`分镜`/`首帧`、覆盖 40 张验收图 —— 让人手点 33 次是白干的活，
/// 而不补就等于「验收自动入队」对全部历史资产失效。
///
/// **只增不减**：已有用途的组跳过（不重复写），也绝不因为组名不含关键词就摘掉人工标过的用途。
#[tauri::command]
#[specta::specta]
pub async fn backfill_group_purposes(
    state: State<'_, AppState>,
    apply: bool,
) -> AppResult<PurposeBackfill> {
    let groups = repo::list_groups(&state.db).await?;
    let scanned = groups.len() as i64;
    let mut hits: Vec<PurposeHit> = Vec::new();
    let mut applied = 0i64;

    for g in groups {
        let existing = repo::group_tags(&state.db, g.id).await?;
        let guessed = backfill_candidates(&existing, &g.name, &g.scene);
        if guessed.is_empty() {
            continue;
        }
        let work_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM accepted_works WHERE group_id = ?1")
                .bind(g.id)
                .fetch_one(&state.db)
                .await?;
        if apply {
            let merged = crate::purpose::merge_purposes(&existing, &guessed);
            let mut tx = state.db.begin().await?;
            repo::set_group_tags(&mut tx, g.id, &merged).await?;
            tx.commit().await?;
            applied += 1;
        }
        hits.push(PurposeHit {
            group_id: g.id,
            name: g.name,
            work_count,
            purposes: guessed,
        });
    }
    Ok(PurposeBackfill {
        hits,
        applied,
        scanned,
    })
}

/// 受控用途清单（前端选择器渲染源，单点定义在 `purpose.rs`）。
#[tauri::command]
#[specta::specta]
pub async fn list_purposes() -> AppResult<Vec<crate::purpose::PurposeView>> {
    Ok(crate::purpose::all())
}

/// 新建正式分组（E30a 参考图导入选组 /「新建分组」；E20 分组管理复用）。
/// 自动从分组名生成唯一前缀（号池按前缀发放）。
#[tauri::command]
#[specta::specta]
pub async fn create_prompt_group(state: State<'_, AppState>, name: String) -> AppResult<GroupView> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(AppError::InvalidInput("分组名不能为空".into()));
    }
    // 生成唯一前缀：与导入 resolve_prefix 同规则（name 首两位 ASCII，冲突追加序号）。
    let base = gen_prefix_from_name(trimmed);
    let mut candidate = base.clone();
    let mut n = 1;
    while repo::find_group_by_prefix(&state.db, &candidate)
        .await?
        .is_some()
    {
        n += 1;
        candidate = format!("{base}{n}");
    }
    let mut tx = state.db.begin().await?;
    let id = repo::create_group(&mut tx, trimmed, &candidate, "", false).await?;
    tx.commit().await?;
    Ok(GroupView {
        id,
        name: trimmed.to_string(),
        prefix: candidate,
        scene: String::new(),
        is_temp: false,
        count: 0,
        tags: Vec::new(),
        archived: false,
    })
}

/// 重命名分组（E20，前缀/编号不变）。
#[tauri::command]
#[specta::specta]
pub async fn rename_prompt_group(
    state: State<'_, AppState>,
    id: i64,
    name: String,
) -> AppResult<()> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(AppError::InvalidInput("分组名不能为空".into()));
    }
    let ok = repo::rename_group(&state.db, id, trimmed).await?;
    if !ok {
        return Err(AppError::InvalidInput("分组不存在".into()));
    }
    Ok(())
}

/// 删除分组（E20）：组内 active 提示词快照入废纸篓（清理时回收编号），随后删除分组。
/// 关联参考图置为未分组、作品快照保留（accepted_works 无外键级联）。
#[tauri::command]
#[specta::specta]
pub async fn delete_prompt_group(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    let group = repo::get_group(&state.db, id)
        .await?
        .ok_or_else(|| AppError::InvalidInput("分组不存在".into()))?;
    let prompts = repo::list_by_group(&state.db, id).await?;
    let source_label = format!("删除分组「{}」", group.name);

    let mut tx = state.db.begin().await?;
    // 组内 active 提示词入废纸篓（保留编号快照供清理回收）。
    for p in &prompts {
        crate::db::repo::trash::insert(
            &mut tx,
            &crate::db::repo::trash::NewTrashItem {
                entity_type: "prompt".into(),
                ref_id: Some(p.id),
                thumb_path: None,
                prompt_text: None,
                code: Some(p.code.clone()),
                title: p.title.clone(),
                source_label: source_label.clone(),
                file_paths: Vec::new(),
            },
        )
        .await?;
    }
    // 删除分组：级联删 prompts / batch_refs；ref_images.group_id 置空；作品快照保留。
    repo::delete_group(&mut tx, id).await?;
    tx.commit().await?;
    Ok(())
}

/// 合并分组（E20）：`fromId` 并入 `intoId`，编号前缀保留原值不重编。
#[tauri::command]
#[specta::specta]
pub async fn merge_prompt_groups(
    state: State<'_, AppState>,
    from_id: i64,
    into_id: i64,
) -> AppResult<()> {
    if from_id == into_id {
        return Err(AppError::InvalidInput("不能合并到同一分组".into()));
    }
    // 两组均须存在。
    repo::get_group(&state.db, from_id)
        .await?
        .ok_or_else(|| AppError::InvalidInput("源分组不存在".into()))?;
    repo::get_group(&state.db, into_id)
        .await?
        .ok_or_else(|| AppError::InvalidInput("目标分组不存在".into()))?;

    let mut tx = state.db.begin().await?;
    repo::merge_into(&mut tx, from_id, into_id).await?;
    repo::delete_group(&mut tx, from_id).await?;
    tx.commit().await?;
    Ok(())
}

/// 批量移动提示词到指定分组（E20 单条 / E36 批量；编号前缀保留原值不重编）。
#[tauri::command]
#[specta::specta]
pub async fn move_prompts_to_group(
    state: State<'_, AppState>,
    ids: Vec<i64>,
    group_id: i64,
) -> AppResult<()> {
    if ids.is_empty() {
        return Ok(());
    }
    repo::get_group(&state.db, group_id)
        .await?
        .ok_or_else(|| AppError::InvalidInput("目标分组不存在".into()))?;
    let mut tx = state.db.begin().await?;
    repo::move_prompts(&mut tx, &ids, group_id).await?;
    tx.commit().await?;
    Ok(())
}

/// 批量设置收藏（E36）。favorite=true 收藏，false 取消。
#[tauri::command]
#[specta::specta]
pub async fn set_prompts_favorite(
    state: State<'_, AppState>,
    ids: Vec<i64>,
    favorite: bool,
) -> AppResult<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let mut tx = state.db.begin().await?;
    repo::set_favorite_many(&mut tx, &ids, favorite).await?;
    tx.commit().await?;
    Ok(())
}

/// 批量删除提示词 → 入废纸篓（E36；编号在清理时回收）。
#[tauri::command]
#[specta::specta]
pub async fn trash_prompts(state: State<'_, AppState>, ids: Vec<i64>) -> AppResult<()> {
    for id in ids {
        if let Some((code, title, _gid)) = repo::set_trash(&state.db, id).await? {
            let mut tx = state.db.begin().await?;
            crate::db::repo::trash::insert(
                &mut tx,
                &crate::db::repo::trash::NewTrashItem {
                    entity_type: "prompt".into(),
                    ref_id: Some(id),
                    thumb_path: None,
                    prompt_text: None,
                    code: Some(code),
                    title,
                    source_label: "批量删除".into(),
                    file_paths: Vec::new(),
                },
            )
            .await?;
            tx.commit().await?;
        }
    }
    Ok(())
}

/// 第一步：解析 txt，构建预览（不落库）。
#[tauri::command]
#[specta::specta]
pub async fn parse_prompt_txt(
    state: State<'_, AppState>,
    path: String,
) -> AppResult<ImportPreview> {
    let bytes = std::fs::read(&path)?;
    // 文件名（不含扩展名）作为「文档里没写分组名」时的兜底组名。
    let stem = std::path::Path::new(&path)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string());
    let parsed = importer::parse_named(&bytes, stem.as_deref());
    if parsed.groups.is_empty() {
        return Err(AppError::InvalidInput("未从文件解析出任何提示词".into()));
    }

    let mut used_prefixes: HashSet<String> = HashSet::new();
    let mut groups = Vec::with_capacity(parsed.groups.len());
    for g in &parsed.groups {
        let (prefix, is_new) = resolve_prefix(&state, g, &mut used_prefixes).await?;
        let count = g.prompts.len() as i64;
        // txt 里显式写了 `标签: 图生视频` 时它已在 tags 里，此时不算「预猜」。
        let explicit: Vec<String> = g
            .tags
            .iter()
            .filter(|t| crate::purpose::is_purpose(t))
            .cloned()
            .collect();
        let guessed = crate::purpose::infer_purposes(&g.name, &g.scene, &g.tags);
        let purpose_inferred = explicit.is_empty() && !guessed.is_empty();
        let purposes = if explicit.is_empty() {
            guessed
        } else {
            explicit
        };
        groups.push(ImportPreviewGroup {
            name: g.name.clone(),
            code_range: code_range(&state, &prefix, count).await,
            prefix_explicit: g.prefix.is_some(),
            prefix,
            scene: g.scene.clone(),
            tags: g.tags.clone(),
            count,
            is_new_group: is_new,
            inferred: g.origin == importer::GroupOrigin::Inferred,
            purposes,
            purpose_inferred,
            prompts: g
                .prompts
                .iter()
                .map(|p| ImportPreviewPrompt {
                    title: p.title.clone(),
                    text: p.text.clone(),
                })
                .collect(),
        });
    }

    let total = parsed.total_prompts() as i64;
    let warnings = parsed
        .warnings
        .into_iter()
        .map(|w| ImportWarning {
            line: w.line as i64,
            message: w.message,
        })
        .collect();
    Ok(ImportPreview {
        encoding: parsed.encoding,
        total,
        groups,
        warnings,
    })
}

/// 编号区间预览字符串（忽略回收池，仅供参考）。空组返回空串。
async fn code_range(state: &AppState, prefix: &str, count: i64) -> String {
    if count <= 0 {
        return String::new();
    }
    let start = ids::peek_next(&state.db, prefix).await.unwrap_or(1);
    format!(
        "{} ~ {}",
        ids::format_code(prefix, start),
        ids::format_code(prefix, start + count - 1)
    )
}

/// 用户在预览里改过组名 / 拆并分组后，重算前缀、编号区间与「是否新建组」。
/// 解析器只负责给出**初稿**：认错分组不再需要回去改 txt，改完这里重新预览即可。
#[tauri::command]
#[specta::specta]
pub async fn repreview_import(
    state: State<'_, AppState>,
    preview: ImportPreview,
) -> AppResult<ImportPreview> {
    let mut used_prefixes: HashSet<String> = HashSet::new();
    let mut groups = Vec::with_capacity(preview.groups.len());
    for g in &preview.groups {
        if g.prompts.is_empty() {
            continue; // 用户把组清空了 → 直接消失
        }
        let name = g.name.trim();
        let name = if name.is_empty() {
            "未命名分组"
        } else {
            name
        };
        // 显式前缀（文件里写的 / 用户手填的）沿用，其余按当前组名重新生成并保证唯一。
        let parsed = ParsedGroup {
            name: name.to_string(),
            prefix: g
                .prefix_explicit
                .then(|| sanitize_prefix(&g.prefix))
                .flatten(),
            scene: g.scene.clone(),
            tags: g.tags.clone(),
            prompts: Vec::new(),
            origin: importer::GroupOrigin::Explicit,
        };
        let (prefix, is_new) = resolve_prefix(&state, &parsed, &mut used_prefixes).await?;
        let count = g.prompts.len() as i64;
        // 用途仍是预猜（用户没表态）→ 按**当前**组名重新推断：改名/拆组后猜测要跟着更新，
        // 否则把 `B-Roll分镜` 改成 `电商主图` 之后那个视频用途还赖着不走。
        // 用户一旦自己选过（purpose_inferred=false），原样沿用——同 prefix_explicit 的门道。
        let purposes = if g.purpose_inferred {
            crate::purpose::infer_purposes(name, &g.scene, &g.tags)
        } else {
            validate_purposes(&g.purposes)?
        };
        groups.push(ImportPreviewGroup {
            name: name.to_string(),
            code_range: code_range(&state, &prefix, count).await,
            prefix,
            prefix_explicit: g.prefix_explicit,
            scene: g.scene.clone(),
            tags: g.tags.clone(),
            count,
            is_new_group: is_new,
            // 「疑似」由用户在预览里点确认才消，不因为改了别处而自动消失。
            inferred: g.inferred,
            purpose_inferred: g.purpose_inferred && !purposes.is_empty(),
            purposes,
            prompts: g.prompts.clone(),
        });
    }
    if groups.is_empty() {
        return Err(AppError::InvalidInput("没有可导入的提示词".into()));
    }
    let total = groups.iter().map(|g| g.count).sum();
    Ok(ImportPreview {
        encoding: preview.encoding,
        total,
        groups,
        warnings: preview.warnings,
    })
}

/// 提示词 txt 模板正文（E37「保存模板」）：覆盖分组/前缀/场景/标签/小标题/序号语法。
const PROMPT_TXT_TEMPLATE: &str = "\
分组: 电商主图
前缀: DZ
场景: 商品
标签: 白底, 3C, 主图

【正面主图】
1. 白底商品正面，居中构图，柔和顶光，画面干净。

【细节特写】
2. 商品材质细节特写，45 度侧光，浅景深。

分组【人物场景】

【楼道骑行】
把卡套连同配件放进照片里，自然光，真实随手拍质感。
";

/// 保存一份提示词 txt 模板到用户选定位置（E37）。返回保存路径；取消返回 None。
#[tauri::command]
#[specta::specta]
pub async fn save_prompt_template(app: tauri::AppHandle) -> AppResult<Option<String>> {
    use tauri_plugin_dialog::DialogExt;
    let picked = app
        .dialog()
        .file()
        .set_file_name("提示词导入模板.txt")
        .add_filter("文本文件", &["txt"])
        .blocking_save_file();
    let Some(path) = picked.and_then(|p| p.into_path().ok()) else {
        return Ok(None);
    };
    std::fs::write(&path, PROMPT_TXT_TEMPLATE)?;
    Ok(Some(path.to_string_lossy().to_string()))
}

/// 校验用途取值（命令边界强制，不只靠 UI 给选择器）。
///
/// 命令是公开边界：放进自由字符串就会「图生视频/图转视频/v2v」三种拼法同时进库，
/// 下游按字符串精确筛选时静默漏掉，且毫无报错。去重后返回，顺序按 `purpose::all()`。
fn validate_purposes(input: &[String]) -> AppResult<Vec<String>> {
    for p in input {
        if !crate::purpose::is_purpose(p) {
            return Err(AppError::InvalidInput(format!("未知用途：{p}")));
        }
    }
    Ok(crate::purpose::all()
        .into_iter()
        .map(|p| p.tag)
        .filter(|t| input.contains(t))
        .collect())
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

    // 预览可被用户改过（改名/拆并/删条），这里按最终态兜底校验，不信任前端结构。
    if preview.groups.iter().all(|g| g.prompts.is_empty()) {
        return Err(AppError::InvalidInput("没有可导入的提示词".into()));
    }

    let mut tx = state.db.begin().await?;
    let mut group_ids = Vec::new();
    let mut inserted = 0i64;

    for pg in &preview.groups {
        let prompts: Vec<&ImportPreviewPrompt> = pg
            .prompts
            .iter()
            .filter(|p| !p.text.trim().is_empty())
            .collect();
        if prompts.is_empty() {
            continue; // 空组不落库
        }
        let name = pg.name.trim();
        let name = if name.is_empty() {
            "未命名分组"
        } else {
            name
        };
        let prefix = sanitize_prefix(&pg.prefix).unwrap_or_else(|| gen_prefix_from_name(name));
        // 复用已有同前缀分组，或新建。
        let group_id = match repo::find_group_by_prefix(&state.db, &prefix).await? {
            Some(existing) => existing.id,
            None => repo::create_group(&mut tx, name, &prefix, &pg.scene, is_temp).await?,
        };
        group_ids.push(group_id);

        for p in prompts {
            let number = ids::allocate(&mut tx, &prefix).await?;
            let code = ids::format_code(&prefix, number);
            repo::insert_prompt(
                &mut tx,
                group_id,
                &code,
                p.title.as_deref(),
                &p.text,
                source,
            )
            .await?;
            inserted += 1;
        }

        // 分组级标签绑定（V1：entity_type='prompt_group'）。与 UI 用途选择器同一写路径。
        //
        // 用途与 txt 里自由写的 `标签: 白底,3C` 恰好共用一张 tags 表，故经 merge_purposes
        // 合并：先剔掉 tags 里混着的用途拼写，再补上校验过的受控值。**追加已有组时不覆盖**
        // 组上原有的用途——同前缀二次导入不该把上次标好的用途抹掉。
        // **导入只增不减**：追加进已有组（同前缀二次导入）时，绝不因为这份新 txt 的组名
        // 不带关键词就把上次标好的用途抹掉。取消用途是提示词库那个选择器的职责
        // （`set_prompt_group_purposes`），那里用户是明确冲着「改用途」去的。
        let purposes = validate_purposes(&pg.purposes)?;
        let mut merged = repo::group_tags(&state.db, group_id).await?;
        for t in pg.tags.iter().chain(purposes.iter()) {
            if !merged.contains(t) {
                merged.push(t.clone());
            }
        }
        repo::bind_group_tags(&mut tx, group_id, &merged).await?;
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

    // 前缀在预览里可手填，落库前必须规整成号池能用的形状（编号 `前缀-0001`）。
    #[test]
    fn sanitize_prefix_keeps_only_ascii_alnum() {
        assert_eq!(sanitize_prefix(" dz ").as_deref(), Some("DZ"));
        assert_eq!(sanitize_prefix("电商DZ主图").as_deref(), Some("DZ"));
        assert_eq!(sanitize_prefix("A-B_C").as_deref(), Some("ABC"));
        assert_eq!(sanitize_prefix("ABCDEFGHIJ").as_deref(), Some("ABCDEF")); // 截 6 位
        assert_eq!(sanitize_prefix("纯中文"), None);
        assert_eq!(sanitize_prefix(""), None);
    }

    // 补标只解决「从来没标过」：已标过用途的组必须跳过，否则人手动取消掉的用途
    // 会在下一轮补标里复活。
    #[test]
    fn backfill_skips_groups_that_already_have_a_purpose() {
        let tagged = vec![crate::purpose::PURPOSE_I2V.to_string()];
        assert!(
            backfill_candidates(&tagged, "鹿晗-B-Roll素材分镜图", "").is_empty(),
            "已标过的组不得重复补标"
        );
        // 只有自由标签的组照常补（自由标签与用途是两套东西）。
        let free = vec!["白底".to_string()];
        assert_eq!(
            backfill_candidates(&free, "鹿晗-B-Roll素材分镜图", ""),
            vec![crate::purpose::PURPOSE_I2V.to_string()]
        );
    }

    // 真实组名（用户库里 187 组中的 33 个长这样）必须命中；普通组名不得被误标。
    #[test]
    fn backfill_matches_real_group_names_only() {
        for name in [
            "梓渝——b-roll图片素材",
            "鹿晗-B-Roll素材分镜图",
            "G-Dragon-B-Roll素材分镜图",
        ] {
            assert_eq!(
                backfill_candidates(&[], name, ""),
                vec![crate::purpose::PURPOSE_I2V.to_string()],
                "{name} 应命中"
            );
        }
        for name in ["电商主图", "白底商品", "详情页"] {
            assert!(
                backfill_candidates(&[], name, "").is_empty(),
                "{name} 不应命中"
            );
        }
    }

    #[test]
    fn gen_prefix_takes_ascii_or_default() {
        assert_eq!(gen_prefix_from_name("DZ 电商"), "DZ");
        assert_eq!(gen_prefix_from_name("ab123"), "AB");
        assert_eq!(gen_prefix_from_name("纯中文"), "IM");
    }
}
