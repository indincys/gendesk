//! 废纸篓数据仓（trash_items）。清理 = 物理删文件 + 级联删记录 + 编号回收。

#![allow(dead_code)]

use sqlx::{FromRow, SqliteConnection, SqlitePool};

use crate::db::now_unix;

#[derive(Debug, Clone, FromRow)]
pub struct TrashItemRow {
    pub id: i64,
    pub entity_type: String,
    pub ref_id: Option<i64>,
    pub thumb_path: Option<String>,
    pub prompt_text: Option<String>,
    pub code: Option<String>,
    pub title: Option<String>,
    pub source_label: String,
    pub file_paths_json: String,
    pub deleted_at: i64,
}

pub struct NewTrashItem {
    pub entity_type: String,
    pub ref_id: Option<i64>,
    pub thumb_path: Option<String>,
    pub prompt_text: Option<String>,
    pub code: Option<String>,
    /// 提示词小标题快照（仅 prompt 类；废纸篓按 `编号_小标题` 展示）
    pub title: Option<String>,
    pub source_label: String,
    /// 待清理时物理删除的文件路径列表
    pub file_paths: Vec<String>,
}

pub async fn insert(conn: &mut SqliteConnection, t: &NewTrashItem) -> Result<i64, sqlx::Error> {
    let files = serde_json::to_string(&t.file_paths).unwrap_or_else(|_| "[]".into());
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO trash_items (entity_type, ref_id, thumb_path, prompt_text, code, title,
            source_label, file_paths_json, deleted_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) RETURNING id",
    )
    .bind(&t.entity_type)
    .bind(t.ref_id)
    .bind(&t.thumb_path)
    .bind(&t.prompt_text)
    .bind(&t.code)
    .bind(&t.title)
    .bind(&t.source_label)
    .bind(files)
    .bind(now_unix())
    .fetch_one(&mut *conn)
    .await?;
    Ok(id)
}

pub async fn list(pool: &SqlitePool) -> Result<Vec<TrashItemRow>, sqlx::Error> {
    sqlx::query_as::<_, TrashItemRow>("SELECT * FROM trash_items ORDER BY deleted_at DESC, id DESC")
        .fetch_all(pool)
        .await
}

pub async fn count(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM trash_items")
        .fetch_one(pool)
        .await
}

/// 按 id 取出待清理项（供物理删文件 + 编号回收）。
pub async fn take(pool: &SqlitePool, ids: &[i64]) -> Result<Vec<TrashItemRow>, sqlx::Error> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let ph = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!("SELECT * FROM trash_items WHERE id IN ({ph})");
    let mut q = sqlx::query_as::<_, TrashItemRow>(&sql);
    for id in ids {
        q = q.bind(id);
    }
    q.fetch_all(pool).await
}

pub async fn all(pool: &SqlitePool) -> Result<Vec<TrashItemRow>, sqlx::Error> {
    list(pool).await
}

/// 删除 trash 记录（在事务内，与编号回收同事务）。
pub async fn delete_rows(conn: &mut SqliteConnection, ids: &[i64]) -> Result<(), sqlx::Error> {
    if ids.is_empty() {
        return Ok(());
    }
    let ph = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!("DELETE FROM trash_items WHERE id IN ({ph})");
    let mut q = sqlx::query(&sql);
    for id in ids {
        q = q.bind(id);
    }
    q.execute(&mut *conn).await?;
    Ok(())
}
