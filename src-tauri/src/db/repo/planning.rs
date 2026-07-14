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

pub async fn get_sheet(pool: &SqlitePool, id: i64) -> Result<Option<SheetRow>, sqlx::Error> {
    sqlx::query_as::<_, SheetRow>("SELECT * FROM task_sheets WHERE id = ?1")
        .bind(id)
        .fetch_optional(pool)
        .await
}

use sqlx::SqliteConnection;

/// 创建任务单（草稿），返回 id。
pub async fn create_sheet(conn: &mut SqliteConnection, date: &str) -> Result<i64, sqlx::Error> {
    let now = crate::db::now_unix();
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO task_sheets (date, created_at, updated_at) VALUES (?1, ?2, ?2) RETURNING id",
    )
    .bind(date)
    .bind(now)
    .fetch_one(&mut *conn)
    .await
}

/// 清空任务单的行与套装（草稿重生成用）。
pub async fn clear_sheet_children(
    conn: &mut SqliteConnection,
    sheet_id: i64,
    date: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM publish_tasks WHERE sheet_id = ?1")
        .bind(sheet_id)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM daily_sets WHERE date = ?1")
        .bind(date)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// 写入缺料清单 JSON。
pub async fn set_shortage(
    conn: &mut SqliteConnection,
    sheet_id: i64,
    json: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE task_sheets SET shortage_json = ?2, updated_at = ?3 WHERE id = ?1")
        .bind(sheet_id)
        .bind(json)
        .bind(crate::db::now_unix())
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// 设置任务单状态（+ 可选 exported_at/closed_at 时间戳）。
pub async fn set_sheet_status(
    conn: &mut SqliteConnection,
    sheet_id: i64,
    status: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE task_sheets SET status = ?2, updated_at = ?3,
            exported_at = CASE WHEN ?2 = 'exported' THEN ?3 ELSE exported_at END,
            closed_at   = CASE WHEN ?2 = 'closed'   THEN ?3 ELSE closed_at END
         WHERE id = ?1",
    )
    .bind(sheet_id)
    .bind(status)
    .bind(crate::db::now_unix())
    .execute(&mut *conn)
    .await?;
    Ok(())
}

pub struct NewDailySet {
    pub date: String,
    pub sku_id: i64,
    pub pack_id: i64,
    pub title_id: i64,
    pub body_id: Option<i64>,
}

pub async fn insert_daily_set(
    conn: &mut SqliteConnection,
    s: &NewDailySet,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO daily_sets (date, sku_id, pack_id, title_id, body_id)
         VALUES (?1, ?2, ?3, ?4, ?5) RETURNING id",
    )
    .bind(&s.date)
    .bind(s.sku_id)
    .bind(s.pack_id)
    .bind(s.title_id)
    .bind(s.body_id)
    .fetch_one(&mut *conn)
    .await
}

pub async fn get_daily_set(pool: &SqlitePool, id: i64) -> Result<Option<DailySetRow>, sqlx::Error> {
    sqlx::query_as::<_, DailySetRow>("SELECT * FROM daily_sets WHERE id = ?1")
        .bind(id)
        .fetch_optional(pool)
        .await
}

pub struct NewPublishTask {
    pub sheet_id: i64,
    pub task_code: String,
    pub set_id: i64,
    pub account_id: i64,
    pub platform: String,
    pub content_kind: String,
    pub planned_time: Option<String>,
}

pub async fn insert_publish_task(
    conn: &mut SqliteConnection,
    t: &NewPublishTask,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO publish_tasks
           (sheet_id, task_code, set_id, account_id, platform, content_kind, planned_time, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) RETURNING id",
    )
    .bind(t.sheet_id)
    .bind(&t.task_code)
    .bind(t.set_id)
    .bind(t.account_id)
    .bind(&t.platform)
    .bind(&t.content_kind)
    .bind(&t.planned_time)
    .bind(crate::db::now_unix())
    .fetch_one(&mut *conn)
    .await
}

pub async fn list_tasks_by_sheet(
    pool: &SqlitePool,
    sheet_id: i64,
) -> Result<Vec<PublishTaskRow>, sqlx::Error> {
    sqlx::query_as::<_, PublishTaskRow>(
        "SELECT * FROM publish_tasks WHERE sheet_id = ?1
         ORDER BY (planned_time IS NULL), planned_time ASC, id ASC",
    )
    .bind(sheet_id)
    .fetch_all(pool)
    .await
}

/// 工作台/看板行（publish_tasks 连 daily_sets/skus/text_items/asset_packs/accounts）。
#[derive(Debug, Clone, FromRow)]
pub struct TaskRowJoin {
    pub id: i64,
    pub task_code: String,
    pub set_id: i64,
    pub sku_id: i64,
    pub sku_code: String,
    pub style_name: String,
    pub product_name: String,
    pub title_text: String,
    pub body_text: Option<String>,
    pub topics_json: String,
    pub cover: Option<String>,
    pub dir_rel: String,
    pub pack_kind: String,
    pub files_json: String,
    pub account_id: i64,
    pub account_name: String,
    pub platform: String,
    pub content_kind: String,
    pub planned_time: Option<String>,
    pub status: String,
    pub fail_kind: Option<String>,
    pub result_url: Option<String>,
    pub result_msg: Option<String>,
    pub result_time: Option<i64>,
    pub screenshot: Option<String>,
}

pub async fn sheet_rows(pool: &SqlitePool, sheet_id: i64) -> Result<Vec<TaskRowJoin>, sqlx::Error> {
    sqlx::query_as::<_, TaskRowJoin>(
        "SELECT pt.id, pt.task_code, pt.set_id, ds.sku_id,
                sk.code AS sku_code, sk.style_name, sk.product_name, sk.topics_json,
                ti.text AS title_text, bt.text AS body_text,
                ap.cover, ap.dir_rel, ap.kind AS pack_kind, ap.files_json,
                pt.account_id, ac.name AS account_name,
                pt.platform, pt.content_kind, pt.planned_time, pt.status,
                pt.fail_kind, pt.result_url, pt.result_msg, pt.result_time, pt.screenshot
         FROM publish_tasks pt
         JOIN daily_sets ds ON ds.id = pt.set_id
         JOIN skus sk ON sk.id = ds.sku_id
         JOIN text_items ti ON ti.id = ds.title_id
         LEFT JOIN text_items bt ON bt.id = ds.body_id
         JOIN asset_packs ap ON ap.id = ds.pack_id
         JOIN accounts ac ON ac.id = pt.account_id
         WHERE pt.sheet_id = ?1
         ORDER BY (pt.planned_time IS NULL), pt.planned_time ASC, pt.id ASC",
    )
    .bind(sheet_id)
    .fetch_all(pool)
    .await
}

pub async fn get_task(pool: &SqlitePool, id: i64) -> Result<Option<PublishTaskRow>, sqlx::Error> {
    sqlx::query_as::<_, PublishTaskRow>("SELECT * FROM publish_tasks WHERE id = ?1")
        .bind(id)
        .fetch_optional(pool)
        .await
}

pub async fn update_task_time(
    pool: &SqlitePool,
    id: i64,
    planned_time: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE publish_tasks SET planned_time = ?2, updated_at = ?3 WHERE id = ?1")
        .bind(id)
        .bind(planned_time)
        .bind(crate::db::now_unix())
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn set_task_status(pool: &SqlitePool, id: i64, status: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE publish_tasks SET status = ?2, updated_at = ?3 WHERE id = ?1")
        .bind(id)
        .bind(status)
        .bind(crate::db::now_unix())
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn delete_task(pool: &SqlitePool, id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM publish_tasks WHERE id = ?1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// 任务单内已用的最大日序号（增补行时续号；删行不回收，前置事实 3）。
pub async fn max_task_seq(
    pool: &SqlitePool,
    sheet_id: i64,
    date_yy: &str,
) -> Result<i64, sqlx::Error> {
    // task_code = T{YYMMDD}-{NNN}；取该单已发出的最大 NNN。
    let codes: Vec<String> =
        sqlx::query_scalar("SELECT task_code FROM publish_tasks WHERE sheet_id = ?1")
            .bind(sheet_id)
            .fetch_all(pool)
            .await?;
    let prefix = format!("T{date_yy}-");
    let max = codes
        .iter()
        .filter_map(|c| c.strip_prefix(&prefix))
        .filter_map(|n| n.parse::<i64>().ok())
        .max()
        .unwrap_or(0);
    Ok(max)
}
