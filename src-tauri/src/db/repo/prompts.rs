//! 提示词分组 / 提示词数据仓（与参考图库共用分组）。

// 数据层 API 先于 M3 消费者落地；未使用项在对应里程碑接入后收紧。
#![allow(dead_code)]

use sqlx::{FromRow, SqliteConnection, SqlitePool};

use crate::db::now_unix;

#[derive(Debug, Clone, FromRow)]
pub struct GroupRow {
    pub id: i64,
    pub name: String,
    pub prefix: String,
    pub scene: String,
    pub is_temp: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone, FromRow)]
pub struct PromptRow {
    pub id: i64,
    pub group_id: i64,
    pub code: String,
    pub title: Option<String>,
    pub text: String,
    pub favorite: i64,
    pub edited: i64,
    pub status: String,
    pub source: String,
    pub created_at: i64,
    pub updated_at: i64,
}

pub async fn list_groups(pool: &SqlitePool) -> Result<Vec<GroupRow>, sqlx::Error> {
    sqlx::query_as::<_, GroupRow>("SELECT * FROM prompt_groups ORDER BY created_at ASC, id ASC")
        .fetch_all(pool)
        .await
}

pub async fn find_group_by_prefix(
    pool: &SqlitePool,
    prefix: &str,
) -> Result<Option<GroupRow>, sqlx::Error> {
    sqlx::query_as::<_, GroupRow>("SELECT * FROM prompt_groups WHERE prefix = ?1")
        .bind(prefix)
        .fetch_optional(pool)
        .await
}

/// 在事务中创建分组，返回其 id。
pub async fn create_group(
    conn: &mut SqliteConnection,
    name: &str,
    prefix: &str,
    scene: &str,
    is_temp: bool,
) -> Result<i64, sqlx::Error> {
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO prompt_groups (name, prefix, scene, is_temp, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5) RETURNING id",
    )
    .bind(name)
    .bind(prefix)
    .bind(scene)
    .bind(is_temp as i64)
    .bind(now_unix())
    .fetch_one(&mut *conn)
    .await?;
    Ok(id)
}

/// 在事务中插入一条提示词。
pub async fn insert_prompt(
    conn: &mut SqliteConnection,
    group_id: i64,
    code: &str,
    title: Option<&str>,
    text: &str,
    source: &str,
) -> Result<i64, sqlx::Error> {
    let now = now_unix();
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO prompts (group_id, code, title, text, source, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6) RETURNING id",
    )
    .bind(group_id)
    .bind(code)
    .bind(title)
    .bind(text)
    .bind(source)
    .bind(now)
    .fetch_one(&mut *conn)
    .await?;
    Ok(id)
}

/// 分组内全部 active 提示词（id + 正文），供批次展开。
pub async fn list_active_prompts(
    pool: &SqlitePool,
    group_id: i64,
) -> Result<Vec<(i64, String)>, sqlx::Error> {
    sqlx::query_as::<_, (i64, String)>(
        "SELECT id, text FROM prompts WHERE group_id = ?1 AND status = 'active' ORDER BY id ASC",
    )
    .bind(group_id)
    .fetch_all(pool)
    .await
}

/// 验收通过时写回微调文本并标记 edited（R8）。
pub async fn apply_edit(pool: &SqlitePool, prompt_id: i64, text: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE prompts SET text = ?2, edited = 1, updated_at = ?3 WHERE id = ?1")
        .bind(prompt_id)
        .bind(text)
        .bind(crate::db::now_unix())
        .execute(pool)
        .await?;
    Ok(())
}

/// 分组转正式（临时组首次验收通过，R7）。
pub async fn promote_group(pool: &SqlitePool, group_id: i64) -> Result<bool, sqlx::Error> {
    let affected =
        sqlx::query("UPDATE prompt_groups SET is_temp = 0 WHERE id = ?1 AND is_temp = 1")
            .bind(group_id)
            .execute(pool)
            .await?
            .rows_affected();
    Ok(affected > 0)
}

pub async fn list_by_group(
    pool: &SqlitePool,
    group_id: i64,
) -> Result<Vec<PromptRow>, sqlx::Error> {
    sqlx::query_as::<_, PromptRow>(
        "SELECT * FROM prompts WHERE group_id = ?1 AND status = 'active' ORDER BY id ASC",
    )
    .bind(group_id)
    .fetch_all(pool)
    .await
}

pub async fn search(pool: &SqlitePool, query: &str) -> Result<Vec<PromptRow>, sqlx::Error> {
    let like = format!("%{query}%");
    sqlx::query_as::<_, PromptRow>(
        "SELECT * FROM prompts WHERE status = 'active' AND (code LIKE ?1 OR text LIKE ?1)
         ORDER BY id ASC LIMIT 300",
    )
    .bind(like)
    .fetch_all(pool)
    .await
}

pub async fn get(pool: &SqlitePool, id: i64) -> Result<Option<PromptRow>, sqlx::Error> {
    sqlx::query_as::<_, PromptRow>("SELECT * FROM prompts WHERE id = ?1")
        .bind(id)
        .fetch_optional(pool)
        .await
}

pub async fn toggle_favorite(pool: &SqlitePool, id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE prompts SET favorite = 1 - favorite WHERE id = ?1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// 置 trash 状态，返回 (code, title, group_id) 供废纸篓快照与编号回收。
#[allow(clippy::type_complexity)]
pub async fn set_trash(
    pool: &SqlitePool,
    id: i64,
) -> Result<Option<(String, Option<String>, i64)>, sqlx::Error> {
    let row = get(pool, id).await?;
    if let Some(r) = &row {
        sqlx::query("UPDATE prompts SET status = 'trash', updated_at = ?2 WHERE id = ?1")
            .bind(id)
            .bind(crate::db::now_unix())
            .execute(pool)
            .await?;
        return Ok(Some((r.code.clone(), r.title.clone(), r.group_id)));
    }
    Ok(None)
}

pub async fn count_in_group(pool: &SqlitePool, group_id: i64) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM prompts WHERE group_id = ?1 AND status = 'active'",
    )
    .bind(group_id)
    .fetch_one(pool)
    .await
}

pub async fn get_group(pool: &SqlitePool, id: i64) -> Result<Option<GroupRow>, sqlx::Error> {
    sqlx::query_as::<_, GroupRow>("SELECT * FROM prompt_groups WHERE id = ?1")
        .bind(id)
        .fetch_optional(pool)
        .await
}

/// 重命名分组（前缀/编号不变，仅改展示名）。
pub async fn rename_group(pool: &SqlitePool, id: i64, name: &str) -> Result<bool, sqlx::Error> {
    let affected = sqlx::query("UPDATE prompt_groups SET name = ?2 WHERE id = ?1")
        .bind(id)
        .bind(name)
        .execute(pool)
        .await?
        .rows_affected();
    Ok(affected > 0)
}

/// 分组绑定的标签名（V1 标签绑定在 prompt_group 级）。
pub async fn group_tags(pool: &SqlitePool, group_id: i64) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar::<_, String>(
        "SELECT t.name FROM tags t
         JOIN tag_bindings b ON b.tag_id = t.id
         WHERE b.entity_type = 'prompt_group' AND b.entity_id = ?1
         ORDER BY t.name ASC",
    )
    .bind(group_id)
    .fetch_all(pool)
    .await
}

/// 批量移动提示词到指定分组（编号/前缀保留原值不重编，E20/E36）。
pub async fn move_prompts(
    conn: &mut SqliteConnection,
    ids: &[i64],
    group_id: i64,
) -> Result<u64, sqlx::Error> {
    if ids.is_empty() {
        return Ok(0);
    }
    let ph = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!(
        "UPDATE prompts SET group_id = ?, updated_at = ? WHERE id IN ({ph}) AND status = 'active'"
    );
    let mut q = sqlx::query(&sql).bind(group_id).bind(now_unix());
    for id in ids {
        q = q.bind(id);
    }
    Ok(q.execute(&mut *conn).await?.rows_affected())
}

/// 批量设置收藏标记（E36：批量收藏）。
pub async fn set_favorite_many(
    conn: &mut SqliteConnection,
    ids: &[i64],
    favorite: bool,
) -> Result<u64, sqlx::Error> {
    if ids.is_empty() {
        return Ok(0);
    }
    let ph = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!("UPDATE prompts SET favorite = ? WHERE id IN ({ph})");
    let mut q = sqlx::query(&sql).bind(favorite as i64);
    for id in ids {
        q = q.bind(id);
    }
    Ok(q.execute(&mut *conn).await?.rows_affected())
}

/// 合并分组：将 `from` 组的提示词/参考图/挂靠/标签迁移到 `into`，编号前缀保留不重编（E20）。
/// 迁移后 `from` 组已空，调用方随即删除之。
pub async fn merge_into(
    conn: &mut SqliteConnection,
    from: i64,
    into: i64,
) -> Result<(), sqlx::Error> {
    // 提示词（含 trash 态一并迁移，避免删组时级联误删 trash 快照对应行）。
    sqlx::query("UPDATE prompts SET group_id = ?2, updated_at = ?3 WHERE group_id = ?1")
        .bind(from)
        .bind(into)
        .bind(now_unix())
        .execute(&mut *conn)
        .await?;
    // 参考图当前分组 + 挂靠记忆。
    sqlx::query("UPDATE ref_images SET group_id = ?2 WHERE group_id = ?1")
        .bind(from)
        .bind(into)
        .execute(&mut *conn)
        .await?;
    sqlx::query("UPDATE ref_images SET last_group_id = ?2 WHERE last_group_id = ?1")
        .bind(from)
        .bind(into)
        .execute(&mut *conn)
        .await?;
    // 历史批次挂靠（PK 冲突则忽略，残留行随 from 组删除级联清理）。
    sqlx::query("UPDATE OR IGNORE batch_refs SET prompt_group_id = ?2 WHERE prompt_group_id = ?1")
        .bind(from)
        .bind(into)
        .execute(&mut *conn)
        .await?;
    // 标签绑定（同标签已绑 into 时忽略）。
    sqlx::query(
        "UPDATE OR IGNORE tag_bindings SET entity_id = ?2
         WHERE entity_type = 'prompt_group' AND entity_id = ?1",
    )
    .bind(from)
    .bind(into)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// 删除分组行（级联：残余 prompts/batch_refs 删除；ref_images 置空；accepted_works 快照保留）。
pub async fn delete_group(conn: &mut SqliteConnection, id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM prompt_groups WHERE id = ?1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即失败
mod tests {
    use super::*;
    use crate::db::test_support::test_pool;

    async fn seed_group(pool: &SqlitePool, name: &str, prefix: &str) -> i64 {
        let mut tx = pool.begin().await.unwrap();
        let id = create_group(&mut tx, name, prefix, "", false)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        id
    }

    async fn seed_prompt(pool: &SqlitePool, group_id: i64, code: &str) -> i64 {
        let mut tx = pool.begin().await.unwrap();
        let id = insert_prompt(&mut tx, group_id, code, None, "text", "library")
            .await
            .unwrap();
        tx.commit().await.unwrap();
        id
    }

    // 合并：from 组提示词全部迁入 into，编号（code）保持不变；from 组删空后消失。
    #[tokio::test]
    async fn merge_moves_prompts_and_keeps_codes() {
        let (pool, _d) = test_pool().await;
        let a = seed_group(&pool, "甲", "AA").await;
        let b = seed_group(&pool, "乙", "BB").await;
        let p1 = seed_prompt(&pool, a, "AA-0001").await;
        seed_prompt(&pool, b, "BB-0001").await;

        let mut tx = pool.begin().await.unwrap();
        merge_into(&mut tx, a, b).await.unwrap();
        delete_group(&mut tx, a).await.unwrap();
        tx.commit().await.unwrap();

        assert_eq!(
            count_in_group(&pool, b).await.unwrap(),
            2,
            "两条都在 into 组"
        );
        let moved = get(&pool, p1).await.unwrap().unwrap();
        assert_eq!(moved.group_id, b, "提示词已迁到 into 组");
        assert_eq!(moved.code, "AA-0001", "编号前缀保留不重编");
        assert!(
            get_group(&pool, a).await.unwrap().is_none(),
            "from 组已删除"
        );
    }

    // 批量移动：仅 active 提示词被移动，编号不变。
    #[tokio::test]
    async fn move_prompts_updates_group_only() {
        let (pool, _d) = test_pool().await;
        let a = seed_group(&pool, "甲", "AA").await;
        let b = seed_group(&pool, "乙", "BB").await;
        let p1 = seed_prompt(&pool, a, "AA-0001").await;
        let p2 = seed_prompt(&pool, a, "AA-0002").await;

        let mut tx = pool.begin().await.unwrap();
        let n = move_prompts(&mut tx, &[p1, p2], b).await.unwrap();
        tx.commit().await.unwrap();

        assert_eq!(n, 2);
        assert_eq!(count_in_group(&pool, a).await.unwrap(), 0);
        assert_eq!(count_in_group(&pool, b).await.unwrap(), 2);
        assert_eq!(get(&pool, p1).await.unwrap().unwrap().code, "AA-0001");
    }

    // 批量收藏：置位后再取消。
    #[tokio::test]
    async fn set_favorite_many_toggles_batch() {
        let (pool, _d) = test_pool().await;
        let g = seed_group(&pool, "甲", "AA").await;
        let p1 = seed_prompt(&pool, g, "AA-0001").await;
        let p2 = seed_prompt(&pool, g, "AA-0002").await;

        let mut tx = pool.begin().await.unwrap();
        set_favorite_many(&mut tx, &[p1, p2], true).await.unwrap();
        tx.commit().await.unwrap();
        assert_eq!(get(&pool, p1).await.unwrap().unwrap().favorite, 1);
        assert_eq!(get(&pool, p2).await.unwrap().unwrap().favorite, 1);

        let mut tx = pool.begin().await.unwrap();
        set_favorite_many(&mut tx, &[p1], false).await.unwrap();
        tx.commit().await.unwrap();
        assert_eq!(get(&pool, p1).await.unwrap().unwrap().favorite, 0);
    }

    // 删除分组：级联删提示词行；但 accepted_works 快照（无外键）保留。
    #[tokio::test]
    async fn delete_group_cascades_prompts_but_keeps_work_snapshot() {
        let (pool, _d) = test_pool().await;
        let g = seed_group(&pool, "甲", "AA").await;
        let p = seed_prompt(&pool, g, "AA-0001").await;
        // 作品快照：prompt_id/group_id 仅为快照列，无外键。
        sqlx::query(
            "INSERT INTO accepted_works (task_id, image_path, thumb_path, prompt_id, prompt_text,
                group_id, accepted_at) VALUES (NULL, 'a.png', 't.png', ?1, 'txt', ?2, 0)",
        )
        .bind(p)
        .bind(g)
        .execute(&pool)
        .await
        .unwrap();

        let mut tx = pool.begin().await.unwrap();
        delete_group(&mut tx, g).await.unwrap();
        tx.commit().await.unwrap();

        assert!(get(&pool, p).await.unwrap().is_none(), "提示词随组级联删除");
        let works: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM accepted_works")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(works, 1, "作品快照保留");
    }
}
