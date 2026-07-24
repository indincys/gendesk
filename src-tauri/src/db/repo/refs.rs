//! 参考图数据仓。

// 数据层 API 先于 M3 消费者落地；未使用项在对应里程碑接入后收紧。
#![allow(dead_code)]

use sqlx::{FromRow, SqliteConnection, SqlitePool};

use crate::db::now_unix;

/// 参考图库分组（0019）。与 prompt_groups 无关，图库自己的目录。
#[derive(Debug, Clone, FromRow)]
pub struct RefGroupRow {
    pub id: i64,
    pub name: String,
    pub sort_order: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone, FromRow)]
pub struct RefImageRow {
    pub id: i64,
    pub name: String,
    /// 历史列（0001 指向 prompt_groups）。0019 起不读不写，保留仅为不改旧行。
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
    /// 图库分组（0019）。
    pub ref_group_id: Option<i64>,
    /// 临时上传（0019）：生成页随手上传的图，只作本批附件，不进长期图库。
    pub ephemeral: bool,
}

pub struct NewRefImage {
    pub name: String,
    pub ref_group_id: Option<i64>,
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

pub async fn insert(pool: &SqlitePool, r: &NewRefImage) -> Result<i64, sqlx::Error> {
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO ref_images (name, ref_group_id, file_path, thumb_path, width, height, file_size,
            content_hash, upload_path, ephemeral, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11) RETURNING id",
    )
    .bind(&r.name)
    .bind(r.ref_group_id)
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

/// 库内已有（未删除）参考图的内容 hash 集合（E30b 去重比对）。
pub async fn active_hashes(pool: &SqlitePool) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar::<_, String>(
        "SELECT content_hash FROM ref_images WHERE deleted_at IS NULL AND content_hash IS NOT NULL",
    )
    .fetch_all(pool)
    .await
}

/// 库内 (内容 hash, 名称) 对（E30b 去重弹窗展示重复源）。
///
/// 0019：临时上传（ephemeral）不作基准——它们不进图库，拿它们判重会让用户
/// 正式导入一张自己刚在生成页试过的图时，收到一句莫名其妙的「重复」。
pub async fn active_hash_names(pool: &SqlitePool) -> Result<Vec<(String, String)>, sqlx::Error> {
    sqlx::query_as::<_, (String, String)>(
        "SELECT content_hash, name FROM ref_images
         WHERE deleted_at IS NULL AND ephemeral = 0 AND content_hash IS NOT NULL",
    )
    .fetch_all(pool)
    .await
}

/// 批量改分组（E30b）。0019 起改的是图库分组 `ref_group_id`。
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
        format!("UPDATE ref_images SET ref_group_id = ? WHERE id IN ({ph}) AND deleted_at IS NULL");
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
    sqlx::query("UPDATE ref_images SET ref_group_id = ?2 WHERE id = ?1")
        .bind(id)
        .bind(group_id)
        .execute(pool)
        .await?;
    Ok(())
}

// ---------- 图库分组（0019） ----------

/// 列出全部图库分组（含每组图片数，排除临时上传与已删除）。
pub async fn list_groups(pool: &SqlitePool) -> Result<Vec<(RefGroupRow, i64)>, sqlx::Error> {
    let rows = sqlx::query_as::<_, RefGroupRow>(
        "SELECT * FROM ref_groups ORDER BY sort_order ASC, id ASC",
    )
    .fetch_all(pool)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let n: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM ref_images
             WHERE ref_group_id = ?1 AND deleted_at IS NULL AND ephemeral = 0",
        )
        .bind(r.id)
        .fetch_one(pool)
        .await?;
        out.push((r, n));
    }
    Ok(out)
}

pub async fn create_group(pool: &SqlitePool, name: &str) -> Result<RefGroupRow, sqlx::Error> {
    // 排在末尾。
    let next: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(sort_order), 0) + 1 FROM ref_groups")
        .fetch_one(pool)
        .await?;
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO ref_groups (name, sort_order, created_at) VALUES (?1, ?2, ?3) RETURNING id",
    )
    .bind(name)
    .bind(next)
    .bind(now_unix())
    .fetch_one(pool)
    .await?;
    Ok(RefGroupRow {
        id,
        name: name.to_string(),
        sort_order: next,
        created_at: now_unix(),
    })
}

pub async fn rename_group(pool: &SqlitePool, id: i64, name: &str) -> Result<bool, sqlx::Error> {
    let n = sqlx::query("UPDATE ref_groups SET name = ?2 WHERE id = ?1")
        .bind(id)
        .bind(name)
        .execute(pool)
        .await?
        .rows_affected();
    Ok(n > 0)
}

/// 删除分组。组内图片**不删**，只是回到未分组（FK ON DELETE SET NULL）。
pub async fn delete_group(pool: &SqlitePool, id: i64) -> Result<bool, sqlx::Error> {
    // FK 未必开启（不同连接配置不同），显式置空比依赖级联更可靠。
    sqlx::query("UPDATE ref_images SET ref_group_id = NULL WHERE ref_group_id = ?1")
        .bind(id)
        .execute(pool)
        .await?;
    let n = sqlx::query("DELETE FROM ref_groups WHERE id = ?1")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected();
    Ok(n > 0)
}

/// 按名取分组（NOCASE），供「新建并导入」时避开唯一索引冲突。
pub async fn find_group_by_name(
    pool: &SqlitePool,
    name: &str,
) -> Result<Option<RefGroupRow>, sqlx::Error> {
    sqlx::query_as::<_, RefGroupRow>("SELECT * FROM ref_groups WHERE name = ?1 COLLATE NOCASE")
        .bind(name)
        .fetch_optional(pool)
        .await
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
