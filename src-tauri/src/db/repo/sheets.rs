//! 新任务单读模型。

use sqlx::{FromRow, SqlitePool};

#[derive(Debug, Clone, FromRow)]
pub struct SheetRow {
    pub id: i64,
    pub date: String,
    pub product_id: i64,
    pub title: String,
    pub status: String,
    pub export_dir: Option<String>,
    pub shortage_json: String,
}

#[derive(Debug, Clone, FromRow)]
pub struct ConfigRow {
    pub id: i64,
    pub product_id: i64,
    pub sku_scope_json: String,
    pub posts_per_day: i64,
    pub images_per_post: i64,
    pub mixed_count: i64,
    pub target_day: String,
    pub enabled: i64,
}

pub async fn list_configs(pool: &SqlitePool) -> Result<Vec<ConfigRow>, sqlx::Error> {
    sqlx::query_as::<_, ConfigRow>(
        "SELECT id,product_id,sku_scope_json,posts_per_day,images_per_post,mixed_count,target_day,enabled
         FROM sheet_configs ORDER BY product_id,id",
    )
        .fetch_all(pool)
        .await
}

pub async fn list_sheets(pool: &SqlitePool) -> Result<Vec<SheetRow>, sqlx::Error> {
    sqlx::query_as::<_, SheetRow>(
        "SELECT id,date,product_id,title,status,export_dir,shortage_json
         FROM task_sheets ORDER BY date DESC,id DESC",
    )
    .fetch_all(pool)
    .await
}

pub async fn get_sheet(pool: &SqlitePool, id: i64) -> Result<Option<SheetRow>, sqlx::Error> {
    sqlx::query_as::<_, SheetRow>(
        "SELECT id,date,product_id,title,status,export_dir,shortage_json FROM task_sheets WHERE id=?1",
    )
        .bind(id)
        .fetch_optional(pool)
        .await
}
