//! 任务单 / 日内容套装 / 发布任务数据仓（task_sheets · daily_sets · publish_tasks）。
//! P2 编排与 P3 对账消费；P1 建骨架就位（row 结构与基础读）。

#![allow(dead_code)]

use sqlx::{FromRow, SqlitePool};

#[derive(Debug, Clone, FromRow)]
pub struct SheetRow {
    pub id: i64,
    pub date: String,
    pub status: String,
    pub shortage_json: String,
    pub report_json: Option<String>,
    pub exported_at: Option<i64>,
    pub closed_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, FromRow)]
pub struct DailySetRow {
    pub id: i64,
    pub date: String,
    pub sku_id: i64,
    pub pack_id: i64,
    pub title_id: i64,
    pub body_id: Option<i64>,
    pub account_id: Option<i64>,
}

#[derive(Debug, Clone, FromRow)]
pub struct PublishTaskRow {
    pub id: i64,
    pub sheet_id: i64,
    pub task_code: String,
    pub set_id: i64,
    pub account_id: i64,
    pub platform: String,
    pub content_kind: String,
    pub planned_time: Option<String>,
    pub status: String,
    pub fail_kind: Option<String>,
    pub result_url: Option<String>,
    pub result_msg: Option<String>,
    pub result_time: Option<i64>,
    pub screenshot: Option<String>,
    pub updated_at: i64,
}

pub async fn get_sheet_by_date(
    pool: &SqlitePool,
    date: &str,
) -> Result<Option<SheetRow>, sqlx::Error> {
    sqlx::query_as::<_, SheetRow>("SELECT * FROM task_sheets WHERE date = ?1")
        .bind(date)
        .fetch_optional(pool)
        .await
}

pub async fn list_sheets(pool: &SqlitePool) -> Result<Vec<SheetRow>, sqlx::Error> {
    sqlx::query_as::<_, SheetRow>("SELECT * FROM task_sheets ORDER BY date DESC")
        .fetch_all(pool)
        .await
}
