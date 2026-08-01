//! 商品根实体、SKU 归属与提示词组 SKU 标注命令。

use serde::{Deserialize, Serialize};
use specta::Type;
use sqlx::SqlitePool;
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;

use crate::db::repo::products as repo;
use crate::error::{AppError, AppResult};
use crate::publish::{paths, product};
use crate::state::AppState;

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProductSkuView {
    pub id: i64,
    pub product_id: Option<i64>,
    pub code: String,
    pub name: String,
    pub tier: String,
    pub status: String,
    pub folder_alias: String,
    pub music_keyword: String,
    pub free_images: i64,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProductView {
    pub id: i64,
    pub code: String,
    pub name: String,
    pub platforms: Vec<String>,
    pub cart_enabled: bool,
    pub douyin_product_url: String,
    pub douyin_short_title: String,
    pub status: String,
    pub note: String,
    pub title_free: i64,
    pub body_free: i64,
    pub image_free: i64,
    pub skus: Vec<ProductSkuView>,
}

#[derive(Debug, Clone, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProductInput {
    pub code: String,
    pub name: String,
    pub platforms: Vec<String>,
    pub cart_enabled: bool,
    pub douyin_product_url: String,
    pub douyin_short_title: String,
    pub note: String,
}

#[derive(Debug, Clone, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProductPatch {
    pub name: String,
    pub platforms: Vec<String>,
    pub cart_enabled: bool,
    pub douyin_product_url: String,
    pub douyin_short_title: String,
    pub status: String,
    pub note: String,
}

#[derive(Debug, Clone, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProductSkuInput {
    pub product_id: i64,
    pub code: String,
    pub name: String,
    pub tier: String,
    pub folder_alias: String,
    pub music_keyword: String,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CatalogImportReport {
    pub rows: i64,
    pub products_created: i64,
    pub skus_created: i64,
    pub skus_updated: i64,
}

fn sku_view(row: repo::ProductSkuRow) -> ProductSkuView {
    ProductSkuView {
        id: row.id,
        product_id: row.product_id,
        code: row.code,
        name: row.style_name,
        tier: row.tier,
        status: row.status,
        folder_alias: row.folder_alias,
        music_keyword: row.music_keyword,
        free_images: row.free_images,
    }
}

#[tauri::command]
#[specta::specta]
pub async fn list_products(state: State<'_, AppState>) -> AppResult<Vec<ProductView>> {
    let products = repo::list(&state.db).await?;
    let all_skus = repo::list_skus(&state.db, None).await?;
    let mut out = Vec::with_capacity(products.len());
    for p in products {
        let (title_free, body_free, image_free): (i64, i64, i64) = sqlx::query_as(
            "SELECT
               (SELECT COUNT(*) FROM text_items WHERE product_id=?1 AND kind='title' AND enabled=1 AND state='free'),
               (SELECT COUNT(*) FROM text_items WHERE product_id=?1 AND kind='body' AND enabled=1 AND state='free'),
               (SELECT COUNT(*) FROM image_assets a JOIN skus s ON s.id=a.sku_id
                  WHERE s.product_id=?1 AND a.state='free')",
        )
        .bind(p.id)
        .fetch_one(&state.db)
        .await?;
        out.push(ProductView {
            id: p.id,
            code: p.code,
            name: p.name,
            platforms: serde_json::from_str(&p.platforms_json).unwrap_or_default(),
            cart_enabled: p.cart_enabled != 0,
            douyin_product_url: p.douyin_product_url,
            douyin_short_title: p.douyin_short_title,
            status: p.status,
            note: p.note,
            title_free,
            body_free,
            image_free,
            skus: all_skus
                .iter()
                .filter(|s| s.product_id == Some(p.id))
                .cloned()
                .map(sku_view)
                .collect(),
        });
    }
    Ok(out)
}

#[tauri::command]
#[specta::specta]
pub async fn list_product_skus(state: State<'_, AppState>) -> AppResult<Vec<ProductSkuView>> {
    Ok(repo::list_skus(&state.db, None)
        .await?
        .into_iter()
        .map(sku_view)
        .collect())
}

fn validate_product_input(input: &ProductInput) -> AppResult<(String, String)> {
    let code = product::validate_code(&input.code)?;
    let name = input.name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::InvalidInput("商品名不能为空".into()));
    }
    product::validate_platforms(&input.platforms)?;
    product::validate_short_title(&input.douyin_short_title)?;
    if input.cart_enabled
        && (input.douyin_product_url.trim().is_empty()
            || input.douyin_short_title.trim().is_empty())
    {
        return Err(AppError::InvalidInput(
            "挂车开启时，抖音商品链接与短标题都必填".into(),
        ));
    }
    Ok((code, name))
}

#[tauri::command]
#[specta::specta]
pub async fn create_product(state: State<'_, AppState>, input: ProductInput) -> AppResult<i64> {
    let (code, name) = validate_product_input(&input)?;
    let platforms = serde_json::to_string(&input.platforms)?;
    Ok(repo::insert(
        &state.db,
        &code,
        &name,
        &platforms,
        input.cart_enabled,
        input.douyin_product_url.trim(),
        input.douyin_short_title.trim(),
        input.note.trim(),
    )
    .await?)
}

#[tauri::command]
#[specta::specta]
pub async fn update_product(
    state: State<'_, AppState>,
    id: i64,
    patch: ProductPatch,
) -> AppResult<()> {
    if patch.name.trim().is_empty() {
        return Err(AppError::InvalidInput("商品名不能为空".into()));
    }
    if !matches!(patch.status.as_str(), "active" | "paused") {
        return Err(AppError::InvalidInput("商品状态非法".into()));
    }
    product::validate_platforms(&patch.platforms)?;
    product::validate_short_title(&patch.douyin_short_title)?;
    if patch.cart_enabled
        && (patch.douyin_product_url.trim().is_empty()
            || patch.douyin_short_title.trim().is_empty())
    {
        return Err(AppError::InvalidInput(
            "挂车开启时，抖音商品链接与短标题都必填".into(),
        ));
    }
    repo::update(
        &state.db,
        id,
        patch.name.trim(),
        &serde_json::to_string(&patch.platforms)?,
        patch.cart_enabled,
        patch.douyin_product_url.trim(),
        patch.douyin_short_title.trim(),
        &patch.status,
        patch.note.trim(),
    )
    .await?;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn delete_product(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    let mut tx = state.db.begin().await?;
    let refs: i64 = sqlx::query_scalar(
        "SELECT (SELECT COUNT(*) FROM skus WHERE product_id=?1)
              + (SELECT COUNT(*) FROM task_sheets WHERE product_id=?1)
              + (SELECT COUNT(*) FROM text_items WHERE product_id=?1)
              + (SELECT COUNT(*) FROM topic_groups WHERE product_id=?1)",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    if refs > 0 {
        return Err(AppError::InvalidInput(
            "商品下仍有 SKU、文案、话题组或任务单，请先清理；暂停商品不会丢数据".into(),
        ));
    }
    sqlx::query("DELETE FROM sheet_configs WHERE product_id=?1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM products WHERE id=?1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn assign_skus_to_product(
    state: State<'_, AppState>,
    product_id: i64,
    sku_ids: Vec<i64>,
) -> AppResult<i64> {
    if repo::get(&state.db, product_id).await?.is_none() {
        return Err(AppError::InvalidInput("商品不存在".into()));
    }
    let mut tx = state.db.begin().await?;
    for sku_id in &sku_ids {
        if repo::sku_reassign_blocked(&mut tx, *sku_id, Some(product_id)).await? {
            return Err(AppError::InvalidInput(format!(
                "SKU {sku_id} 正被未关闭任务单引用，不能跨商品改挂"
            )));
        }
    }
    let changed = repo::assign_skus(&mut tx, product_id, &sku_ids).await?;
    tx.commit().await?;
    Ok(changed as i64)
}

#[tauri::command]
#[specta::specta]
pub async fn create_product_sku(
    state: State<'_, AppState>,
    input: ProductSkuInput,
) -> AppResult<i64> {
    let code = input.code.trim();
    if !paths::is_valid_sku_code(code) {
        return Err(AppError::InvalidInput("SKU 编码格式非法".into()));
    }
    if !matches!(input.tier.as_str(), "hot" | "warm" | "cold") {
        return Err(AppError::InvalidInput(
            "SKU 分层必须是 hot/warm/cold".into(),
        ));
    }
    let now = crate::db::now_unix();
    let id = sqlx::query_scalar(
        "INSERT INTO skus(code,style_name,product_name,tier,topics_json,status,is_general,note,
                          created_at,updated_at,folder_alias,product_id,music_keyword)
         VALUES(?1,?2,'',?3,'[]','active',0,?4,?5,?5,?6,?7,?8) RETURNING id",
    )
    .bind(code)
    .bind(input.name.trim())
    .bind(&input.tier)
    .bind(input.note.trim())
    .bind(now)
    .bind(input.folder_alias.trim())
    .bind(input.product_id)
    .bind(input.music_keyword.trim())
    .fetch_one(&state.db)
    .await?;
    Ok(id)
}

#[tauri::command]
#[specta::specta]
pub async fn update_product_sku(
    state: State<'_, AppState>,
    id: i64,
    product_id: Option<i64>,
    tier: String,
    music_keyword: String,
) -> AppResult<()> {
    if !matches!(tier.as_str(), "hot" | "warm" | "cold") {
        return Err(AppError::InvalidInput(
            "SKU 分层必须是 hot/warm/cold".into(),
        ));
    }
    let mut tx = state.db.begin().await?;
    if repo::sku_reassign_blocked(&mut tx, id, product_id).await? {
        return Err(AppError::InvalidInput(
            "SKU 正被未关闭任务单引用，不能跨商品改挂".into(),
        ));
    }
    repo::update_sku_publish_fields(&mut tx, id, product_id, &tier, music_keyword.trim()).await?;
    tx.commit().await?;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn set_prompt_group_sku(
    state: State<'_, AppState>,
    group_id: i64,
    sku_id: Option<i64>,
) -> AppResult<()> {
    sqlx::query("UPDATE prompt_groups SET sku_id=?2 WHERE id=?1")
        .bind(group_id)
        .bind(sku_id)
        .execute(&state.db)
        .await?;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn pick_product_catalog_file(app: AppHandle) -> AppResult<Option<String>> {
    Ok(app
        .dialog()
        .file()
        .add_filter("商品建档表", &["csv", "tsv", "txt"])
        .blocking_pick_file()
        .and_then(|path| path.into_path().ok())
        .map(|path| path.to_string_lossy().to_string()))
}

fn tier_code(raw: &str) -> Option<&'static str> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "hot" | "热" | "热款" => Some("hot"),
        "warm" | "温" | "温款" | "" => Some("warm"),
        "cold" | "冷" | "冷款" => Some("cold"),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CatalogRow {
    product_code: String,
    product_name: String,
    sku_code: String,
    sku_name: String,
    tier: String,
    folder_alias: String,
    music_keyword: String,
}

fn split_catalog_line(line: &str, delimiter: char) -> Result<Vec<String>, String> {
    let mut values = Vec::new();
    let mut value = String::new();
    let mut chars = line.chars().peekable();
    let mut quoted = false;
    while let Some(ch) = chars.next() {
        match ch {
            '"' if quoted && chars.peek() == Some(&'"') => {
                value.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ch if ch == delimiter && !quoted => {
                values.push(value.trim().to_string());
                value.clear();
            }
            _ => value.push(ch),
        }
    }
    if quoted {
        return Err("引号没有闭合".into());
    }
    values.push(value.trim().to_string());
    Ok(values)
}

fn parse_catalog(source: &str) -> AppResult<Vec<CatalogRow>> {
    let mut parsed = Vec::new();
    for (index, line) in source.lines().enumerate() {
        let line = line.trim().trim_start_matches('\u{feff}');
        if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
            continue;
        }
        let delimiter = if line.contains('\t') { '\t' } else { ',' };
        let cols = split_catalog_line(line, delimiter).map_err(|message| {
            AppError::InvalidInput(format!("建档表第 {} 行：{message}", index + 1))
        })?;
        if cols.get(2).is_some_and(|value| value.contains("SKU编码")) {
            continue;
        }
        if cols.len() < 4 {
            return Err(AppError::InvalidInput(format!(
                "建档表第 {} 行至少需要 4 列：所属商品编码、商品名、SKU编码、SKU名",
                index + 1
            )));
        }
        let product_code = product::validate_code(&cols[0])?;
        if !paths::is_valid_sku_code(&cols[2]) {
            return Err(AppError::InvalidInput(format!(
                "建档表第 {} 行 SKU 编码非法",
                index + 1
            )));
        }
        if cols[3].is_empty() {
            return Err(AppError::InvalidInput(format!(
                "建档表第 {} 行 SKU 名称不能为空",
                index + 1
            )));
        }
        let tier = tier_code(cols.get(4).map(String::as_str).unwrap_or(""))
            .ok_or_else(|| AppError::InvalidInput(format!("建档表第 {} 行层级非法", index + 1)))?;
        parsed.push(CatalogRow {
            product_code,
            product_name: cols[1].trim().to_string(),
            sku_code: cols[2].trim().to_string(),
            sku_name: cols[3].trim().to_string(),
            tier: tier.to_string(),
            folder_alias: cols.get(5).cloned().unwrap_or_default(),
            music_keyword: cols.get(6).cloned().unwrap_or_default(),
        });
    }
    if parsed.is_empty() {
        return Err(AppError::InvalidInput("建档表没有数据行".into()));
    }
    Ok(parsed)
}

async fn apply_catalog(
    pool: &SqlitePool,
    parsed: Vec<CatalogRow>,
) -> AppResult<CatalogImportReport> {
    let mut tx = pool.begin().await?;
    let now = crate::db::now_unix();
    let mut report = CatalogImportReport {
        rows: 0,
        products_created: 0,
        skus_created: 0,
        skus_updated: 0,
    };
    for row in parsed {
        let product_id: i64 = match sqlx::query_scalar::<_, i64>(
            "SELECT id FROM products WHERE code=?1 COLLATE NOCASE",
        )
        .bind(&row.product_code)
        .fetch_optional(&mut *tx)
        .await?
        {
            Some(id) => id,
            None => {
                report.products_created += 1;
                sqlx::query_scalar(
                    "INSERT INTO products(code,name,platforms_json,created_at,updated_at)
                     VALUES(?1,?2,'[\"douyin\",\"xhs\",\"kuaishou\",\"shipinhao\"]',?3,?3)
                     RETURNING id",
                )
                .bind(&row.product_code)
                .bind(if row.product_name.is_empty() {
                    &row.product_code
                } else {
                    &row.product_name
                })
                .bind(now)
                .fetch_one(&mut *tx)
                .await?
            }
        };
        let existing: Option<(i64, i64)> =
            sqlx::query_as("SELECT id,is_general FROM skus WHERE code=?1 COLLATE NOCASE")
                .bind(&row.sku_code)
                .fetch_optional(&mut *tx)
                .await?;
        if let Some((id, is_general)) = existing {
            if is_general != 0 {
                return Err(AppError::InvalidInput(format!(
                    "SKU 编码 {} 与系统通用 SKU 冲突",
                    row.sku_code
                )));
            }
            if repo::sku_reassign_blocked(&mut tx, id, Some(product_id)).await? {
                return Err(AppError::InvalidInput(format!(
                    "SKU {} 正被未关闭任务单引用，不能由目录导入跨商品改挂",
                    row.sku_code
                )));
            }
            sqlx::query(
                "UPDATE skus SET product_id=?2,style_name=?3,tier=?4,folder_alias=?5,
                 music_keyword=?6,updated_at=?7 WHERE id=?1",
            )
            .bind(id)
            .bind(product_id)
            .bind(&row.sku_name)
            .bind(&row.tier)
            .bind(&row.folder_alias)
            .bind(&row.music_keyword)
            .bind(now)
            .execute(&mut *tx)
            .await?;
            report.skus_updated += 1;
        } else {
            sqlx::query(
                "INSERT INTO skus(code,style_name,product_name,tier,topics_json,status,is_general,note,
                 created_at,updated_at,folder_alias,product_id,music_keyword)
                 VALUES(?1,?2,'',?3,'[]','active',0,'',?4,?4,?5,?6,?7)",
            )
            .bind(&row.sku_code)
            .bind(&row.sku_name)
            .bind(&row.tier)
            .bind(now)
            .bind(&row.folder_alias)
            .bind(product_id)
            .bind(&row.music_keyword)
            .execute(&mut *tx)
            .await?;
            report.skus_created += 1;
        }
        report.rows += 1;
    }
    tx.commit().await?;
    Ok(report)
}

/// 导入列：所属商品编码、所属商品名称、SKU编码、SKU名称、层级、文件夹别名、音乐关键词。
#[tauri::command]
#[specta::specta]
pub async fn import_product_catalog(
    state: State<'_, AppState>,
    path: String,
) -> AppResult<CatalogImportReport> {
    let source = std::fs::read_to_string(&path)
        .map_err(|err| AppError::InvalidInput(format!("建档表必须是 UTF-8：{err}")))?;
    apply_catalog(&state.db, parse_catalog(&source)?).await
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // 测试断言失败即测试失败
mod tests {
    use super::*;
    use crate::db::test_support::test_pool;

    #[test]
    fn catalog_parser_supports_quoted_csv_and_tsv() {
        let csv = "所属商品编码,所属商品名称,SKU编码,SKU名称,层级,文件夹别名,音乐关键词\n\
                   A,\"挂件,音乐款\",A-1,黄星款,热,黄星,黄星";
        let rows = parse_catalog(csv).unwrap();
        assert_eq!(rows[0].product_name, "挂件,音乐款");
        assert_eq!(rows[0].tier, "hot");

        let tsv = "B\t商品 B\tB-1\t蓝色款\tcold\t蓝色\t蓝星";
        assert_eq!(parse_catalog(tsv).unwrap()[0].tier, "cold");
    }

    #[tokio::test]
    async fn catalog_import_assigns_each_sku_to_its_unique_product() {
        let (pool, _dir) = test_pool().await;
        let rows = parse_catalog("A,商品 A,A-1,款式 A,hot\nB,商品 B,B-1,款式 B,warm").unwrap();
        let report = apply_catalog(&pool, rows).await.unwrap();
        assert_eq!((report.products_created, report.skus_created), (2, 2));
        let assigned: Vec<(String, String)> = sqlx::query_as(
            "SELECT s.code,p.code FROM skus s JOIN products p ON p.id=s.product_id
             WHERE s.code IN ('A-1','B-1') ORDER BY s.code",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            assigned,
            vec![("A-1".into(), "A".into()), ("B-1".into(), "B".into())]
        );
        let platforms: Vec<String> =
            sqlx::query_scalar("SELECT platforms_json FROM products ORDER BY code")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert!(platforms.iter().all(|value| {
            value.contains("douyin")
                && value.contains("xhs")
                && value.contains("shipinhao")
                && value.contains("kuaishou")
        }));
    }
}
