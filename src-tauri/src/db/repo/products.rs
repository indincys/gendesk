//! 商品与商品下 SKU 数据仓。

use sqlx::{FromRow, SqliteConnection, SqlitePool};

use crate::db::now_unix;

#[derive(Debug, Clone, FromRow)]
pub struct ProductRow {
    pub id: i64,
    pub code: String,
    pub name: String,
    pub platforms_json: String,
    pub cart_enabled: i64,
    pub douyin_product_url: String,
    pub douyin_short_title: String,
    pub status: String,
    pub note: String,
}

#[derive(Debug, Clone, FromRow)]
pub struct ProductSkuRow {
    pub id: i64,
    pub product_id: Option<i64>,
    pub code: String,
    pub style_name: String,
    pub tier: String,
    pub status: String,
    pub folder_alias: String,
    pub music_keyword: String,
    pub free_images: i64,
}

pub async fn list(pool: &SqlitePool) -> Result<Vec<ProductRow>, sqlx::Error> {
    sqlx::query_as::<_, ProductRow>(
        "SELECT id,code,name,platforms_json,cart_enabled,douyin_product_url,douyin_short_title,status,note
         FROM products ORDER BY code COLLATE NOCASE,id",
    )
        .fetch_all(pool)
        .await
}

pub async fn get(pool: &SqlitePool, id: i64) -> Result<Option<ProductRow>, sqlx::Error> {
    sqlx::query_as::<_, ProductRow>(
        "SELECT id,code,name,platforms_json,cart_enabled,douyin_product_url,douyin_short_title,status,note
         FROM products WHERE id=?1",
    )
        .bind(id)
        .fetch_optional(pool)
        .await
}

#[allow(clippy::too_many_arguments)] // 与 products 的用户可编辑列一一对应
pub async fn insert(
    pool: &SqlitePool,
    code: &str,
    name: &str,
    platforms_json: &str,
    cart_enabled: bool,
    product_url: &str,
    short_title: &str,
    note: &str,
) -> Result<i64, sqlx::Error> {
    let now = now_unix();
    sqlx::query_scalar(
        "INSERT INTO products
         (code,name,platforms_json,cart_enabled,douyin_product_url,douyin_short_title,note,created_at,updated_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?8) RETURNING id",
    )
    .bind(code)
    .bind(name)
    .bind(platforms_json)
    .bind(cart_enabled as i64)
    .bind(product_url)
    .bind(short_title)
    .bind(note)
    .bind(now)
    .fetch_one(pool)
    .await
}

#[allow(clippy::too_many_arguments)] // 与 products 的用户可编辑列一一对应
pub async fn update(
    pool: &SqlitePool,
    id: i64,
    name: &str,
    platforms_json: &str,
    cart_enabled: bool,
    product_url: &str,
    short_title: &str,
    status: &str,
    note: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE products SET name=?2,platforms_json=?3,cart_enabled=?4,
         douyin_product_url=?5,douyin_short_title=?6,status=?7,note=?8,updated_at=?9 WHERE id=?1",
    )
    .bind(id)
    .bind(name)
    .bind(platforms_json)
    .bind(cart_enabled as i64)
    .bind(product_url)
    .bind(short_title)
    .bind(status)
    .bind(note)
    .bind(now_unix())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn list_skus(
    pool: &SqlitePool,
    product_id: Option<i64>,
) -> Result<Vec<ProductSkuRow>, sqlx::Error> {
    sqlx::query_as::<_, ProductSkuRow>(
        "SELECT s.id,s.product_id,s.code,s.style_name,s.tier,s.status,s.folder_alias,s.music_keyword,
           (SELECT COUNT(*) FROM image_assets a WHERE a.sku_id=s.id AND a.state='free') AS free_images
         FROM skus s
         WHERE s.is_general=0 AND (?1 IS NULL OR s.product_id=?1)
         ORDER BY s.product_id IS NULL DESC, s.code COLLATE NOCASE, s.id",
    )
    .bind(product_id)
    .fetch_all(pool)
    .await
}

pub async fn assign_skus(
    conn: &mut SqliteConnection,
    product_id: i64,
    sku_ids: &[i64],
) -> Result<u64, sqlx::Error> {
    let mut changed = 0;
    for sku_id in sku_ids {
        changed +=
            sqlx::query("UPDATE skus SET product_id=?2,updated_at=?3 WHERE id=?1 AND is_general=0")
                .bind(sku_id)
                .bind(product_id)
                .bind(now_unix())
                .execute(&mut *conn)
                .await?
                .rows_affected();
    }
    Ok(changed)
}

/// SKU 跨商品改挂会改变任务包动态 JOIN 到的 SKU 信息。只要仍被未关闭发布链路
/// 引用，或素材/文案处于 held/used，就必须阻止改挂。
pub async fn sku_reassign_blocked(
    conn: &mut SqliteConnection,
    sku_id: i64,
    target_product_id: Option<i64>,
) -> Result<bool, sqlx::Error> {
    let current: Option<Option<i64>> =
        sqlx::query_scalar("SELECT product_id FROM skus WHERE id=?1 AND is_general=0")
            .bind(sku_id)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(current_product_id) = current else {
        return Ok(false);
    };
    if current_product_id == target_product_id {
        return Ok(false);
    }
    sqlx::query_scalar(
        "SELECT EXISTS(
           SELECT 1 FROM image_assets a
           WHERE a.sku_id=?1 AND (
             a.state IN ('held','used') OR EXISTS(
               SELECT 1 FROM post_images pi JOIN posts p ON p.id=pi.post_id
               JOIN task_sheets ts ON ts.id=p.sheet_id
               WHERE pi.asset_id=a.id AND ts.status!='closed'
             )
           )
           UNION ALL
           SELECT 1 FROM text_items t WHERE t.sku_id=?1 AND t.state IN ('held','used')
         )",
    )
    .bind(sku_id)
    .fetch_one(&mut *conn)
    .await
}

pub async fn update_sku_publish_fields(
    conn: &mut SqliteConnection,
    id: i64,
    product_id: Option<i64>,
    tier: &str,
    music_keyword: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE skus SET product_id=?2,tier=?3,music_keyword=?4,updated_at=?5 WHERE id=?1 AND is_general=0",
    )
    .bind(id)
    .bind(product_id)
    .bind(tier)
    .bind(music_keyword)
    .bind(now_unix())
    .execute(conn)
    .await?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // 测试断言失败即测试失败
mod tests {
    use super::*;
    use crate::db::test_support::test_pool;

    #[tokio::test]
    async fn free_legacy_copy_follows_reassignment_but_held_asset_blocks_it() {
        let (pool, _dir) = test_pool().await;
        for (id, code) in [(1, "A"), (2, "B")] {
            sqlx::query(
                "INSERT INTO products(id,code,name,created_at,updated_at) VALUES(?1,?2,?2,0,0)",
            )
            .bind(id)
            .bind(code)
            .execute(&pool)
            .await
            .unwrap();
        }
        sqlx::query(
            "INSERT INTO skus(id,code,style_name,product_name,tier,topics_json,status,is_general,
             note,created_at,updated_at,folder_alias,product_id,music_keyword)
             VALUES(10,'A-1','款式','','hot','[]','active',0,'',0,0,'',1,'')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO text_items(sku_id,product_id,kind,text,source,state,created_at)
             VALUES(10,NULL,'title','旧标题','manual','free',0)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let mut tx = pool.begin().await.unwrap();
        assert!(!sku_reassign_blocked(&mut tx, 10, Some(2)).await.unwrap());
        assign_skus(&mut tx, 2, &[10]).await.unwrap();
        tx.commit().await.unwrap();
        let product_id: i64 =
            sqlx::query_scalar("SELECT product_id FROM text_items WHERE sku_id=10")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(product_id, 2);

        sqlx::query(
            "INSERT INTO image_assets(sku_id,path_rel,thumb_rel,source,state,created_at,updated_at)
             VALUES(10,'图片素材库/A-1/held.jpg','图片素材库/A-1/held.jpg','import','held',0,0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let mut tx = pool.begin().await.unwrap();
        assert!(sku_reassign_blocked(&mut tx, 10, Some(1)).await.unwrap());
        tx.rollback().await.unwrap();
    }
}
