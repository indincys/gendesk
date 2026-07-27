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
    /// 还原载荷（0027）：行被真删掉的实体（作品）在此存整行快照。
    pub payload_json: Option<String>,
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
    /// 整行快照（0027）。只有「删除即真删行」的实体需要它——task/prompt/ref/clip
    /// 还原时把状态拨回去就行，行一直都在。
    pub payload_json: Option<String>,
}

pub async fn insert(conn: &mut SqliteConnection, t: &NewTrashItem) -> Result<i64, sqlx::Error> {
    let files = serde_json::to_string(&t.file_paths).unwrap_or_else(|_| "[]".into());
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO trash_items (entity_type, ref_id, thumb_path, prompt_text, code, title,
            source_label, file_paths_json, deleted_at, payload_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) RETURNING id",
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
    .bind(&t.payload_json)
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

/// 删除时刻早于 cutoff 的项 id（E40 到期自动清理）。
pub async fn expired_ids(pool: &SqlitePool, cutoff: i64) -> Result<Vec<i64>, sqlx::Error> {
    sqlx::query_scalar::<_, i64>("SELECT id FROM trash_items WHERE deleted_at < ?1")
        .bind(cutoff)
        .fetch_all(pool)
        .await
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

/// 删掉某条 clip 的废纸篓行（重跑/退回改写时收回它）。返回删了几行。
///
/// 视频重跑是**就地**的：`v2v_clips` 只有一行，成片路径锚在 clip id 上
/// （`clips/clip{id}.mp4`）。于是一条被判「不通过」的 clip 重跑之后，新片子会落到
/// 与旧片子**完全相同**的路径，而废纸篓里那行还指着它 —— 下一次清空废纸篓就会
/// 物理删掉一条还活着的成片。收回这一行是两道闸中的第一道。
pub async fn delete_by_clip(conn: &mut SqliteConnection, clip_id: i64) -> Result<u64, sqlx::Error> {
    let n = sqlx::query("DELETE FROM trash_items WHERE entity_type = 'clip' AND ref_id = ?1")
        .bind(clip_id)
        .execute(&mut *conn)
        .await?
        .rows_affected();
    Ok(n)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即失败
mod tests {
    use super::*;
    use crate::db::test_support::test_pool;

    // E40 / D3：仅删除时刻早于 cutoff 的项被判定为到期。
    #[tokio::test]
    async fn expired_ids_selects_only_older_than_cutoff() {
        let (pool, _d) = test_pool().await;
        let now = now_unix();
        // 旧项（40 天前）与新项（1 天前）。
        let old = sqlx::query_scalar::<_, i64>(
            "INSERT INTO trash_items (entity_type, source_label, deleted_at) VALUES ('prompt','x',?1) RETURNING id",
        )
        .bind(now - 40 * 86_400)
        .fetch_one(&pool)
        .await
        .unwrap();
        let _fresh = sqlx::query_scalar::<_, i64>(
            "INSERT INTO trash_items (entity_type, source_label, deleted_at) VALUES ('prompt','x',?1) RETURNING id",
        )
        .bind(now - 86_400)
        .fetch_one(&pool)
        .await
        .unwrap();

        let cutoff = now - 30 * 86_400;
        let ids = expired_ids(&pool, cutoff).await.unwrap();
        assert_eq!(ids, vec![old], "仅 40 天前的项到期，1 天前的项保留");
    }

    // 0027：作品是唯一「删除即真删行」的实体，还原全靠这份整行快照。
    // 它必须能原样往返（尤其是 id —— v2v_clips.work_id 是不设 FK 的锚点，
    // 换个新 id 等于把那条视频认领给了别人）。
    #[tokio::test]
    async fn work_payload_round_trips_through_the_trash() {
        use crate::db::repo::works;
        let (pool, _d) = test_pool().await;
        sqlx::query(
            "INSERT INTO accepted_works (id,task_id,image_path,thumb_path,prompt_id,prompt_text,
                group_id,ref_image_id,batch_id,favorite,accepted_at,prompt_code,group_name)
             VALUES (7,NULL,'/o.jpg','/t.jpg',1,'正文',2,4,5,1,900,'GG-0007','夏日组')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let row = works::delete(&pool, 7).await.unwrap().unwrap();
        let payload = works::to_payload(&row).expect("整行应能序列化");
        assert_eq!(works::count(&pool).await.unwrap(), 0);

        let parsed: works::AcceptedWorkRow = serde_json::from_str(&payload).unwrap();
        works::restore(&pool, &parsed).await.unwrap();

        let back = works::get(&pool, 7).await.unwrap().expect("应还原回原 id");
        assert_eq!(back.prompt_code, "GG-0007", "编号快照不能在往返中丢失");
        assert_eq!(back.group_name, "夏日组");
        assert_eq!(back.favorite, 1, "收藏这类人手动设过的状态也要原样回来");
        assert_eq!(back.accepted_at, 900, "验收时刻不得被改成「现在」");
    }
}
