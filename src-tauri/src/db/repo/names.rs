//! 发号台账 `issued_names`（0044）：交付文件名的唯一发放处。
//!
//! 交付文件名 = `{语义名}-{uid}.{EXT}`。语义名由 skill 在工单里给定（GenDesk 一个
//! 字符都不改）；`uid` 由这里发，而且**只在验收通过那一刻发**——生成中间产物不发号。
//!
//! ## 为什么不能「先 SELECT 查重再 INSERT」
//!
//! 查与插之间就是重名窗口。它与 `claim_ready` / `mark_running` 那三处并发窗口是同一
//! 种形状，代价也一样是钱：撞号意味着两张不同的图在 `自动发布` 那边共用一个主键，
//! 而那要到人发现「这条视频配错图了」才看得出来。
//!
//! 故这里**直接 INSERT，撞 UNIQUE 就重摇**——唯一性由数据库的索引保证，不由我们
//! 读到的那一瞬间的快照保证。

use sqlx::SqliteConnection;

use crate::db::now_unix;
use crate::error::{AppError, AppResult};

/// UID 字符集：小写字母 + 数字。
///
/// 不含大写：文件名在 macOS（大小写不敏感）与 Linux（敏感）之间往返，`Hu72` 与
/// `hu72` 在一边是两个号、在另一边是同一个文件。
pub const UID_ALPHABET: &[u8; 36] = b"abcdefghijklmnopqrstuvwxyz0123456789";

/// UID 长度。36⁴ = 1,679,616。
///
/// 4 位够用的判据不是「空间有多大」，而是**已发放数 ÷ 空间**：10 万条时首摇撞上的
/// 概率约 6%，而撞了只是再摇一次（下面那个循环），不是失败。真到了需要加长的那天，
/// 加长是 forward-only 的——老号仍然合法，新号变 5 位，两者共存不冲突。
pub const UID_LEN: usize = 4;

/// 重摇上限。摇满这么多次还撞，说明的不是运气不好而是空间快满了，该加长 UID 了。
const MAX_REROLLS: usize = 8;

/// 随机 UID（`[a-z0-9]{4}`）。
///
/// 用 `OsRng`（chacha20poly1305 默认特性带的 `getrandom`，与 `secrets.rs` 同一个来源），
/// 不用时间戳或自增：UID 的意义就在于**没有可推断的顺序**，否则 `自动发布` 那边
/// 迟早有人拿它当序号排序。
pub fn random_uid() -> String {
    use chacha20poly1305::aead::rand_core::RngCore;
    let mut buf = [0u8; UID_LEN];
    chacha20poly1305::aead::OsRng.fill_bytes(&mut buf);
    buf.iter()
        .map(|b| UID_ALPHABET[*b as usize % UID_ALPHABET.len()] as char)
        .collect()
}

/// 完整文件名主干：`{语义名}-{uid}`。
pub fn compose(item_name: &str, uid: &str) -> String {
    format!("{item_name}-{uid}")
}

/// 发一个全局唯一 UID 并记进台账。返回 `(uid, name)`。
///
/// `work_id` 允许为 `None`：UID 必须在**拷贝之前**定下来（文件名里有它），而作品行
/// 要等整批拷完才写。中途失败会留下一个没有主人的号——那是「不回收」的正常代价。
/// 作品行落库之后由 [`attach_work`] 认领。
pub async fn issue(
    conn: &mut SqliteConnection,
    item_name: &str,
    work_id: Option<i64>,
) -> AppResult<(String, String)> {
    issue_with(conn, item_name, work_id, random_uid).await
}

/// [`issue`] 的可注入版本：测试用固定生成器逼出重摇路径。
pub async fn issue_with<F>(
    conn: &mut SqliteConnection,
    item_name: &str,
    work_id: Option<i64>,
    mut gen: F,
) -> AppResult<(String, String)>
where
    F: FnMut() -> String,
{
    let item_name = item_name.trim();
    if item_name.is_empty() {
        // 名字是身份。空名字发出去的号，人在 Finder 里认不出是哪张图。
        return Err(AppError::InvalidInput("发号需要一个非空的语义名".into()));
    }
    let at = now_unix();
    let mut last: Option<sqlx::Error> = None;
    for _ in 0..MAX_REROLLS {
        let uid = gen();
        let name = compose(item_name, &uid);
        // 直接插。撞 UNIQUE 是**正常路径**，不是异常：SQLite 的约束冲突默认 ABORT，
        // 只回退这一条语句，外层事务照旧活着，所以可以就地再摇一次。
        match sqlx::query(
            "INSERT INTO issued_names (name, uid, work_id, issued_at) VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(&name)
        .bind(&uid)
        .bind(work_id)
        .bind(at)
        .execute(&mut *conn)
        .await
        {
            Ok(_) => return Ok((uid, name)),
            Err(e) if is_unique_violation(&e) => {
                last = Some(e);
                continue;
            }
            Err(e) => return Err(e.into()),
        }
    }
    let issued: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM issued_names")
        .fetch_one(&mut *conn)
        .await
        .unwrap_or(-1);
    tracing::error!(error = ?last, issued, "UID 重摇耗尽");
    Err(AppError::Internal(format!(
        "连摇 {MAX_REROLLS} 次都撞上已发放的 UID（台账已有 {issued} 条）。\
         这不是运气问题，是 {UID_LEN} 位空间快满了 —— 需要加长 UID 才能继续发号。"
    )))
}

/// 作品行落库后认领这个号。失败不致命：台账的意义是「发出去过没有」，主人是附加信息。
pub async fn attach_work(
    conn: &mut SqliteConnection,
    uid: &str,
    work_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE issued_names SET work_id = ?2 WHERE uid = ?1")
        .bind(uid)
        .bind(work_id)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

fn is_unique_violation(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(db) if db.is_unique_violation())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即失败
mod tests {
    use super::*;
    use crate::db::test_support::test_pool;

    #[test]
    fn uid_shape_is_lowercase_alnum_of_fixed_length() {
        for _ in 0..200 {
            let u = random_uid();
            assert_eq!(u.chars().count(), UID_LEN, "{u}");
            assert!(
                u.bytes().all(|b| UID_ALPHABET.contains(&b)),
                "字符集越界：{u}"
            );
        }
    }

    // R1 主路径：同一个语义名连续验收两次 → 两个不同 uid、台账两行。
    // 名字带 uid 才是主干，所以「同名图重跑一次」不会互相覆盖文件。
    #[tokio::test]
    async fn same_item_name_twice_gets_two_distinct_uids() {
        let (pool, _d) = test_pool().await;
        let mut tx = pool.begin().await.unwrap();
        let (u1, n1) = issue(&mut tx, "W-王一博-55-正面", None).await.unwrap();
        let (u2, n2) = issue(&mut tx, "W-王一博-55-正面", None).await.unwrap();
        tx.commit().await.unwrap();
        assert_ne!(u1, u2, "同名两次必须拿到不同的 UID");
        assert_ne!(n1, n2);
        assert!(n1.starts_with("W-王一博-55-正面-"));
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM issued_names")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 2);
    }

    // 生成器固定返回同一个值 → 第二次插入必须走**重摇**路径（而不是覆盖或静默成功），
    // 摇满上限后给出一句人能照着做事的错误。
    #[tokio::test]
    async fn a_colliding_generator_rerolls_then_fails_readably() {
        let (pool, _d) = test_pool().await;
        let mut tx = pool.begin().await.unwrap();
        let (u1, _) = issue_with(&mut tx, "甲", None, || "aaaa".into())
            .await
            .unwrap();
        assert_eq!(u1, "aaaa");

        let err = issue_with(&mut tx, "乙", None, || "aaaa".into())
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("重摇") || msg.contains("撞"), "{msg}");
        assert!(msg.contains("加长"), "要说清该做什么：{msg}");

        // 撞号那几次一行都不该留下。语义名不同（甲/乙）但 uid 相同 —— 唯一索引
        // 挡的是 uid 而不只是 name，这正是「`自动发布` 可以只存 uid 当主键」的前提。
        tx.commit().await.unwrap();
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM issued_names")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 1, "重摇失败不得留下半行");
    }

    // 一次撞号之后换一个号就该成功 —— 重摇不是「失败」，是正常路径。
    #[tokio::test]
    async fn one_collision_is_absorbed_by_the_next_roll() {
        let (pool, _d) = test_pool().await;
        let mut tx = pool.begin().await.unwrap();
        issue_with(&mut tx, "甲", None, || "aaaa".into())
            .await
            .unwrap();
        let mut seq = ["aaaa", "bbbb"].into_iter();
        let (uid, _) = issue_with(&mut tx, "乙", None, || {
            seq.next().unwrap_or("cccc").to_string()
        })
        .await
        .unwrap();
        assert_eq!(uid, "bbbb", "第一摇撞了就该用第二摇的号");
        tx.commit().await.unwrap();
    }

    // R3：号不回收。作品是本仓库唯一「删除即真删行」的实体，而那个文件很可能早已
    // 交付到 `自动发布` 那边 —— 台账必须仍然答得出「这个 UID 发出去过」。
    // 孤儿行（work_id 指向一个不存在的作品）是**正常状态**，不是要修的脏数据。
    #[tokio::test]
    async fn deleting_the_work_does_not_take_the_uid_with_it() {
        let (pool, _d) = test_pool().await;
        sqlx::query(
            "INSERT INTO accepted_works (id,image_path,thumb_path,prompt_text,accepted_at)
             VALUES (7,'/o.jpg','/t.jpg','x',0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let mut tx = pool.begin().await.unwrap();
        let (uid, _) = issue(&mut tx, "W-王一博-55-正面", None).await.unwrap();
        attach_work(&mut tx, &uid, 7).await.unwrap();
        tx.commit().await.unwrap();

        // 作品进废纸篓 → 清空废纸篓 = 这一行被物理删掉。
        sqlx::query("DELETE FROM accepted_works WHERE id = 7")
            .execute(&pool)
            .await
            .unwrap();

        let (n, owner): (i64, Option<i64>) = sqlx::query_as(
            "SELECT (SELECT COUNT(*) FROM issued_names), (SELECT work_id FROM issued_names)",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(n, 1, "作品删了，号不许跟着消失");
        assert_eq!(owner, Some(7), "孤儿指针照留：它记的是历史，不是当前归属");
    }

    #[tokio::test]
    async fn empty_item_name_is_rejected() {
        let (pool, _d) = test_pool().await;
        let mut tx = pool.begin().await.unwrap();
        assert!(issue(&mut tx, "   ", None).await.is_err());
    }
}
