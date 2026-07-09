//! 各业务域数据仓（repo）。薄封装 SQL；业务规则在上层命令 / 引擎。

pub mod api_keys;
pub mod prompts;
pub mod refs;
pub mod settings;
pub mod tasks;

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
            },
        )
        .await
        .unwrap();
        assert_eq!(api_keys::list(&pool).await.unwrap().len(), 1);

        api_keys::set_enabled(&pool, id, false).await.unwrap();
        assert_eq!(api_keys::get(&pool, id).await.unwrap().unwrap().enabled, 0);

        api_keys::update_fields(&pool, id, Some("改名"), None, None, Some(7))
            .await
            .unwrap();
        let row = api_keys::get(&pool, id).await.unwrap().unwrap();
        assert_eq!(row.name, "改名");
        assert_eq!(row.concurrency_limit, 7);

        // 无 attempts 时成功率样本为 0
        let (rate, n) = api_keys::success_rate(&pool, id, 50).await.unwrap();
        assert_eq!((rate, n), (0.0, 0));

        let account = api_keys::delete(&pool, id).await.unwrap();
        assert_eq!(account.as_deref(), Some("acct-1"));
        assert!(api_keys::list(&pool).await.unwrap().is_empty());
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
            prompts::insert_prompt(&mut tx, gid, &code, "正文", "library")
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
    async fn refs_insert_list_setgroup() {
        let (pool, _d) = test_pool().await;
        let id = refs::insert(
            &pool,
            &refs::NewRefImage {
                name: "productA".into(),
                group_id: None,
                file_path: "/x/a.jpg".into(),
                thumb_path: "/x/a_t.jpg".into(),
                width: 1024,
                height: 768,
                file_size: 12345,
            },
        )
        .await
        .unwrap();
        assert_eq!(refs::list_active(&pool).await.unwrap().len(), 1);
        // set_group 需要一个真实分组以满足外键
        let mut tx = pool.begin().await.unwrap();
        let gid = prompts::create_group(&mut tx, "组", "GG", "", false)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        refs::set_group(&pool, id, Some(gid)).await.unwrap();
        let rows = refs::list_active(&pool).await.unwrap();
        assert_eq!(rows[0].group_id, Some(gid));
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
