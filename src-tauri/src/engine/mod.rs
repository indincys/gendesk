//! 任务引擎（执行计划 M2 / 技术文档 4.1）。
//!
//! 组合展开（挂靠模型）→ 单 dispatcher 循环 + per-Key Semaphore + 策略 + 重试/退避
//! → 状态迁移统一落库 + task_attempts 全记录 → 伪进度/事件推送 → 中断恢复。

// 引擎公有 API 由命令层（下一步）消费；先落地。
#![allow(dead_code)]

pub mod classify;
pub mod dispatcher;
pub mod events;
pub mod progress;
pub mod recovery;
pub mod status;
pub mod strategy;

use std::sync::Arc;
use std::time::Duration;

use sqlx::SqlitePool;

use crate::db::repo::{
    api_keys as key_repo, prompts as prompt_repo, refs as ref_repo, tasks as task_repo,
};
use crate::error::AppResult;
use crate::files::DataDirs;
use crate::provider::{openai::OpenAiCompatible, ImageProvider};
use crate::secrets::SecretStore;

use dispatcher::Scheduler;
use events::SharedSink;
use strategy::Strategy;

/// 单个 Key 的运行配置（含明文 Key，仅在引擎内存活，永不外泄）。
#[derive(Clone)]
pub struct KeyConfig {
    pub id: i64,
    pub base_url: String,
    pub model: String,
    pub api_key: String,
    pub concurrency_limit: u32,
    pub enabled: bool,
    /// 每分钟请求上限（E18）；None = 不限速。
    pub rpm_limit: Option<u32>,
}

/// Provider 工厂（便于测试注入 FakeProvider）。
pub trait ProviderFactory: Send + Sync + 'static {
    fn build(&self, key: &KeyConfig) -> Arc<dyn ImageProvider>;
}

/// 生产工厂：复用同一 reqwest::Client，按 Key 生成 OpenAiCompatible。
pub struct OpenAiFactory {
    client: reqwest::Client,
    request_timeout: Duration,
}

impl OpenAiFactory {
    pub fn new(connect_timeout: Duration, request_timeout: Duration) -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(connect_timeout)
            .build()
            .unwrap_or_default();
        Self {
            client,
            request_timeout,
        }
    }
}

impl ProviderFactory for OpenAiFactory {
    fn build(&self, key: &KeyConfig) -> Arc<dyn ImageProvider> {
        Arc::new(OpenAiCompatible::with_client(
            self.client.clone(),
            &key.base_url,
            &key.api_key,
            self.request_timeout,
        ))
    }
}

/// 从 DB + 钥匙串加载全部 Key 配置（无密钥的跳过）。
pub fn load_key_configs(rows: &[key_repo::ApiKeyRow], secrets: &dyn SecretStore) -> Vec<KeyConfig> {
    rows.iter()
        .filter_map(|r| {
            let api_key = secrets.get(&r.keyring_account).ok().flatten()?;
            Some(KeyConfig {
                id: r.id,
                base_url: r.base_url.clone(),
                model: r.model.clone(),
                api_key,
                concurrency_limit: r.concurrency_limit.clamp(1, key_repo::MAX_CONCURRENCY) as u32,
                enabled: r.enabled != 0,
                rpm_limit: r
                    .rpm_limit
                    .and_then(|n| u32::try_from(n).ok())
                    .filter(|n| *n > 0),
            })
        })
        .collect()
}

/// 一次挂靠：某参考图挂靠某提示词组。
#[derive(Debug, Clone, Copy)]
pub struct RefMapping {
    pub ref_image_id: i64,
    pub prompt_group_id: i64,
}

/// 组合展开并创建批次（R1 挂靠模型，一次事务）。返回 (batch_id, 任务数)。
pub async fn create_batch(
    pool: &SqlitePool,
    output_dir: &str,
    params_json: &str,
    mappings: &[RefMapping],
    draws: i64,
) -> AppResult<(i64, i64)> {
    // 抽卡次数（E17 / D2）：每个组合独立生成 k 次；夹取 1..=5 防脏输入。
    let draws = draws.clamp(1, 5);
    // 预取各组 active 提示词（读，不在事务内）。
    let mut combos: Vec<(i64, i64, String)> = Vec::new(); // (ref, prompt_id, snapshot)
    for m in mappings {
        let prompts = prompt_repo::list_active_prompts(pool, m.prompt_group_id).await?;
        for (pid, text) in prompts {
            combos.push((m.ref_image_id, pid, text));
        }
    }
    if combos.is_empty() {
        return Err(crate::error::AppError::InvalidInput(
            "所选组合展开后无任务（提示词组为空？）".into(),
        ));
    }
    let task_count = combos.len() as i64 * draws;

    let mut tx = pool.begin().await?;
    let batch_id = task_repo::create_batch(&mut tx, output_dir, params_json).await?;
    for m in mappings {
        task_repo::add_batch_ref(&mut tx, batch_id, m.ref_image_id, m.prompt_group_id).await?;
        // E32 挂靠记忆：记录该参考图本次挂靠的组，下批预填。
        sqlx::query("UPDATE ref_images SET last_group_id = ?2 WHERE id = ?1")
            .bind(m.ref_image_id)
            .bind(m.prompt_group_id)
            .execute(&mut *tx)
            .await?;
    }
    // 每个组合展开 draws 个任务，draw_index ∈ 1..=draws（供输出命名去重）。
    for (ref_id, prompt_id, snapshot) in &combos {
        for draw_index in 1..=draws {
            task_repo::insert_task(&mut tx, batch_id, *ref_id, *prompt_id, snapshot, draw_index)
                .await?;
        }
    }
    // 归档本批用到的参考图与提示词组（0016）：批次已开跑，这批素材的使命就此完成，
    // 从生成页选择器里让位给下一批。同事务提交 —— 批次建成则必已归档，不会出现
    // 「任务跑起来了但素材还留在选择器里」的中间态。库里仍在，可一键取消归档。
    let at = crate::db::now_unix();
    let mut ref_ids: Vec<i64> = mappings.iter().map(|m| m.ref_image_id).collect();
    ref_ids.sort_unstable();
    ref_ids.dedup();
    let mut group_ids: Vec<i64> = mappings.iter().map(|m| m.prompt_group_id).collect();
    group_ids.sort_unstable();
    group_ids.dedup();
    ref_repo::archive_many(&mut tx, &ref_ids, at).await?;
    prompt_repo::archive_groups(&mut tx, &group_ids, at).await?;

    tx.commit().await?;
    Ok((batch_id, task_count))
}

/// 引擎门面：持有调度器，供命令层调用。
pub struct Engine {
    scheduler: Arc<Scheduler>,
}

impl Engine {
    /// 启动引擎：加载 Key、恢复中断、拉起调度循环。
    #[allow(clippy::too_many_arguments)] // 启动装配需完整依赖；集中于此一处
    pub async fn start(
        pool: SqlitePool,
        dirs: Arc<DataDirs>,
        factory: Arc<dyn ProviderFactory>,
        sink: SharedSink,
        strategy: Strategy,
        user_retry: u32,
        paused: bool,
        secrets: Arc<dyn SecretStore>,
    ) -> AppResult<Self> {
        // 中断恢复：run/retry → fail(Interrupted)。
        let _recovered = recovery::recover(&pool, &sink).await?;

        let key_rows = key_repo::list(&pool).await?;
        let keys = load_key_configs(&key_rows, secrets.as_ref());

        let scheduler = Scheduler::new(pool, dirs, factory, sink, strategy, user_retry, paused);
        scheduler.set_keys(keys);
        let scheduler = Arc::new(scheduler);
        Scheduler::spawn_loop(scheduler.clone());
        scheduler.notify();
        Ok(Self { scheduler })
    }

    pub fn scheduler(&self) -> Arc<Scheduler> {
        self.scheduler.clone()
    }

    pub fn pause(&self) {
        self.scheduler.pause();
    }
    pub fn resume(&self) {
        self.scheduler.resume();
    }
    /// 是否有已启用且凭据完整的 Key 可供任务派发。
    pub fn has_usable_key(&self) -> bool {
        self.scheduler.has_usable_key()
    }
    /// 人工恢复任务：重置上一波失败退避，并仅在暂停来自自动熔断时继续队列。
    pub fn prepare_manual_retry(&self) {
        self.scheduler.reset_failure_backoff();
        self.scheduler.resume_if_auto_paused();
        self.scheduler.notify();
    }
    /// Key 被人工恢复后，仅消费自动暂停；手工暂停保持不变。
    pub fn resume_if_auto_paused(&self) -> bool {
        self.scheduler.resume_if_auto_paused()
    }
    pub fn is_paused(&self) -> bool {
        self.scheduler.is_paused()
    }
    pub fn set_strategy(&self, s: Strategy) {
        self.scheduler.set_strategy(s);
    }
    pub fn set_user_retry(&self, n: u32) {
        self.scheduler.set_user_retry(n);
    }
    /// 设置全局熔断阈值（E05；0 = 关闭）。
    pub fn set_global_fail_threshold(&self, n: u32) {
        self.scheduler.set_global_fail_threshold(n);
    }
    /// 新任务入队后唤醒调度。
    pub fn kick(&self) {
        self.scheduler.notify();
    }

    /// 补发某批次汇总事件（验收等命令改动任务态后调用，驱动导航徽章）。
    pub async fn emit_summary(&self, batch_id: i64) {
        self.scheduler.emit_summary(batch_id).await;
    }

    /// 重新加载 Key 运行时（增删改 Key 后调用）。
    pub async fn reload_keys(&self, pool: &SqlitePool, secrets: &dyn SecretStore) -> AppResult<()> {
        let rows = key_repo::list(pool).await?;
        self.scheduler.set_keys(load_key_configs(&rows, secrets));
        self.scheduler.notify();
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即失败
mod tests {
    use super::*;
    use crate::db::repo::api_keys::ApiKeyRow;
    use crate::db::repo::prompts as prompt_repo;
    use crate::db::repo::refs as ref_repo;
    use crate::db::test_support::test_pool;
    use crate::secrets::MemoryStore;

    fn row(id: i64, account: &str, enabled: i64, concurrency: i64) -> ApiKeyRow {
        ApiKeyRow {
            id,
            name: format!("k{id}"),
            keyring_account: account.into(),
            base_url: "http://x/v1".into(),
            model: "m".into(),
            concurrency_limit: concurrency,
            enabled,
            created_at: 0,
            rpm_limit: None,
            circuit_broken: 0,
        }
    }

    #[test]
    fn load_key_configs_maps_enabled_and_skips_secretless() {
        let store = MemoryStore::default();
        store.set("a-en", "sk-1").unwrap();
        store.set("a-dis", "sk-2").unwrap();
        // 第三个无密钥 → 应跳过
        let rows = [
            row(1, "a-en", 1, 5),
            row(2, "a-dis", 0, 3),
            row(3, "a-nosecret", 1, 2),
        ];
        let cfgs = load_key_configs(&rows, &store);
        assert_eq!(cfgs.len(), 2, "无密钥的 Key 应被跳过");
        let en = cfgs.iter().find(|c| c.id == 1).unwrap();
        assert!(en.enabled, "enabled=1 → true");
        assert_eq!(en.concurrency_limit, 5);
        let dis = cfgs.iter().find(|c| c.id == 2).unwrap();
        assert!(!dis.enabled, "enabled=0 → false");
    }

    /// v0.11.0 把上限放到 100，但引擎侧的夹取还写死 10：DB 存 50、设置页显示 50，
    /// 实跑却恒为 10 个并发。此处必须用 >10 的值取样 —— 上面那条用 5 的断言在
    /// 两种夹取下都通过，正是它让这个回归漏了过去。
    #[test]
    fn load_key_configs_preserves_limits_above_ten() {
        let store = MemoryStore::default();
        store.set("a", "sk-1").unwrap();
        store.set("b", "sk-2").unwrap();
        store.set("c", "sk-3").unwrap();
        let rows = [
            row(1, "a", 1, 50),
            row(2, "b", 1, key_repo::MAX_CONCURRENCY + 1), // 越界仍夹到上界
            row(3, "c", 1, 0),                             // 0/负数仍抬到 1
        ];
        let cfgs = load_key_configs(&rows, &store);
        let at = |id: i64| cfgs.iter().find(|c| c.id == id).unwrap().concurrency_limit;
        assert_eq!(at(1), 50, "设置页填 50 就要跑 50，不得被引擎侧夹回 10");
        assert_eq!(at(2), key_repo::MAX_CONCURRENCY as u32);
        assert_eq!(at(3), 1);
    }

    #[tokio::test]
    async fn create_batch_expands_task_count_correctly() {
        let (pool, _d) = test_pool().await;
        // 1 参考图 + 1 组(3 提示词)
        let rid = ref_repo::insert(
            &pool,
            &ref_repo::NewRefImage {
                name: "r".into(),
                ref_group_id: None,
                ephemeral: false,
                file_path: "/r".into(),
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
        let gid = prompt_repo::create_group(&mut tx, "g", "GG", "", false)
            .await
            .unwrap();
        for i in 1..=3 {
            prompt_repo::insert_prompt(
                &mut tx,
                gid,
                &format!("GG-000{i}"),
                None,
                "t",
                "library",
                None,
            )
            .await
            .unwrap();
        }
        tx.commit().await.unwrap();

        let (batch_id, count) = create_batch(
            &pool,
            "/out",
            "{}",
            &[RefMapping {
                ref_image_id: rid,
                prompt_group_id: gid,
            }],
            1,
        )
        .await
        .unwrap();
        assert_eq!(count, 3, "1 参考图 × 3 提示词 × 抽 1 = 3 任务");
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE batch_id = ?1")
            .bind(batch_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 3);

        // E32 挂靠记忆：创建批次后参考图应记录本次挂靠的组。
        let last: Option<i64> =
            sqlx::query_scalar("SELECT last_group_id FROM ref_images WHERE id = ?1")
                .bind(rid)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(last, Some(gid), "参考图应记住本次挂靠组");

        // E17 D2：抽卡次数展开。1 参考图 × 3 提示词 × 抽 2 = 6 任务，draw_index ∈ {1,2}。
        let (batch2, count2) = create_batch(
            &pool,
            "/out",
            "{}",
            &[RefMapping {
                ref_image_id: rid,
                prompt_group_id: gid,
            }],
            2,
        )
        .await
        .unwrap();
        assert_eq!(count2, 6, "抽卡 2 次 → 任务翻倍");
        // 每个组合恰好 draw_index 1 与 2 各一。
        let draws: Vec<i64> = sqlx::query_scalar(
            "SELECT draw_index FROM tasks WHERE batch_id = ?1 AND prompt_id =
                (SELECT id FROM prompts WHERE code='GG-0001') ORDER BY draw_index",
        )
        .bind(batch2)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(draws, vec![1, 2], "同组合两次抽卡序号为 1、2");
    }

    // 0016：批次一建成，本批用到的参考图与提示词组必须已归档 —— 这是「开始生成后生成页
    // 自动让位给下一批」的全部机制。归档只打时间戳，不得动到软删除位或提示词本身。
    #[tokio::test]
    async fn create_batch_archives_used_refs_and_groups() {
        let (pool, _d) = test_pool().await;
        let rid = ref_repo::insert(
            &pool,
            &ref_repo::NewRefImage {
                name: "r".into(),
                ref_group_id: None,
                ephemeral: false,
                file_path: "/r".into(),
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
        let gid = prompt_repo::create_group(&mut tx, "g", "GG", "", false)
            .await
            .unwrap();
        prompt_repo::insert_prompt(&mut tx, gid, "GG-0001", None, "t", "library", None)
            .await
            .unwrap();
        tx.commit().await.unwrap();

        // 建批次前两者都未归档。
        assert!(ref_repo::list_active(&pool).await.unwrap()[0]
            .archived_at
            .is_none());
        assert!(prompt_repo::list_groups(&pool).await.unwrap()[0]
            .archived_at
            .is_none());

        create_batch(
            &pool,
            "/out",
            "{}",
            &[RefMapping {
                ref_image_id: rid,
                prompt_group_id: gid,
            }],
            1,
        )
        .await
        .unwrap();

        let r = &ref_repo::list_active(&pool).await.unwrap()[0];
        assert!(r.archived_at.is_some(), "本批参考图应随批次创建一并归档");
        assert!(r.deleted_at.is_none(), "归档不是删除：参考图仍在库里");
        let g = &prompt_repo::list_groups(&pool).await.unwrap()[0];
        assert!(g.archived_at.is_some(), "本批提示词组应随批次创建一并归档");
        assert_eq!(
            prompt_repo::count_in_group(&pool, gid).await.unwrap(),
            1,
            "归档不动提示词本身"
        );

        // 取消归档可逆（库页「取消归档」走这条）。
        prompt_repo::set_group_archived(&pool, gid, false)
            .await
            .unwrap();
        ref_repo::set_archived(&pool, rid, false).await.unwrap();
        assert!(prompt_repo::list_groups(&pool).await.unwrap()[0]
            .archived_at
            .is_none());
        assert!(ref_repo::list_active(&pool).await.unwrap()[0]
            .archived_at
            .is_none());
    }
}
