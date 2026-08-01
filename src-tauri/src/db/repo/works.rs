//! 通过作品数据仓（accepted_works，快照式冗余）。

#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqliteConnection, SqlitePool};

use crate::db::now_unix;

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct AcceptedWorkRow {
    pub id: i64,
    /// 0008 起可空：批次被清理后作品仍在，只是不再指向任何任务。
    /// 提示词成为消耗品之后这会是**常态**而不是例外。
    pub task_id: Option<i64>,
    pub image_path: String,
    pub thumb_path: String,
    pub prompt_id: Option<i64>,
    pub prompt_text: String,
    pub group_id: Option<i64>,
    pub ref_image_id: Option<i64>,
    pub batch_id: Option<i64>,
    pub favorite: i64,
    pub accepted_at: i64,
    /// 编号与组名的快照（0027）。提示词是消耗品，会随批次一起删掉——
    /// 现读 prompts/prompt_groups 的话，作品会在上游被清理的那一刻丢掉自己的身份。
    #[serde(default)]
    pub prompt_code: String,
    #[serde(default)]
    pub group_name: String,
    #[serde(default)]
    pub sku_id: Option<i64>,
}

pub struct NewWork {
    pub task_id: i64,
    pub image_path: String,
    pub thumb_path: String,
    pub prompt_id: i64,
    pub prompt_text: String,
    pub group_id: Option<i64>,
    pub ref_image_id: i64,
    pub batch_id: i64,
    pub prompt_code: String,
    pub group_name: String,
    pub sku_id: Option<i64>,
}

pub async fn insert(conn: &mut SqliteConnection, w: &NewWork) -> Result<i64, sqlx::Error> {
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO accepted_works (task_id, image_path, thumb_path, prompt_id, prompt_text,
            group_id, ref_image_id, batch_id, accepted_at, prompt_code, group_name, sku_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12) RETURNING id",
    )
    .bind(w.task_id)
    .bind(&w.image_path)
    .bind(&w.thumb_path)
    .bind(w.prompt_id)
    .bind(&w.prompt_text)
    .bind(w.group_id)
    .bind(w.ref_image_id)
    .bind(w.batch_id)
    .bind(now_unix())
    .bind(&w.prompt_code)
    .bind(&w.group_name)
    .bind(w.sku_id)
    .fetch_one(&mut *conn)
    .await?;
    Ok(id)
}

/// 整行 → 废纸篓还原载荷（0027）。序列化失败返回 None：那只会让这一条无法还原，
/// 不该连带把「删除」这个动作一起弄失败。
pub fn to_payload(row: &AcceptedWorkRow) -> Option<String> {
    serde_json::to_string(row).ok()
}

/// 从废纸篓载荷把作品写回原位（0027）。
///
/// **连 id 一起写回**：v2v_clips.work_id 是不设 FK 的锚点（0020），换个新 id 等于把
/// 那条视频认领给了别人。id 在删除时就空出来了，除非期间有人手工塞了行，那种情况
/// INSERT 会自己撞主键失败——比静默换 id 好。
///
/// 返回 `true` = `task_id` 因为原任务已经不在而被写成了 NULL。
///
/// `task_id` 是这张表上唯一的外键，而它指向的任务**会被删掉**：批次跑完就退休
/// （v0.21.0：提示词是消耗品），任务随之级联消失。快照本来就是为「上游消失」而生的
/// —— 让整条还原因为一个已经退休的任务而永久失败，是把安全网换成了绊索。
/// 编号与组名在 0027 已冗余进本行，作品照样答得出自己是谁。
pub async fn restore(pool: &SqlitePool, row: &AcceptedWorkRow) -> Result<bool, sqlx::Error> {
    let task_id = match row.task_id {
        Some(t) => {
            let alive: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE id = ?1")
                .bind(t)
                .fetch_one(pool)
                .await?;
            if alive > 0 {
                Some(t)
            } else {
                None
            }
        }
        None => None,
    };
    let dropped = row.task_id.is_some() && task_id.is_none();
    sqlx::query(
        "INSERT INTO accepted_works (id, task_id, image_path, thumb_path, prompt_id, prompt_text,
            group_id, ref_image_id, batch_id, favorite, accepted_at, prompt_code, group_name, sku_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
    )
    .bind(row.id)
    .bind(task_id)
    .bind(&row.image_path)
    .bind(&row.thumb_path)
    .bind(row.prompt_id)
    .bind(&row.prompt_text)
    .bind(row.group_id)
    .bind(row.ref_image_id)
    .bind(row.batch_id)
    .bind(row.favorite)
    .bind(row.accepted_at)
    .bind(&row.prompt_code)
    .bind(&row.group_name)
    .bind(row.sku_id)
    .execute(pool)
    .await?;
    Ok(dropped)
}

pub async fn get(pool: &SqlitePool, id: i64) -> Result<Option<AcceptedWorkRow>, sqlx::Error> {
    sqlx::query_as::<_, AcceptedWorkRow>("SELECT * FROM accepted_works WHERE id = ?1")
        .bind(id)
        .fetch_optional(pool)
        .await
}

pub async fn toggle_favorite(pool: &SqlitePool, id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE accepted_works SET favorite = 1 - favorite WHERE id = ?1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// 删除作品记录，返回其行（供上层搬运文件进废纸篓）。
pub async fn delete(pool: &SqlitePool, id: i64) -> Result<Option<AcceptedWorkRow>, sqlx::Error> {
    let row = get(pool, id).await?;
    if row.is_some() {
        sqlx::query("DELETE FROM accepted_works WHERE id = ?1")
            .bind(id)
            .execute(pool)
            .await?;
    }
    Ok(row)
}

pub async fn count(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM accepted_works")
        .fetch_one(pool)
        .await
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即失败
mod tests {
    use super::*;
    use crate::db::test_support::test_pool;

    fn row(task_id: Option<i64>) -> AcceptedWorkRow {
        AcceptedWorkRow {
            id: 1,
            task_id,
            image_path: "/o.jpg".into(),
            thumb_path: "/t.jpg".into(),
            prompt_id: Some(1),
            prompt_text: "原文".into(),
            group_id: Some(1),
            ref_image_id: Some(1),
            batch_id: Some(7),
            favorite: 0,
            accepted_at: 100,
            prompt_code: "GG-0001".into(),
            group_name: "g".into(),
            sku_id: None,
        }
    }

    /// 原任务已经退休时，还原**照样成立**，只是 task_id 写成 NULL。
    ///
    /// `task_id` 是这张表唯一的外键，而它指向的任务会被删掉：批次跑完就退休
    /// （v0.21.0：提示词是消耗品）。原来这里无条件写回原值 —— 一个已退休的任务
    /// 会让 INSERT 撞外键，于是那张作品**永远**还原不回来。快照本来就是为
    /// 「上游消失」而生的，让它变成绊索是把安全网用反了。
    #[tokio::test]
    async fn restoring_a_work_survives_its_task_having_been_retired() {
        let (pool, _d) = test_pool().await;
        let dropped = restore(&pool, &row(Some(999))).await.unwrap();
        assert!(dropped, "要如实告诉调用方这条连线断了");
        let back = get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(back.task_id, None);
        assert_eq!(back.prompt_code, "GG-0001", "身份由 0027 的快照列回答");
        assert_eq!(back.group_name, "g");
    }

    /// 任务还在就原样连回去 —— 这条链只在不得已时才断。
    #[tokio::test]
    async fn restoring_keeps_the_task_link_when_the_task_is_still_there() {
        let (pool, _d) = test_pool().await;
        let mut tx = pool.begin().await.unwrap();
        let bid = crate::db::repo::tasks::create_batch(&mut tx, "/out", "{}")
            .await
            .unwrap();
        sqlx::query("INSERT INTO prompt_groups (id,name,prefix,scene,is_temp,created_at) VALUES (1,'g','GG','',0,0)").execute(&mut *tx).await.unwrap();
        sqlx::query("INSERT INTO prompts (id,group_id,code,text,status,source,created_at,updated_at) VALUES (1,1,'GG-0001','t','active','library',0,0)").execute(&mut *tx).await.unwrap();
        sqlx::query("INSERT INTO ref_images (id,name,file_path,thumb_path,width,height,file_size,created_at) VALUES (1,'r','/a','/t',1,1,1,0)").execute(&mut *tx).await.unwrap();
        let tid = crate::db::repo::tasks::insert_task(&mut tx, bid, 1, 1, "t", 1)
            .await
            .unwrap();
        tx.commit().await.unwrap();

        let dropped = restore(&pool, &row(Some(tid))).await.unwrap();
        assert!(!dropped);
        assert_eq!(get(&pool, 1).await.unwrap().unwrap().task_id, Some(tid));
    }
}
