//! 图片素材库数据仓。

use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqliteConnection, SqlitePool};

use crate::db::now_unix;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ImageAssetRow {
    pub id: i64,
    pub sku_id: i64,
    pub path_rel: String,
    pub thumb_rel: String,
    pub source: String,
    pub work_id: Option<i64>,
    pub state: String,
    pub post_id: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, FromRow)]
pub struct ImageLibraryRow {
    pub id: i64,
    pub sku_id: i64,
    pub sku_code: String,
    pub sku_name: String,
    pub product_id: Option<i64>,
    pub product_name: Option<String>,
    pub path_rel: String,
    pub thumb_rel: String,
    pub source: String,
    pub state: String,
    pub post_id: Option<i64>,
    pub created_at: i64,
}

pub async fn list(
    pool: &SqlitePool,
    product_id: Option<i64>,
    sku_id: Option<i64>,
    state: Option<&str>,
) -> Result<Vec<ImageLibraryRow>, sqlx::Error> {
    sqlx::query_as::<_, ImageLibraryRow>(
        "SELECT a.id,a.sku_id,s.code AS sku_code,s.style_name AS sku_name,s.product_id,
                p.name AS product_name,a.path_rel,a.thumb_rel,a.source,a.state,a.post_id,a.created_at
         FROM image_assets a JOIN skus s ON s.id=a.sku_id LEFT JOIN products p ON p.id=s.product_id
         WHERE (?1 IS NULL OR s.product_id=?1) AND (?2 IS NULL OR a.sku_id=?2)
           AND (?3 IS NULL OR a.state=?3)
         ORDER BY s.code COLLATE NOCASE,a.created_at DESC,a.id DESC",
    )
    .bind(product_id)
    .bind(sku_id)
    .bind(state)
    .fetch_all(pool)
    .await
}

pub async fn get(pool: &SqlitePool, id: i64) -> Result<Option<ImageAssetRow>, sqlx::Error> {
    sqlx::query_as::<_, ImageAssetRow>("SELECT * FROM image_assets WHERE id=?1")
        .bind(id)
        .fetch_optional(pool)
        .await
}

pub async fn insert(
    conn: &mut SqliteConnection,
    sku_id: i64,
    path_rel: &str,
    thumb_rel: &str,
    source: &str,
    work_id: Option<i64>,
) -> Result<i64, sqlx::Error> {
    let now = now_unix();
    sqlx::query_scalar(
        "INSERT INTO image_assets(sku_id,path_rel,thumb_rel,source,work_id,created_at,updated_at)
         VALUES(?1,?2,?3,?4,?5,?6,?6)
         ON CONFLICT(path_rel) DO NOTHING
         RETURNING id",
    )
    .bind(sku_id)
    .bind(path_rel)
    .bind(thumb_rel)
    .bind(source)
    .bind(work_id)
    .bind(now)
    .fetch_one(&mut *conn)
    .await
}

pub async fn set_sku(pool: &SqlitePool, ids: &[i64], sku_id: i64) -> Result<u64, sqlx::Error> {
    let mut changed = 0;
    for id in ids {
        changed += sqlx::query(
            "UPDATE image_assets SET sku_id=?2,updated_at=?3 WHERE id=?1 AND state='free'",
        )
        .bind(id)
        .bind(sku_id)
        .bind(now_unix())
        .execute(pool)
        .await?
        .rows_affected();
    }
    Ok(changed)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // 测试断言失败即测试失败
mod tests {
    use super::*;
    use crate::db::test_support::test_pool;

    async fn seed_sku(pool: &SqlitePool) {
        sqlx::query(
            "INSERT INTO products(id,code,name,created_at,updated_at) VALUES(100,'A','商品 A',0,0)",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO skus(id,code,style_name,product_name,tier,topics_json,status,is_general,
             note,created_at,updated_at,folder_alias,product_id,music_keyword)
             VALUES(100,'A-1','款式','商品 A','hot','[]','active',0,'',0,0,'',100,'')",
        )
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn image_asset_schema_rejects_absolute_paths() {
        let (pool, _dir) = test_pool().await;
        seed_sku(&pool).await;
        for absolute in [
            "/tmp/a.jpg",
            r"C:\\images\\a.jpg",
            r"\\server\\share\\a.jpg",
        ] {
            let result = sqlx::query(
                "INSERT INTO image_assets(sku_id,path_rel,thumb_rel,source,created_at,updated_at)
                 VALUES(100,?1,?1,'import',0,0)",
            )
            .bind(absolute)
            .execute(&pool)
            .await;
            assert!(result.is_err(), "绝对路径必须由数据库约束兜底：{absolute}");
        }

        let mut tx = pool.begin().await.unwrap();
        insert(
            &mut tx,
            100,
            "图片素材库/A-1/a.jpg",
            "图片素材库/A-1/a.jpg",
            "import",
            None,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }

    #[tokio::test]
    async fn path_collision_never_reassigns_or_mutates_held_asset() {
        let (pool, _dir) = test_pool().await;
        seed_sku(&pool).await;
        let mut tx = pool.begin().await.unwrap();
        insert(
            &mut tx,
            100,
            "图片素材库/A-1/fixed.jpg",
            "图片素材库/A-1/fixed.jpg",
            "import",
            None,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        sqlx::query(
            "UPDATE image_assets SET state='held',post_id=99 WHERE path_rel LIKE '%fixed.jpg'",
        )
        .execute(&pool)
        .await
        .unwrap();
        let mut tx = pool.begin().await.unwrap();
        assert!(insert(
            &mut tx,
            100,
            "图片素材库/A-1/fixed.jpg",
            "图片素材库/A-1/other-thumb.jpg",
            "works",
            Some(42),
        )
        .await
        .is_err());
        tx.rollback().await.unwrap();
        let row: (String, String, Option<i64>, Option<i64>) = sqlx::query_as(
            "SELECT state,source,work_id,post_id FROM image_assets WHERE path_rel LIKE '%fixed.jpg'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row, ("held".into(), "import".into(), None, Some(99)));
    }
}
