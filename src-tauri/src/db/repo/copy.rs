//! 商品级标题/正文与三档话题组数据仓。

use sqlx::{FromRow, SqliteConnection, SqlitePool};

use crate::db::now_unix;

#[derive(Debug, Clone, FromRow)]
pub struct CopyRow {
    pub id: i64,
    pub product_id: i64,
    pub product_code: String,
    pub product_name: String,
    pub kind: String,
    pub text: String,
    pub source: String,
    pub enabled: i64,
    pub state: String,
    pub post_id: Option<i64>,
    pub created_at: i64,
}

#[derive(Debug, Clone, FromRow)]
pub struct TopicGroupRow {
    pub id: i64,
    pub product_id: Option<i64>,
    pub product_name: Option<String>,
    pub scope: String,
    pub sku_ids_json: String,
    pub tags_json: String,
    pub enabled: i64,
    pub created_at: i64,
}

pub async fn list_copy(
    pool: &SqlitePool,
    product_id: Option<i64>,
    kind: &str,
) -> Result<Vec<CopyRow>, sqlx::Error> {
    sqlx::query_as::<_, CopyRow>(
        "SELECT t.id,t.product_id,p.code AS product_code,p.name AS product_name,t.kind,t.text,
                t.source,t.enabled,t.state,t.post_id,t.created_at
         FROM text_items t JOIN products p ON p.id=t.product_id
         WHERE t.kind=?2 AND (?1 IS NULL OR t.product_id=?1)
         ORDER BY p.code COLLATE NOCASE,t.created_at DESC,t.id DESC",
    )
    .bind(product_id)
    .bind(kind)
    .fetch_all(pool)
    .await
}

pub async fn insert_copy(
    conn: &mut SqliteConnection,
    product_id: i64,
    kind: &str,
    text: &str,
    source: &str,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "INSERT INTO text_items(sku_id,product_id,kind,text,platform,source,enabled,use_count,state,created_at)
         VALUES(NULL,?1,?2,?3,'general',?4,1,0,'free',?5) RETURNING id",
    )
    .bind(product_id)
    .bind(kind)
    .bind(text)
    .bind(source)
    .bind(now_unix())
    .fetch_one(&mut *conn)
    .await
}

pub async fn list_topics(
    pool: &SqlitePool,
    product_id: Option<i64>,
) -> Result<Vec<TopicGroupRow>, sqlx::Error> {
    sqlx::query_as::<_, TopicGroupRow>(
        "SELECT g.id,g.product_id,p.name AS product_name,g.scope,g.sku_ids_json,g.tags_json,g.enabled,g.created_at
         FROM topic_groups g LEFT JOIN products p ON p.id=g.product_id
         WHERE (?1 IS NULL OR g.product_id=?1 OR g.scope='general')
         ORDER BY CASE g.scope WHEN 'combo' THEN 0 WHEN 'product' THEN 1 ELSE 2 END,g.id",
    )
    .bind(product_id)
    .fetch_all(pool)
    .await
}
