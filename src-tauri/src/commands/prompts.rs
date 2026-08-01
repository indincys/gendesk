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
    /// 写出这一组词的 skill（0032，来自组头 `skill:` 或 job.json）。
    ///
    /// 它跟着预览走一趟前端再回来，而不是在收件侧直接写库：手工导入与工单收件共用
    /// 同一条落库路径（`commit_preview`），让其中一条绕过去，两条路就会开始分叉。
    /// 手工导入这一项恒为 null —— 不知道就别写。
    pub skill: Option<String>,
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
    /// 发布域 SKU 绑定。
    pub sku_id: Option<i64>,
    pub sku_code: Option<String>,
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
        let sku_code = match g.sku_id {
            Some(id) => {
                sqlx::query_scalar("SELECT code FROM skus WHERE id=?1")
                    .bind(id)
                    .fetch_optional(&state.db)
                    .await?
            }
            None => None,
        };
        out.push(GroupView {
            id: g.id,
            name: g.name,
            prefix: g.prefix,
            scene: g.scene,
            is_temp: g.is_temp != 0,
            count,
            tags,
            archived: g.archived_at.is_some(),
            sku_id: g.sku_id,
            sku_code,
        });
    }
    Ok(out)
}

/// 一个分组该带哪些用途（纯规则，便于测试）。空 = 不标。
///
/// **已标过用途的组一律跳过**：人可能刚刚手动取消过，下一轮又给它加回去是最气人的
/// 那种「软件比我懂」。这条规则只解决「从来没标过」这一种情况。
///
/// 存量补标那个命令随提示词库页一起去掉了（v0.21.0：提示词是消耗品，不存在需要
/// 回头补标的历史资产），但规则本身仍是导入预览预猜用途的依据，故留在这里。
fn purpose_candidates(existing: &[String], name: &str, scene: &str) -> Vec<String> {
    if existing.iter().any(|t| crate::purpose::is_purpose(t)) {
        return Vec::new();
    }
    crate::purpose::infer_purposes(name, scene, existing)
}

/// 受控用途清单（前端选择器渲染源，单点定义在 `purpose.rs`）。
#[tauri::command]
#[specta::specta]
pub async fn list_purposes() -> AppResult<Vec<crate::purpose::PurposeView>> {
    Ok(crate::purpose::all())
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
    build_preview(&state.db, &bytes, stem.as_deref()).await
}

/// txt 字节 → 导入预览（不落库）。
///
/// 从命令里提出来只为一件事：**工单收件（`intake`）与手动导入必须是同一条解析路径**。
/// 分成两份实现的话，形态推断、前缀分配、用途预猜三件事迟早各走各的，
/// 而「同一份 txt 手动导入是 3 组、经工单进来是 1 组」这种分歧没人能解释。
pub(crate) async fn build_preview(
    pool: &sqlx::SqlitePool,
    bytes: &[u8],
    stem: Option<&str>,
) -> AppResult<ImportPreview> {
    let parsed = importer::parse_named(bytes, stem);
    if parsed.groups.is_empty() {
        return Err(AppError::InvalidInput("未从文件解析出任何提示词".into()));
    }
    let warnings = parsed
        .warnings
        .into_iter()
        .map(|w| ImportWarning {
            line: w.line as i64,
            message: w.message,
        })
        .collect();
    build_preview_from_parsed(pool, &parsed.groups, parsed.encoding, warnings).await
}

/// 已解析的分组 → 预览（分配前缀、算编号区间、定用途）。
///
/// 工单收件的**内联提示词**从这里进来：它不经 txt 解析器（组名与条目切分由 skill
/// 直接给定），但前缀分配与用途判定必须与 txt 导入完全一致，否则同一个组名经两条
/// 入口会拿到两个前缀。
pub(crate) async fn build_preview_from_parsed(
    pool: &sqlx::SqlitePool,
    parsed_groups: &[ParsedGroup],
    encoding: String,
    warnings: Vec<ImportWarning>,
) -> AppResult<ImportPreview> {
    let mut used_prefixes: HashSet<String> = HashSet::new();
    let mut groups = Vec::with_capacity(parsed_groups.len());
    for g in parsed_groups {
        let (prefix, is_new) = resolve_prefix(pool, g, &mut used_prefixes).await?;
        let count = g.prompts.len() as i64;
        // txt 里显式写了 `标签: 图生视频` 时它已在 tags 里，此时不算「预猜」。
        let explicit: Vec<String> = g
            .tags
            .iter()
            .filter(|t| crate::purpose::is_purpose(t))
            .cloned()
            .collect();
        let guessed = purpose_candidates(&g.tags, &g.name, &g.scene);
        let purpose_inferred = explicit.is_empty() && !guessed.is_empty();
        let purposes = if explicit.is_empty() {
            guessed
        } else {
            explicit
        };
        groups.push(ImportPreviewGroup {
            name: g.name.clone(),
            code_range: code_range(pool, &prefix, count).await,
            prefix_explicit: g.prefix.is_some(),
            prefix,
            scene: g.scene.clone(),
            tags: g.tags.clone(),
            count,
            is_new_group: is_new,
            inferred: g.origin == importer::GroupOrigin::Inferred,
            skill: g.skill.clone(),
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

    let total = groups.iter().map(|g| g.count).sum();
    Ok(ImportPreview {
        encoding,
        total,
        groups,
        warnings,
    })
}

/// 编号区间预览字符串（忽略回收池，仅供参考）。空组返回空串。
async fn code_range(pool: &sqlx::SqlitePool, prefix: &str, count: i64) -> String {
    if count <= 0 {
        return String::new();
    }
    let start = ids::peek_next(pool, prefix).await.unwrap_or(1);
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
            ..Default::default()
        };
        let (prefix, is_new) = resolve_prefix(&state.db, &parsed, &mut used_prefixes).await?;
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
            code_range: code_range(&state.db, &prefix, count).await,
            prefix,
            prefix_explicit: g.prefix_explicit,
            scene: g.scene.clone(),
            tags: g.tags.clone(),
            count,
            is_new_group: is_new,
            // 「疑似」由用户在预览里点确认才消，不因为改了别处而自动消失。
            inferred: g.inferred,
            // 重算预览（用户改了名/拆了组）时原样带回：它是一件已经发生的事实，
            // 与用户在预览里改了什么无关。丢掉它，工单收录路上重算一次预览
            // 就会把整批词的来源抹成「不知道」。
            skill: g.skill.clone(),
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
    pool: &sqlx::SqlitePool,
    g: &ParsedGroup,
    used: &mut HashSet<String>,
) -> AppResult<(String, bool)> {
    // 显式前缀：若 DB 已有同前缀分组 → 复用（追加），否则新建。
    if let Some(p) = &g.prefix {
        let exists = repo::find_group_by_prefix(pool, p).await?.is_some();
        used.insert(p.clone());
        return Ok((p.clone(), !exists));
    }
    // 生成唯一前缀
    let base = gen_prefix_from_name(&g.name);
    let mut candidate = base.clone();
    let mut n = 1;
    loop {
        let db_taken = repo::find_group_by_prefix(pool, &candidate)
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
    commit_preview(&state.db, &preview, &ctx).await
}

/// 预览 → 落库（号池发放与写入同事务）。
///
/// 与 `build_preview` 同样的理由提出来：工单收件走这里，于是「同前缀复用已有组」
/// 「用途只增不减」「空组不落库」这些规则对两条入口天然一致。
pub(crate) async fn commit_preview(
    pool: &sqlx::SqlitePool,
    preview: &ImportPreview,
    ctx: &str,
) -> AppResult<ImportResult> {
    let is_temp = ctx == "generate";
    let source = if is_temp { "temp_import" } else { "library" };

    // 预览可被用户改过（改名/拆并/删条），这里按最终态兜底校验，不信任前端结构。
    if preview.groups.iter().all(|g| g.prompts.is_empty()) {
        return Err(AppError::InvalidInput("没有可导入的提示词".into()));
    }

    let mut tx = pool.begin().await?;
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
        let group_id = match repo::find_group_by_prefix(pool, &prefix).await? {
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
                pg.skill.as_deref().filter(|s| !s.trim().is_empty()),
            )
            .await?;
            inserted += 1;
        }

        // 分组级标签绑定（V1：entity_type='prompt_group'）。用途与 txt 里自由写的
        // `标签: 白底,3C` 恰好共用一张 tags 表，故这里是合并而不是覆盖。
        //
        // **导入只增不减**：追加进已有组（同前缀二次导入）时，绝不因为这份新 txt 的组名
        // 不带关键词就把上次标好的用途抹掉。要改用途就在导入预览那一刻改——提示词是
        // 消耗品，一份 txt 从进库到跑完就那么一次，没有「回头再管理它」这个阶段。
        let purposes = validate_purposes(&pg.purposes)?;
        let mut merged = repo::group_tags(pool, group_id).await?;
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
    fn purpose_rule_skips_groups_that_already_have_a_purpose() {
        let tagged = vec![crate::purpose::PURPOSE_I2V.to_string()];
        assert!(
            purpose_candidates(&tagged, "鹿晗-B-Roll素材分镜图", "").is_empty(),
            "已标过的组不得重复补标"
        );
        // 只有自由标签的组照常补（自由标签与用途是两套东西）。
        let free = vec!["白底".to_string()];
        assert_eq!(
            purpose_candidates(&free, "鹿晗-B-Roll素材分镜图", ""),
            vec![crate::purpose::PURPOSE_I2V.to_string()]
        );
    }

    // 真实组名（用户库里 187 组中的 33 个长这样）必须命中；普通组名不得被误标。
    #[test]
    fn purpose_rule_matches_real_group_names_only() {
        for name in [
            "梓渝——b-roll图片素材",
            "鹿晗-B-Roll素材分镜图",
            "G-Dragon-B-Roll素材分镜图",
        ] {
            assert_eq!(
                purpose_candidates(&[], name, ""),
                vec![crate::purpose::PURPOSE_I2V.to_string()],
                "{name} 应命中"
            );
        }
        for name in ["电商主图", "白底商品", "详情页"] {
            assert!(
                purpose_candidates(&[], name, "").is_empty(),
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
