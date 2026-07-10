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

use crate::db::repo::{api_keys as key_repo, prompts as prompt_repo, tasks as task_repo};
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
                concurrency_limit: r.concurrency_limit.clamp(1, 10) as u32,
                enabled: r.enabled != 0,
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
    }
    // 每个组合展开 draws 个任务，draw_index ∈ 1..=draws（供输出命名去重）。
    for (ref_id, prompt_id, snapshot) in &combos {
        for draw_index in 1..=draws {
            task_repo::insert_task(&mut tx, batch_id, *ref_id, *prompt_id, snapshot, draw_index)
                .await?;
        }
    }
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
    pub fn is_paused(&self) -> bool {
        self.scheduler.is_paused()
    }
    pub fn set_strategy(&self, s: Strategy) {
        self.scheduler.set_strategy(s);
    }
    pub fn set_user_retry(&self, n: u32) {
        self.scheduler.set_user_retry(n);
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

    #[tokio::test]
    async fn create_batch_expands_task_count_correctly() {
        let (pool, _d) = test_pool().await;
        // 1 参考图 + 1 组(3 提示词)
        let rid = ref_repo::insert(
            &pool,
            &ref_repo::NewRefImage {
                name: "r".into(),
                group_id: None,
                file_path: "/r".into(),
                thumb_path: "/t".into(),
                width: 1,
                height: 1,
                file_size: 1,
            },
        )
        .await
        .unwrap();
        let mut tx = pool.begin().await.unwrap();
        let gid = prompt_repo::create_group(&mut tx, "g", "GG", "", false)
            .await
            .unwrap();
        for i in 1..=3 {
            prompt_repo::insert_prompt(&mut tx, gid, &format!("GG-000{i}"), None, "t", "library")
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
}
