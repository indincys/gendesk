//! 参考图数据仓。

// 数据层 API 先于 M3 消费者落地；未使用项在对应里程碑接入后收紧。
#![allow(dead_code)]

use sqlx::{FromRow, SqliteConnection, SqlitePool};

use crate::db::now_unix;

#[derive(Debug, Clone, FromRow)]
pub struct RefImageRow {
    pub id: i64,
    pub name: String,
    pub group_id: Option<i64>,
    pub file_path: String,
    pub thumb_path: String,
    pub width: i64,
    pub height: i64,
    pub file_size: i64,
    pub created_at: i64,
    pub deleted_at: Option<i64>,
    /// 最近一次挂靠的提示词组（E32 挂靠记忆）。
    pub last_group_id: Option<i64>,
    /// 内容 hash（E30b 去重）；历史行为空。
    pub content_hash: Option<String>,
    /// 上传用压缩副本路径（E41）；空表示上传直接用原图。
    pub upload_path: Option<String>,
    /// 归档时间（0016）：非空表示已归档，生成页选择器默认不再列出。
    pub archived_at: Option<i64>,
}

pub struct NewRefImage {
    pub name: String,
    pub group_id: Option<i64>,
    pub file_path: String,
    pub thumb_path: String,
    pub width: i64,
    pub height: i64,
    pub file_size: i64,
    pub content_hash: Option<String>,
    pub upload_path: Option<String>,
}

pub async fn insert(pool: &SqlitePool, r: &NewRefImage) -> Result<i64, sqlx::Error> {
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO ref_images (name, group_id, file_path, thumb_path, width, height, file_size,
            content_hash, upload_path, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) RETURNING id",
    )
    .bind(&r.name)
    .bind(r.group_id)
    .bind(&r.file_path)
    .bind(&r.thumb_path)
    .bind(r.width)
    .bind(r.height)
    .bind(r.file_size)
    .bind(&r.content_hash)
    .bind(&r.upload_path)
    .bind(now_unix())
    .fetch_one(pool)
    .await?;
    Ok(id)
}

/// 库内已有（未删除）参考图的内容 hash 集合（E30b 去重比对）。
pub async fn active_hashes(pool: &SqlitePool) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar::<_, String>(
        "SELECT content_hash FROM ref_images WHERE deleted_at IS NULL AND content_hash IS NOT NULL",
    )
    .fetch_all(pool)
    .await
}

/// 库内 (内容 hash, 名称) 对（E30b 去重弹窗展示重复源）。
pub async fn active_hash_names(pool: &SqlitePool) -> Result<Vec<(String, String)>, sqlx::Error> {
    sqlx::query_as::<_, (String, String)>(
        "SELECT content_hash, name FROM ref_images
         WHERE deleted_at IS NULL AND content_hash IS NOT NULL",
    )
    .fetch_all(pool)
    .await
}

/// 批量改分组（E30b）。
pub async fn set_group_many(
    pool: &SqlitePool,
    ids: &[i64],
    group_id: Option<i64>,
) -> Result<u64, sqlx::Error> {
    if ids.is_empty() {
        return Ok(0);
    }
    let ph = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql =
        format!("UPDATE ref_images SET group_id = ? WHERE id IN ({ph}) AND deleted_at IS NULL");
    let mut q = sqlx::query(&sql).bind(group_id);
    for id in ids {
        q = q.bind(id);
    }
    Ok(q.execute(pool).await?.rows_affected())
}

pub async fn list_active(pool: &SqlitePool) -> Result<Vec<RefImageRow>, sqlx::Error> {
    sqlx::query_as::<_, RefImageRow>(
        "SELECT * FROM ref_images WHERE deleted_at IS NULL ORDER BY created_at DESC, id DESC",
    )
    .fetch_all(pool)
    .await
}

/// 归档 / 取消归档一张参考图（0016）。归档只影响生成页选择器可见性，图仍在库里。
pub async fn set_archived(pool: &SqlitePool, id: i64, archived: bool) -> Result<bool, sqlx::Error> {
    let affected = sqlx::query("UPDATE ref_images SET archived_at = ?2 WHERE id = ?1")
        .bind(id)
        .bind(archived.then(now_unix))
        .execute(pool)
        .await?
        .rows_affected();
    Ok(affected > 0)
}

/// 批量归档（0016）：随批次创建同事务提交，故收 `SqliteConnection`。
pub async fn archive_many(
    conn: &mut SqliteConnection,
    ids: &[i64],
    at: i64,
) -> Result<(), sqlx::Error> {
    for id in ids {
        sqlx::query("UPDATE ref_images SET archived_at = ?2 WHERE id = ?1")
            .bind(id)
            .bind(at)
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}

pub async fn set_group(
    pool: &SqlitePool,
    id: i64,
    group_id: Option<i64>,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE ref_images SET group_id = ?2 WHERE id = ?1")
        .bind(id)
        .bind(group_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn get(pool: &SqlitePool, id: i64) -> Result<Option<RefImageRow>, sqlx::Error> {
    sqlx::query_as::<_, RefImageRow>("SELECT * FROM ref_images WHERE id = ?1")
        .bind(id)
        .fetch_optional(pool)
        .await
}

/// 软删除（deleted_at），返回原行供搬运文件进废纸篓。
pub async fn soft_delete(pool: &SqlitePool, id: i64) -> Result<Option<RefImageRow>, sqlx::Error> {
    let row = get(pool, id).await?;
    if row.is_some() {
        sqlx::query("UPDATE ref_images SET deleted_at = ?2 WHERE id = ?1")
            .bind(id)
            .bind(now_unix())
            .execute(pool)
            .await?;
    }
    Ok(row)
}

// 更新参考图文件的全部快照列（路径/尺寸/大小/hash/上传副本），字段即参数，无需聚合结构体。
#[allow(clippy::too_many_arguments)]
pub async fn update_file(
    pool: &SqlitePool,
    id: i64,
    file_path: &str,
    thumb_path: &str,
    width: i64,
    height: i64,
    file_size: i64,
    content_hash: Option<&str>,
    upload_path: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE ref_images SET file_path = ?2, thumb_path = ?3, width = ?4, height = ?5,
            file_size = ?6, content_hash = ?7, upload_path = ?8 WHERE id = ?1",
    )
    .bind(id)
    .bind(file_path)
    .bind(thumb_path)
    .bind(width)
    .bind(height)
    .bind(file_size)
    .bind(content_hash)
    .bind(upload_path)
    .execute(pool)
    .await?;
    Ok(())
}

/// 记录参考图最近一次挂靠的提示词组（E32）。批次创建时按挂靠更新。
pub async fn set_last_group(
    pool: &SqlitePool,
    ref_id: i64,
    group_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE ref_images SET last_group_id = ?2 WHERE id = ?1")
        .bind(ref_id)
        .bind(group_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// 使用次数（batch_refs）与产出通过作品数（accepted_works）。
pub async fn usage_stats(pool: &SqlitePool, id: i64) -> Result<(i64, i64), sqlx::Error> {
    let used: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM batch_refs WHERE ref_image_id = ?1")
        .bind(id)
        .fetch_one(pool)
        .await?;
    let works: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM accepted_works WHERE ref_image_id = ?1")
            .bind(id)
            .fetch_one(pool)
            .await?;
    Ok((used, works))
}
