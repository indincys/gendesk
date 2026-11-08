//! 参考图数据仓。

use sqlx::{FromRow, SqliteConnection, SqlitePool};

use crate::db::now_unix;

#[derive(Debug, Clone, FromRow)]
pub struct RefImageRow {
    pub id: i64,
    pub name: String,
    pub file_path: String,
    pub thumb_path: String,
    pub width: i64,
    pub height: i64,
    pub deleted_at: Option<i64>,
    /// 最近一次挂靠的提示词组（E32 挂靠记忆）。
    pub last_group_id: Option<i64>,
    /// 归档时间（0016）：非空表示已归档，生成页选择器默认不再列出。
    pub archived_at: Option<i64>,
    /// 临时上传（0019）：生成页随手上传的图，只作本批附件，不进长期图库。
    pub ephemeral: bool,
}

pub struct NewRefImage {
    pub name: String,
    pub file_path: String,
    pub thumb_path: String,
    pub width: i64,
    pub height: i64,
    pub file_size: i64,
    pub content_hash: Option<String>,
    pub upload_path: Option<String>,
    /// true = 生成页临时上传，不进图库列表、不参与去重基准。
    pub ephemeral: bool,
}

pub async fn insert<'e, E: sqlx::Executor<'e, Database = sqlx::Sqlite>>(
    pool: E,
    r: &NewRefImage,
) -> Result<i64, sqlx::Error> {
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO ref_images (name, file_path, thumb_path, width, height, file_size,
            content_hash, upload_path, ephemeral, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) RETURNING id",
    )
    .bind(&r.name)
    .bind(&r.file_path)
    .bind(&r.thumb_path)
    .bind(r.width)
    .bind(r.height)
    .bind(r.file_size)
    .bind(&r.content_hash)
    .bind(&r.upload_path)
    .bind(r.ephemeral)
    .bind(now_unix())
    .fetch_one(pool)
    .await?;
    Ok(id)
}

pub async fn list_active(pool: &SqlitePool) -> Result<Vec<RefImageRow>, sqlx::Error> {
    sqlx::query_as::<_, RefImageRow>(
        "SELECT * FROM ref_images WHERE deleted_at IS NULL ORDER BY created_at DESC, id DESC",
    )
    .fetch_all(pool)
    .await
}

/// 归档 / 取消归档一张参考图（0016）。归档只影响生成页选择器可见性，图仍在库里。
#[cfg(test)]
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

// ---------- 图库分组（0019） ----------

pub async fn get(pool: &SqlitePool, id: i64) -> Result<Option<RefImageRow>, sqlx::Error> {
    sqlx::query_as::<_, RefImageRow>("SELECT * FROM ref_images WHERE id = ?1")
        .bind(id)
        .fetch_optional(pool)
        .await
}

// 更新参考图文件的全部快照列（路径/尺寸/大小/hash/上传副本），字段即参数，无需聚合结构体。
