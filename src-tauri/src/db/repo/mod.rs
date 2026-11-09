//! 各业务域数据仓（repo）。薄封装 SQL；业务规则在上层命令 / 引擎。

pub mod api_keys;
pub mod intake;
pub mod names;
pub mod prompts;
pub mod ref_library;
pub mod refs;
pub mod settings;
pub mod tasks;
pub mod trash;
pub mod v2v;
pub mod works;

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即失败
mod tests {
    use super::*;
    use crate::db::test_support::test_pool;
    use crate::ids;

    #[tokio::test]
    async fn api_keys_crud_and_rate() {
        let (pool, _d) = test_pool().await;
        let id = api_keys::insert(
            &pool,
            &api_keys::NewApiKey {
                name: "主力".into(),
                keyring_account: "acct-1".into(),
                base_url: "https://api.example.com/v1".into(),
                model: "gpt-image-2".into(),
                concurrency_limit: 3,
                rpm_limit: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(api_keys::list(&pool).await.unwrap().len(), 1);

        api_keys::set_enabled(&pool, id, false).await.unwrap();
        assert_eq!(api_keys::get(&pool, id).await.unwrap().unwrap().enabled, 0);

        // 改名 + 并发 + 设置 rpm_limit=30。
        api_keys::update_fields(&pool, id, Some("改名"), None, None, Some(7), Some(Some(30)))
            .await
            .unwrap();
        let row = api_keys::get(&pool, id).await.unwrap().unwrap();
        assert_eq!(row.name, "改名");
        assert_eq!(row.concurrency_limit, 7);
        assert_eq!(row.rpm_limit, Some(30));

        // 熔断 → 恢复。
        api_keys::trip_circuit(&pool, id).await.unwrap();
        let row = api_keys::get(&pool, id).await.unwrap().unwrap();
        assert_eq!((row.enabled, row.circuit_broken), (0, 1));
        api_keys::recover_circuit(&pool, id).await.unwrap();
        let row = api_keys::get(&pool, id).await.unwrap().unwrap();
        assert_eq!((row.enabled, row.circuit_broken), (1, 0));

        // 熔断后直接打开开关也是一次显式恢复，状态位必须一起清掉。
        api_keys::trip_circuit(&pool, id).await.unwrap();
        api_keys::set_enabled(&pool, id, true).await.unwrap();
        let row = api_keys::get(&pool, id).await.unwrap().unwrap();
        assert_eq!((row.enabled, row.circuit_broken), (1, 0));

        // 无 attempts 时成功率样本为 0
        let (rate, n) = api_keys::success_rate(&pool, id, 50).await.unwrap();
        assert_eq!((rate, n), (0.0, 0));

        let account = api_keys::delete(&pool, id).await.unwrap();
        assert_eq!(account.as_deref(), Some("acct-1"));
        assert!(api_keys::list(&pool).await.unwrap().is_empty());
    }

    // 0043：并发上限放宽到 250。迁移 api_keys 时**不得删除父表** —— DROP 会让
    // tasks / task_attempts.api_key_id 经 ON DELETE SET NULL 整列清空（成功率统计与
    // 验收页「按 Key」分组一起报废）。结构与 foreign_key_check 仍然都会显示正常，
    // 所以测试必须在迁移前造真实关联、迁移后直接核对值，而不只检查 schema。
    #[tokio::test]
    async fn migration_0043_widens_concurrency_and_keeps_key_fk() {
        use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

        let dir = tempfile::tempdir().unwrap();
        let opts = SqliteConnectOptions::new()
            .filename(dir.path().join("pre-0043.db"))
            .create_if_missing(true)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .unwrap();
        sqlx::raw_sql(
            "CREATE TABLE api_keys (
               id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL,
               keyring_account TEXT NOT NULL UNIQUE, base_url TEXT NOT NULL, model TEXT NOT NULL,
               concurrency_limit INTEGER NOT NULL DEFAULT 2
                 CHECK (concurrency_limit BETWEEN 1 AND 100),
               enabled INTEGER NOT NULL DEFAULT 1, created_at INTEGER NOT NULL,
               rpm_limit INTEGER, circuit_broken INTEGER NOT NULL DEFAULT 0
             );
             CREATE TABLE tasks (
               id INTEGER PRIMARY KEY, api_key_id INTEGER REFERENCES api_keys(id) ON DELETE SET NULL
             );
             CREATE TABLE task_attempts (
               id INTEGER PRIMARY KEY, api_key_id INTEGER REFERENCES api_keys(id) ON DELETE SET NULL
             );",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO api_keys
             (id,name,keyring_account,base_url,model,concurrency_limit,enabled,created_at)
             VALUES (1,'old','acct-old','http://x/v1','m',100,1,0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO tasks (id,api_key_id) VALUES (1,1)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO task_attempts (id,api_key_id) VALUES (1,1)")
            .execute(&pool)
            .await
            .unwrap();

        sqlx::raw_sql(include_str!(
            "../../../migrations/0043_key_concurrency_250.sql"
        ))
        .execute(&pool)
        .await
        .unwrap();

        let (current, legacy): (i64, i64) = sqlx::query_as(
            "SELECT concurrency_limit,concurrency_limit_legacy FROM api_keys WHERE id=1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!((current, legacy), (250, 100), "旧上限值应明确提到 250");
        for table in ["tasks", "task_attempts"] {
            let linked: Option<i64> =
                sqlx::query_scalar(&format!("SELECT api_key_id FROM {table} WHERE id=1"))
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(linked, Some(1), "{table}.api_key_id 不得被迁移清空");
        }

        let mk = |conc: i64| api_keys::NewApiKey {
            name: "k".into(),
            keyring_account: format!("acct-{conc}"),
            base_url: "http://x/v1".into(),
            model: "m".into(),
            concurrency_limit: conc,
            rpm_limit: None,
        };
        assert!(
            api_keys::insert(&pool, &mk(250)).await.is_ok(),
            "250 应可存"
        );
        assert!(
            api_keys::insert(&pool, &mk(251)).await.is_err(),
            "251 应被 CHECK 拒绝"
        );
    }

    #[tokio::test]
    async fn prompts_group_and_insert_with_ids() {
        let (pool, _d) = test_pool().await;
        let mut tx = pool.begin().await.unwrap();
        let gid = prompts::create_group(&mut tx, "电商", "DZ", "商品", false)
            .await
            .unwrap();
        for _ in 0..3 {
            let n = ids::allocate(&mut tx, "DZ").await.unwrap();
            let code = ids::format_code("DZ", n);
            prompts::insert_prompt(
                &mut tx,
                &prompts::NewPrompt {
                    group_id: gid,
                    code: &code,
                    text: "正文",
                    source: "library",
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        }
        tx.commit().await.unwrap();

        assert_eq!(prompts::count_in_group(&pool, gid).await.unwrap(), 3);
        assert_eq!(prompts::list_groups(&pool).await.unwrap().len(), 1);
        let found = prompts::find_group_by_prefix(&pool, "DZ").await.unwrap();
        assert_eq!(found.unwrap().id, gid);
    }

    #[tokio::test]
    async fn refs_insert_list() {
        let (pool, _d) = test_pool().await;
        refs::insert(
            &pool,
            &refs::NewRefImage {
                name: "productA".into(),
                ephemeral: false,
                file_path: "/x/a.jpg".into(),
                thumb_path: "/x/a_t.jpg".into(),
                width: 1024,
                height: 768,
                file_size: 12345,
                content_hash: None,
                upload_path: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(refs::list_active(&pool).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn works_and_trash_and_prompt_trash() {
        let (pool, _d) = test_pool().await;
        // 基础依赖：参考图（池写，先于事务）+ 分组 + 提示词 + 批次 + 任务
        let rid = refs::insert(
            &pool,
            &refs::NewRefImage {
                name: "a".into(),
                ephemeral: false,
                file_path: "/a".into(),
                thumb_path: "/t".into(),
                width: 1,
                height: 1,
                file_size: 1,
                content_hash: None,
                upload_path: None,
            },
        )
        .await
        .unwrap();
        let mut tx = pool.begin().await.unwrap();
        let gid = prompts::create_group(&mut tx, "电商", "DZ", "商品", false)
            .await
            .unwrap();
        let n = ids::allocate(&mut tx, "DZ").await.unwrap();
        let code = ids::format_code("DZ", n);
        let pid = prompts::insert_prompt(
            &mut tx,
            &prompts::NewPrompt {
                group_id: gid,
                code: &code,
                title: Some("小标题"),
                text: "正文",
                source: "library",
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let bid = tasks::create_batch(&mut tx, "/out", "{}").await.unwrap();
        let tid = tasks::insert_task(&mut tx, bid, rid, pid, "正文", "")
            .await
            .unwrap();
        tx.commit().await.unwrap();

        // 作品插入 + 收藏 + 计数
        let mut tx = pool.begin().await.unwrap();
        let wid = works::insert(
            &mut tx,
            &works::NewWork {
                task_id: tid,
                image_path: "/img.jpg".into(),
                thumb_path: "/img_t.jpg".into(),
                prompt_id: pid,
                prompt_text: "正文".into(),
                group_id: Some(gid),
                ref_image_id: rid,
                batch_id: bid,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        assert_eq!(works::count(&pool).await.unwrap(), 1);
        works::toggle_favorite(&pool, wid).await.unwrap();
        assert_eq!(works::get(&pool, wid).await.unwrap().unwrap().favorite, 1);

        // 提示词进废纸篓（entity_type='prompt' + code），take/purge 编号回收
        let (tcode, ttitle, _g) = prompts::set_trash(&pool, pid).await.unwrap().unwrap();
        assert_eq!(ttitle.as_deref(), Some("小标题")); // title 快照随删除保留
        let mut tx = pool.begin().await.unwrap();
        let trash_id = trash::insert(
            &mut tx,
            &trash::NewTrashItem {
                entity_type: "prompt".into(),
                ref_id: Some(pid),
                thumb_path: None,
                prompt_text: None,
                code: Some(tcode),
                title: ttitle,
                source_label: "手动删除".into(),
                file_paths: Vec::new(),
                payload_json: None,
            },
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        assert_eq!(trash::count(&pool).await.unwrap(), 1);

        let taken = trash::take(&pool, &[trash_id]).await.unwrap();
        assert_eq!(taken.len(), 1);
        // 回收编号 + 删记录同事务
        let mut tx = pool.begin().await.unwrap();
        ids::recycle(&mut tx, "DZ", 1).await.unwrap();
        trash::delete_rows(&mut tx, &[trash_id]).await.unwrap();
        tx.commit().await.unwrap();
        assert_eq!(trash::count(&pool).await.unwrap(), 0);
        // 回收后再发放应复用 1
        let mut conn = pool.acquire().await.unwrap();
        assert_eq!(ids::allocate(&mut conn, "DZ").await.unwrap(), 1);
    }

    #[tokio::test]
    async fn settings_roundtrip() {
        let (pool, _d) = test_pool().await;
        assert!(settings::get_raw(&pool).await.unwrap().is_none());
        settings::set_raw(&pool, r#"{"paused":true}"#)
            .await
            .unwrap();
        assert_eq!(
            settings::get_raw(&pool).await.unwrap().as_deref(),
            Some(r#"{"paused":true}"#)
        );
        // upsert
        settings::set_raw(&pool, r#"{"paused":false}"#)
            .await
            .unwrap();
        assert_eq!(
            settings::get_raw(&pool).await.unwrap().as_deref(),
            Some(r#"{"paused":false}"#)
        );
    }
}
