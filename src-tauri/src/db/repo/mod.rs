//! 各业务域数据仓（repo）。薄封装 SQL；业务规则在上层命令 / 引擎。

pub mod api_keys;
pub mod prompts;
pub mod refs;
pub mod settings;
pub mod tasks;
pub mod trash;
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
            prompts::insert_prompt(&mut tx, gid, &code, None, "正文", "library")
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
    async fn works_and_trash_and_prompt_trash() {
        let (pool, _d) = test_pool().await;
        // 基础依赖：参考图（池写，先于事务）+ 分组 + 提示词 + 批次 + 任务
        let rid = refs::insert(
            &pool,
            &refs::NewRefImage {
                name: "a".into(),
                group_id: None,
                file_path: "/a".into(),
                thumb_path: "/t".into(),
                width: 1,
                height: 1,
                file_size: 1,
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
        let pid = prompts::insert_prompt(&mut tx, gid, &code, Some("小标题"), "正文", "library")
            .await
            .unwrap();
        let bid = tasks::create_batch(&mut tx, "/out", "{}").await.unwrap();
        let tid = tasks::insert_task(&mut tx, bid, rid, pid, "正文", 1)
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
