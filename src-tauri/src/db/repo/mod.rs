//! 各业务域数据仓（repo）。薄封装 SQL；业务规则在上层命令 / 引擎。

pub mod accounts;
pub mod api_keys;
pub mod assets;
pub mod inbox;
pub mod intake;
pub mod ledger;
pub mod planning;
pub mod prompts;
pub mod refs;
pub mod settings;
pub mod skus;
pub mod tasks;
pub mod texts;
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

        // 无 attempts 时成功率样本为 0
        let (rate, n) = api_keys::success_rate(&pool, id, 50).await.unwrap();
        assert_eq!((rate, n), (0.0, 0));

        let account = api_keys::delete(&pool, id).await.unwrap();
        assert_eq!(account.as_deref(), Some("acct-1"));
        assert!(api_keys::list(&pool).await.unwrap().is_empty());
    }

    // 0017：并发上限放宽到 100。重建 api_keys 时**不得**碰到子表的外键指向 ——
    // 若在 FK 开启下走「建新表 → DROP api_keys → 改名」，DROP 触发隐式 DELETE，
    // tasks / task_attempts.api_key_id 会被 ON DELETE SET NULL 整列清空（成功率统计与
    // 验收页「按 Key」分组一起报废）。此测试守住迁移方式，不只是守住上限数字。
    #[tokio::test]
    async fn migration_0017_widens_concurrency_and_keeps_key_fk() {
        let (pool, _d) = test_pool().await;
        let mk = |conc: i64| api_keys::NewApiKey {
            name: "k".into(),
            keyring_account: format!("acct-{conc}"),
            base_url: "http://x/v1".into(),
            model: "m".into(),
            concurrency_limit: conc,
            rpm_limit: None,
        };
        assert!(
            api_keys::insert(&pool, &mk(100)).await.is_ok(),
            "100 应可存"
        );
        assert!(
            api_keys::insert(&pool, &mk(101)).await.is_err(),
            "101 应被 CHECK 拒绝"
        );

        // 子表 FK 仍指向 api_keys（而非迁移中间体 api_keys_old）。
        for table in ["tasks", "task_attempts"] {
            let sql: String = sqlx::query_scalar(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name = ?1",
            )
            .bind(table)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert!(
                sql.contains("REFERENCES api_keys "),
                "{table} 的外键应仍指向 api_keys，实际：{sql}"
            );
        }
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
            prompts::insert_prompt(&mut tx, gid, &code, None, "正文", "library", None)
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
                ref_group_id: None,
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
        // 0019：归属的是**图库分组**（ref_groups），不再是提示词组。
        // 此处原本建的是 prompt_group —— 迁移后那样写会直接撞外键（正是它抓到了语义切换）。
        let g = refs::create_group(&pool, "产品图").await.unwrap();
        refs::set_group(&pool, id, Some(g.id)).await.unwrap();
        let rows = refs::list_active(&pool).await.unwrap();
        assert_eq!(rows[0].ref_group_id, Some(g.id));
        // 历史列 group_id 不再被任何写路径碰到。
        assert_eq!(rows[0].group_id, None);
    }

    // 0019：图库分组 CRUD。删组不删图——图回到未分组。
    #[tokio::test]
    async fn ref_groups_crud_and_delete_keeps_images() {
        let (pool, _d) = test_pool().await;
        let g = refs::create_group(&pool, "场景图").await.unwrap();
        // NOCASE 唯一：同名（含大小写变体）不得再建一个。
        assert!(refs::find_group_by_name(&pool, "场景图")
            .await
            .unwrap()
            .is_some());
        assert!(refs::create_group(&pool, "场景图").await.is_err());

        let id = refs::insert(
            &pool,
            &refs::NewRefImage {
                name: "a".into(),
                ref_group_id: Some(g.id),
                ephemeral: false,
                file_path: "/x/a.jpg".into(),
                thumb_path: "/x/a_t.jpg".into(),
                width: 1,
                height: 1,
                file_size: 1,
                content_hash: None,
                upload_path: None,
            },
        )
        .await
        .unwrap();

        let listed = refs::list_groups(&pool).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].1, 1, "组内计数");

        assert!(refs::rename_group(&pool, g.id, "场景素材").await.unwrap());
        assert_eq!(
            refs::list_groups(&pool).await.unwrap()[0].0.name,
            "场景素材"
        );

        assert!(refs::delete_group(&pool, g.id).await.unwrap());
        assert!(refs::list_groups(&pool).await.unwrap().is_empty());
        let rows = refs::list_active(&pool).await.unwrap();
        assert_eq!(rows.len(), 1, "删分组不该删图");
        assert_eq!(rows[0].id, id);
        assert_eq!(rows[0].ref_group_id, None, "图回到未分组");
    }

    // 0019 的数据搬运：既有「按提示词组名」的归属须原样落到同名图库分组，不能塌成未分组。
    //
    // migrate!() 一次跑完全部迁移，测试无法在 0018 与 0019 之间插数据，故这里对齐一张
    // 「迁移前形态」的行（group_id 有值 / ref_group_id 为空）后，逐字重放 0019 的两条
    // 搬运语句。迁移文件一旦发布即不可改（forward-only），复制不会漂。
    #[tokio::test]
    async fn migration_0019_moves_prompt_group_association_to_ref_groups() {
        let (pool, _d) = test_pool().await;
        let mut tx = pool.begin().await.unwrap();
        let pg = prompts::create_group(&mut tx, "场景图", "CJ", "", false)
            .await
            .unwrap();
        tx.commit().await.unwrap();

        // 迁移前形态：归属写在指向 prompt_groups 的历史列上。
        let rid = refs::insert(
            &pool,
            &refs::NewRefImage {
                name: "a".into(),
                ref_group_id: None,
                ephemeral: false,
                file_path: "/x/a.jpg".into(),
                thumb_path: "/x/a_t.jpg".into(),
                width: 1,
                height: 1,
                file_size: 1,
                content_hash: None,
                upload_path: None,
            },
        )
        .await
        .unwrap();
        sqlx::query("UPDATE ref_images SET group_id = ?2 WHERE id = ?1")
            .bind(rid)
            .bind(pg)
            .execute(&pool)
            .await
            .unwrap();

        // —— 以下两条逐字来自 0019_ref_groups.sql ——
        sqlx::query(
            "INSERT OR IGNORE INTO ref_groups (name, sort_order, created_at)
             SELECT DISTINCT pg.name, 0, strftime('%s', 'now')
             FROM ref_images ri
             JOIN prompt_groups pg ON pg.id = ri.group_id
             WHERE ri.deleted_at IS NULL",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE ref_images
             SET ref_group_id = (
               SELECT rg.id FROM ref_groups rg
               WHERE rg.name = (SELECT pg.name FROM prompt_groups pg WHERE pg.id = ref_images.group_id)
                 COLLATE NOCASE
               LIMIT 1
             )
             WHERE group_id IS NOT NULL AND deleted_at IS NULL",
        )
        .execute(&pool)
        .await
        .unwrap();

        let groups = refs::list_groups(&pool).await.unwrap();
        assert_eq!(groups.len(), 1, "应按提示词组名建出同名图库分组");
        assert_eq!(groups[0].0.name, "场景图");
        assert_eq!(groups[0].1, 1, "图应挂在该组下，而不是掉进未分组");
        let rows = refs::list_active(&pool).await.unwrap();
        assert_eq!(rows[0].ref_group_id, Some(groups[0].0.id));
    }

    // 0019：临时上传进 ref_images（tasks 要以它为父），但不作去重基准、不计入组内张数。
    #[tokio::test]
    async fn ephemeral_uploads_stay_out_of_library() {
        let (pool, _d) = test_pool().await;
        let g = refs::create_group(&pool, "长期").await.unwrap();
        let mk = |name: &str, hash: &str, eph: bool, gid: Option<i64>| refs::NewRefImage {
            name: name.into(),
            ref_group_id: gid,
            ephemeral: eph,
            file_path: format!("/x/{name}.jpg"),
            thumb_path: format!("/x/{name}_t.jpg"),
            width: 1,
            height: 1,
            file_size: 1,
            content_hash: Some(hash.into()),
            upload_path: None,
        };
        refs::insert(&pool, &mk("lib", "H1", false, Some(g.id)))
            .await
            .unwrap();
        refs::insert(&pool, &mk("tmp", "H2", true, Some(g.id)))
            .await
            .unwrap();

        // 去重基准只认图库的图：否则用户正式导入一张自己刚在生成页试过的图会被判「重复」。
        let hn = refs::active_hash_names(&pool).await.unwrap();
        assert_eq!(hn.len(), 1);
        assert_eq!(hn[0].1, "lib");

        // 组内计数同样不含临时上传。
        assert_eq!(refs::list_groups(&pool).await.unwrap()[0].1, 1);

        // 但 list_active 仍返回它——生成页要靠它渲染刚上传的那几张。
        let rows = refs::list_active(&pool).await.unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().any(|r| r.ephemeral && r.name == "tmp"));
    }

    // E30b：内容 hash 记录可查（去重比对源）+ 批量改分组。
    #[tokio::test]
    async fn refs_hash_names_and_batch_set_group() {
        let (pool, _d) = test_pool().await;
        let mk = |name: &str, hash: &str| refs::NewRefImage {
            name: name.into(),
            ref_group_id: None,
            ephemeral: false,
            file_path: format!("/x/{name}.jpg"),
            thumb_path: format!("/x/{name}_t.jpg"),
            width: 10,
            height: 10,
            file_size: 1,
            content_hash: Some(hash.into()),
            upload_path: None,
        };
        let a = refs::insert(&pool, &mk("a", "H1")).await.unwrap();
        let b = refs::insert(&pool, &mk("b", "H2")).await.unwrap();

        let hn = refs::active_hash_names(&pool).await.unwrap();
        assert_eq!(hn.len(), 2);
        assert!(hn.iter().any(|(h, n)| h == "H1" && n == "a"));

        // 0019：批量改的是图库分组。
        let g = refs::create_group(&pool, "组").await.unwrap();
        let n = refs::set_group_many(&pool, &[a, b], Some(g.id))
            .await
            .unwrap();
        assert_eq!(n, 2);
        let rows = refs::list_active(&pool).await.unwrap();
        assert!(rows.iter().all(|r| r.ref_group_id == Some(g.id)));
    }

    #[tokio::test]
    async fn works_and_trash_and_prompt_trash() {
        let (pool, _d) = test_pool().await;
        // 基础依赖：参考图（池写，先于事务）+ 分组 + 提示词 + 批次 + 任务
        let rid = refs::insert(
            &pool,
            &refs::NewRefImage {
                name: "a".into(),
                ref_group_id: None,
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
        let pid =
            prompts::insert_prompt(&mut tx, gid, &code, Some("小标题"), "正文", "library", None)
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
                prompt_code: String::new(),
                group_name: String::new(),
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
