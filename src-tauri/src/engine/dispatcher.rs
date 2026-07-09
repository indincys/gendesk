//! 调度器（执行计划 2.4/2.5/2.6）。
//!
//! 单 dispatcher 循环：从队列取待生成/就绪重试任务 → 按策略选可用 Key → 获取该 Key
//! 的 Semaphore 许可 → spawn worker。所有状态迁移由 worker 统一落库；task_attempts 全记录。
//! 暂停 = 停派发；批次全终态自动归档。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use sqlx::SqlitePool;
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;

use super::classify::{classify, decide};
use super::events::{
    BatchSummary, KeyHealth, KeyState, SharedSink, TaskProgress, TaskStatusChanged,
};
use super::progress::{compute_pct, expected_from_history, Phase};
use super::status::TaskStatus;
use super::strategy::{pick, Candidate, Strategy};
use super::{KeyConfig, ProviderFactory};
use crate::db::now_unix;
use crate::db::repo::{api_keys as key_repo, tasks as task_repo};
use crate::files::DataDirs;
use crate::provider::{DownloadProgress, GenRequest, ProgressFn};

const PROGRESS_THROTTLE: Duration = Duration::from_millis(250);
const RATE_WINDOW: i64 = 50;

/// 单 Key 运行时。
struct KeyRuntime {
    config: KeyConfig,
    sem: Arc<Semaphore>,
    cooldown_until: Instant,
    consecutive_failures: u32,
}

/// 防 Mutex 中毒导致 panic：取 into_inner。
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub struct Scheduler {
    pool: SqlitePool,
    dirs: Arc<DataDirs>,
    factory: Arc<dyn ProviderFactory>,
    sink: SharedSink,
    keys: Mutex<Vec<KeyRuntime>>,
    strategy: Mutex<Strategy>,
    rr_counter: Mutex<usize>,
    ready_retry: Mutex<HashMap<i64, Instant>>,
    paused: AtomicBool,
    user_retry: AtomicU32,
    active: AtomicI64,
    /// Key 退避冷却基数（ms）；生产 30s，测试可调小。
    cooldown_base_ms: AtomicU64,
    notify: Arc<Notify>,
}

/// 生产默认 Key 冷却基数（对应 strategy::backoff 的 30s 起点）。
const DEFAULT_COOLDOWN_BASE_MS: u64 = 30_000;

impl Scheduler {
    pub fn new(
        pool: SqlitePool,
        dirs: Arc<DataDirs>,
        factory: Arc<dyn ProviderFactory>,
        sink: SharedSink,
        strategy: Strategy,
        user_retry: u32,
        paused: bool,
    ) -> Self {
        Self {
            pool,
            dirs,
            factory,
            sink,
            keys: Mutex::new(Vec::new()),
            strategy: Mutex::new(strategy),
            rr_counter: Mutex::new(0),
            ready_retry: Mutex::new(HashMap::new()),
            paused: AtomicBool::new(paused),
            user_retry: AtomicU32::new(user_retry),
            active: AtomicI64::new(0),
            cooldown_base_ms: AtomicU64::new(DEFAULT_COOLDOWN_BASE_MS),
            notify: Arc::new(Notify::new()),
        }
    }

    /// 调小 Key 退避冷却基数（测试用，避免真实时钟长等待）。
    #[cfg(test)]
    pub fn set_cooldown_base_ms(&self, ms: u64) {
        self.cooldown_base_ms.store(ms, Ordering::SeqCst);
    }

    /// 连续失败 `consec` 次时的 Key 冷却时长（首次失败不冷却；随后指数退避，上限 20×基数）。
    fn key_cooldown(&self, consec: u32) -> Duration {
        let base = self.cooldown_base_ms.load(Ordering::SeqCst);
        if consec < 2 || base == 0 {
            return Duration::ZERO;
        }
        let steps = (consec - 2).min(6);
        let ms = base
            .saturating_mul(1u64 << steps)
            .min(base.saturating_mul(20));
        Duration::from_millis(ms)
    }

    /// 重设 Key 运行时（添加/更新 Key 后调用）。
    pub fn set_keys(&self, configs: Vec<KeyConfig>) {
        let now = Instant::now();
        let mut keys = lock(&self.keys);
        *keys = configs
            .into_iter()
            .map(|c| KeyRuntime {
                sem: Arc::new(Semaphore::new(c.concurrency_limit.max(1) as usize)),
                config: c,
                cooldown_until: now,
                consecutive_failures: 0,
            })
            .collect();
    }

    pub fn set_strategy(&self, s: Strategy) {
        *lock(&self.strategy) = s;
    }
    pub fn set_user_retry(&self, n: u32) {
        self.user_retry.store(n, Ordering::SeqCst);
    }
    pub fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
    }
    pub fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
        self.notify();
    }
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }
    pub fn notify(&self) {
        self.notify.notify_one();
    }
    pub fn active_concurrency(&self) -> i64 {
        self.active.load(Ordering::SeqCst)
    }

    fn has_enabled_keys(&self) -> bool {
        lock(&self.keys).iter().any(|k| k.config.enabled)
    }

    /// 拉起后台调度循环。
    pub fn spawn_loop(self: Arc<Self>) {
        tokio::spawn(async move {
            loop {
                let n = self.dispatch_once().await;
                if n == 0 {
                    tokio::select! {
                        _ = self.notify.notified() => {}
                        _ = tokio::time::sleep(Duration::from_millis(250)) => {}
                    }
                }
            }
        });
    }

    /// 驱动至空闲（测试用：无活跃、无排队、无待冷却重试）。
    #[cfg(test)]
    pub async fn drive_to_idle(self: &Arc<Self>) {
        loop {
            self.dispatch_once().await;
            let idle = self.active.load(Ordering::SeqCst) == 0
                && lock(&self.ready_retry).is_empty()
                && (self.queued_count().await == 0 || !self.has_enabled_keys());
            if idle {
                break;
            }
            tokio::select! {
                _ = self.notify.notified() => {}
                _ = tokio::time::sleep(Duration::from_millis(50)) => {}
            }
        }
    }

    async fn queued_count(&self) -> i64 {
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tasks WHERE status = 'q'")
            .fetch_one(&self.pool)
            .await
            .unwrap_or(0)
    }

    /// 各 Key 近 50 次成功率快照。
    async fn rates(&self, ids: &[i64]) -> HashMap<i64, f64> {
        let mut map = HashMap::new();
        for &id in ids {
            let (rate, _) = key_repo::success_rate(&self.pool, id, RATE_WINDOW)
                .await
                .unwrap_or((0.0, 0));
            map.insert(id, rate);
        }
        map
    }

    /// 一轮派发：尽可能多地派发就绪任务。返回派发数。
    async fn dispatch_once(self: &Arc<Self>) -> usize {
        if self.paused.load(Ordering::SeqCst) {
            return 0;
        }

        // 1) 就绪 Key 快照（启用、未冷却），并克隆各自 Semaphore。
        let now = Instant::now();
        let eligible: Vec<(i64, Arc<Semaphore>)> = {
            let keys = lock(&self.keys);
            keys.iter()
                .filter(|k| k.config.enabled && k.cooldown_until <= now)
                .map(|k| (k.config.id, k.sem.clone()))
                .collect()
        };
        if eligible.is_empty() {
            return 0;
        }
        let rates = self
            .rates(&eligible.iter().map(|(id, _)| *id).collect::<Vec<_>>())
            .await;

        // 2) 就绪任务：先 ready 重试，再 'q'（FIFO）。
        // 注意：此处只「读」到期重试 id，不移除；仅在成功派发后从 ready_retry 移除，
        // 否则容量不足时被移除的重试任务会成为孤儿（永远停在 retry）。
        let mut queue: Vec<task_repo::TaskRow> = Vec::new();
        let ready_ids: Vec<i64> = {
            let rr = lock(&self.ready_retry);
            rr.iter()
                .filter(|(_, t)| **t <= now)
                .map(|(id, _)| *id)
                .collect()
        };
        for id in ready_ids {
            if let Ok(Some(t)) = task_repo::get_task(&self.pool, id).await {
                if t.status == "retry" {
                    queue.push(t);
                }
            }
        }
        let capacity: usize = eligible.iter().map(|(_, s)| s.available_permits()).sum();
        if capacity > 0 {
            if let Ok(q) = task_repo::fetch_queued(&self.pool, capacity as i64).await {
                queue.extend(q);
            }
        }

        // 3) 逐个派发。
        let mut spawned = 0;
        for task in queue {
            // 当前仍有余量的候选 Key
            let cands: Vec<Candidate> = eligible
                .iter()
                .filter(|(_, s)| s.available_permits() > 0)
                .map(|(id, _)| Candidate {
                    id: *id,
                    success_rate: *rates.get(id).unwrap_or(&0.0),
                })
                .collect();
            if cands.is_empty() {
                break;
            }
            let key_id = {
                let strat = *lock(&self.strategy);
                let mut rr = lock(&self.rr_counter);
                pick(strat, &cands, &mut rr)
            };
            let Some(key_id) = key_id else { break };
            let Some((_, sem)) = eligible.iter().find(|(id, _)| *id == key_id) else {
                continue;
            };
            let permit = match sem.clone().try_acquire_owned() {
                Ok(p) => p,
                Err(_) => continue, // 竞争丢失，换下一个任务再试
            };

            let from = task.status.parse::<TaskStatus>().unwrap_or(TaskStatus::Q);
            if !super::status::can_transition(from, TaskStatus::Run) {
                continue; // 守卫：非法迁移不派发
            }
            if task_repo::mark_running(&self.pool, task.id, key_id)
                .await
                .is_err()
            {
                continue;
            }
            // 派发成功后才从就绪重试集移除。
            if from == TaskStatus::Retry {
                lock(&self.ready_retry).remove(&task.id);
            }
            self.emit_status(&task, TaskStatus::Run, Some(key_id), None, None);
            self.spawn_worker(task, key_id, permit);
            spawned += 1;
        }
        spawned
    }

    fn spawn_worker(
        self: &Arc<Self>,
        task: task_repo::TaskRow,
        key_id: i64,
        permit: OwnedSemaphorePermit,
    ) {
        // active 在 spawn 前自增，确保 drive_to_idle 不会在 worker 真正开始前误判空闲。
        self.active.fetch_add(1, Ordering::SeqCst);
        let me = self.clone();
        tokio::spawn(async move {
            me.run_worker(task, key_id, permit).await;
        });
    }

    async fn run_worker(
        self: Arc<Self>,
        task: task_repo::TaskRow,
        key_id: i64,
        permit: OwnedSemaphorePermit,
    ) {
        let key_cfg = self.key_config(key_id);
        let started = Instant::now();
        let started_unix = now_unix();
        let attempt_id = task_repo::insert_attempt(&self.pool, task.id, key_id, started_unix)
            .await
            .ok();

        // 伪进度 ticker（250ms 节流）。
        let expected = {
            let durs = task_repo::key_success_durations(&self.pool, key_id, 20)
                .await
                .unwrap_or_default();
            expected_from_history(&durs)
        };
        let done = Arc::new(AtomicBool::new(false));
        self.spawn_progress_ticker(task.id, started, expected, done.clone());

        // 组装请求。
        let outcome = match (self.ref_path(task.ref_image_id).await, key_cfg.clone()) {
            (Some(image_path), Some(cfg)) => {
                let provider = self.factory.build(&cfg);
                let req = GenRequest {
                    prompt: task.prompt_text_snapshot.clone(),
                    image_path,
                    model: cfg.model.clone(),
                };
                let progress = self.download_progress_cb(task.id);
                provider.generate(req, Some(progress)).await
            }
            _ => Err(crate::provider::ProviderError::new(
                crate::provider::ProviderErrorKind::Other,
                None,
                "参考图或 Key 配置缺失",
            )),
        };

        done.store(true, Ordering::SeqCst);
        let duration_ms = started.elapsed().as_millis() as i64;

        match outcome {
            Ok(img) => {
                self.on_success(&task, key_id, attempt_id, img, duration_ms)
                    .await;
            }
            Err(perr) => {
                self.on_failure(&task, key_id, attempt_id, perr, duration_ms)
                    .await;
            }
        }

        drop(permit);
        // 先归档 + 汇总，再减 active，避免 drive_to_idle 在归档前误判空闲。
        let _ = task_repo::archive_if_all_terminal(&self.pool, task.batch_id).await;
        self.emit_summary(task.batch_id).await;
        self.active.fetch_sub(1, Ordering::SeqCst);
        self.notify();
    }

    async fn on_success(
        &self,
        task: &task_repo::TaskRow,
        key_id: i64,
        attempt_id: Option<i64>,
        img: crate::provider::GenImage,
        duration_ms: i64,
    ) {
        // 落盘：结果暂存 + 缩略图。
        let results = self.dirs.results();
        let _ = tokio::fs::create_dir_all(&results).await;
        let full = results.join(format!("{}.jpg", task.id));
        let thumb = self.dirs.thumbs().join(format!("result_{}.jpg", task.id));
        let _ = tokio::fs::write(&full, &img.jpeg).await;
        let (full_c, thumb_c) = (full.clone(), thumb.clone());
        let thumb_ok = tokio::task::spawn_blocking(move || {
            crate::files::generate_thumbnail(&full_c, &thumb_c)
        })
        .await
        .map(|r| r.is_ok())
        .unwrap_or(false);
        let thumb_path = if thumb_ok { thumb } else { full.clone() };

        let _ = task_repo::mark_review(
            &self.pool,
            task.id,
            &full.to_string_lossy(),
            &thumb_path.to_string_lossy(),
        )
        .await;
        if let Some(aid) = attempt_id {
            let _ = task_repo::finish_attempt(
                &self.pool,
                aid,
                now_unix(),
                "success",
                None,
                None,
                None,
                duration_ms,
            )
            .await;
        }
        self.on_key_result(key_id, true);
        self.sink.progress(TaskProgress {
            task_id: task.id,
            pct: 100,
            phase: Phase::Saved,
        });
        self.emit_status(task, TaskStatus::Rev, Some(key_id), None, None);
    }

    async fn on_failure(
        &self,
        task: &task_repo::TaskRow,
        key_id: i64,
        attempt_id: Option<i64>,
        perr: crate::provider::ProviderError,
        duration_ms: i64,
    ) {
        let et = classify(perr.kind);
        if let Some(aid) = attempt_id {
            let _ = task_repo::finish_attempt(
                &self.pool,
                aid,
                now_unix(),
                "error",
                Some(et.as_str()),
                Some(&perr.message),
                perr.http_status.map(|s| s as i64),
                duration_ms,
            )
            .await;
        }

        let decision = decide(
            et,
            task.retry_count as u32,
            self.user_retry.load(Ordering::SeqCst),
        );
        if decision.retry {
            let new_rc = task.retry_count + 1;
            let _ = task_repo::mark_retry(&self.pool, task.id, new_rc, et.as_str(), &perr.message)
                .await;
            // 冷却结束再派发。
            let ready_at = Instant::now() + decision.cooldown;
            lock(&self.ready_retry).insert(task.id, ready_at);
            // 定时器：冷却到点唤醒调度。
            let notify = self.notify.clone();
            let cd = decision.cooldown;
            tokio::spawn(async move {
                tokio::time::sleep(cd).await;
                notify.notify_one();
            });
            let mut updated = task.clone();
            updated.retry_count = new_rc;
            self.emit_status(
                &updated,
                TaskStatus::Retry,
                Some(key_id),
                Some(et),
                Some(&perr.message),
            );
        } else {
            let _ = task_repo::mark_fail(&self.pool, task.id, et.as_str(), &perr.message).await;
            self.emit_status(
                task,
                TaskStatus::Fail,
                Some(key_id),
                Some(et),
                Some(&perr.message),
            );
        }
        self.on_key_result(key_id, false);
        if et.suggests_disable_key() {
            self.sink.key_health(KeyHealth {
                key_id,
                state: KeyState::AuthFailed,
                used_concurrency: 0,
                success_rate: 0.0,
            });
        }
    }

    // ---------- 辅助 ----------

    fn key_config(&self, id: i64) -> Option<KeyConfig> {
        lock(&self.keys)
            .iter()
            .find(|k| k.config.id == id)
            .map(|k| k.config.clone())
    }

    async fn ref_path(&self, ref_image_id: i64) -> Option<std::path::PathBuf> {
        sqlx::query_scalar::<_, String>("SELECT file_path FROM ref_images WHERE id = ?1")
            .bind(ref_image_id)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
            .map(std::path::PathBuf::from)
    }

    /// 更新 Key 连续失败/冷却，并推送健康事件。
    fn on_key_result(&self, id: i64, success: bool) {
        let now = Instant::now();
        let mut state = KeyState::Ok;
        {
            let mut keys = lock(&self.keys);
            if let Some(k) = keys.iter_mut().find(|k| k.config.id == id) {
                if success {
                    k.consecutive_failures = 0;
                    k.cooldown_until = now;
                } else {
                    k.consecutive_failures += 1;
                    let cd = self.key_cooldown(k.consecutive_failures);
                    k.cooldown_until = now + cd;
                    if cd > Duration::ZERO {
                        state = KeyState::Limited;
                    }
                }
            }
        }
        self.sink.key_health(KeyHealth {
            key_id: id,
            state,
            used_concurrency: 0,
            success_rate: 0.0,
        });
    }

    fn spawn_progress_ticker(
        &self,
        task_id: i64,
        started: Instant,
        expected: Duration,
        done: Arc<AtomicBool>,
    ) {
        let sink = self.sink.clone();
        tokio::spawn(async move {
            sink.progress(TaskProgress {
                task_id,
                pct: 10,
                phase: Phase::RequestStarted,
            });
            loop {
                tokio::time::sleep(PROGRESS_THROTTLE).await;
                if done.load(Ordering::SeqCst) {
                    break;
                }
                let pct = compute_pct(Phase::Generating, started.elapsed(), expected, 0.0);
                sink.progress(TaskProgress {
                    task_id,
                    pct,
                    phase: Phase::Generating,
                });
            }
        });
    }

    fn download_progress_cb(&self, task_id: i64) -> ProgressFn {
        let sink = self.sink.clone();
        Arc::new(move |p: DownloadProgress| {
            let frac = p
                .total
                .map(|t| p.received as f32 / t.max(1) as f32)
                .unwrap_or(0.0);
            let pct = compute_pct(Phase::Downloading, Duration::ZERO, Duration::ZERO, frac);
            sink.progress(TaskProgress {
                task_id,
                pct,
                phase: Phase::Downloading,
            });
        })
    }

    fn emit_status(
        &self,
        task: &task_repo::TaskRow,
        status: TaskStatus,
        api_key_id: Option<i64>,
        error_type: Option<super::classify::ErrorType>,
        error_message: Option<&str>,
    ) {
        self.sink.status_changed(TaskStatusChanged {
            task_id: task.id,
            batch_id: task.batch_id,
            status,
            error_type,
            error_message: error_message.map(|s| s.to_string()),
            retry_count: task.retry_count,
            api_key_id,
        });
    }

    async fn emit_summary(&self, batch_id: i64) {
        if let Ok(counts) = task_repo::counts_for_batch(&self.pool, batch_id).await {
            self.sink.batch_summary(BatchSummary {
                batch_id,
                counts,
                active_concurrency: self.active.load(Ordering::SeqCst),
                paused: self.paused.load(Ordering::SeqCst),
            });
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即失败
mod tests {
    use super::*;
    use crate::db::test_support::test_pool;
    use crate::engine::events::test_sink::CollectingSink;
    use crate::engine::{create_batch, RefMapping};
    use crate::provider::{GenImage, ImageProvider, ProviderError, ProviderErrorKind};
    use std::io::Cursor;
    use std::sync::atomic::AtomicI64;

    fn tiny_jpeg() -> Vec<u8> {
        let img = image::RgbImage::from_pixel(4, 4, image::Rgb([120, 130, 140]));
        let mut buf = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut buf, image::ImageFormat::Jpeg)
            .unwrap();
        buf.into_inner()
    }

    type OutcomeFn = Arc<dyn Fn(i64) -> Result<(), ProviderErrorKind> + Send + Sync>;

    struct FakeFactory {
        peak: Arc<AtomicI64>,
        cur: Arc<AtomicI64>,
        hold: Duration,
        outcome: OutcomeFn,
        jpeg: Arc<Vec<u8>>,
    }

    struct FakeProvider {
        key_id: i64,
        peak: Arc<AtomicI64>,
        cur: Arc<AtomicI64>,
        hold: Duration,
        outcome: OutcomeFn,
        jpeg: Arc<Vec<u8>>,
    }

    impl ProviderFactory for FakeFactory {
        fn build(&self, key: &KeyConfig) -> Arc<dyn ImageProvider> {
            Arc::new(FakeProvider {
                key_id: key.id,
                peak: self.peak.clone(),
                cur: self.cur.clone(),
                hold: self.hold,
                outcome: self.outcome.clone(),
                jpeg: self.jpeg.clone(),
            })
        }
    }

    #[async_trait::async_trait]
    impl ImageProvider for FakeProvider {
        async fn generate(
            &self,
            _req: GenRequest,
            _p: Option<ProgressFn>,
        ) -> Result<GenImage, ProviderError> {
            let now = self.cur.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(now, Ordering::SeqCst);
            tokio::time::sleep(self.hold).await;
            let res = (self.outcome)(self.key_id);
            self.cur.fetch_sub(1, Ordering::SeqCst);
            match res {
                Ok(()) => Ok(GenImage {
                    jpeg: (*self.jpeg).clone(),
                }),
                Err(kind) => Err(ProviderError::new(kind, None, "fake 错误")),
            }
        }
    }

    struct Harness {
        sched: Arc<Scheduler>,
        sink: Arc<CollectingSink>,
        peak: Arc<AtomicI64>,
        pool: SqlitePool,
        _dir: tempfile::TempDir,
    }

    async fn setup(
        num_prompts: usize,
        keys: &[(i64, u32)], // (id, concurrency)
        hold: Duration,
        outcome: OutcomeFn,
        strategy: Strategy,
        user_retry: u32,
    ) -> Harness {
        let (pool, _dir) = test_pool().await;
        // 分组 + N 提示词 + 1 参考图
        let mut tx = pool.begin().await.unwrap();
        sqlx::query("INSERT INTO prompt_groups (id,name,prefix,scene,is_temp,created_at) VALUES (1,'g','GG','',0,0)").execute(&mut *tx).await.unwrap();
        for i in 1..=num_prompts {
            sqlx::query("INSERT INTO prompts (group_id,code,text,status,source,created_at,updated_at) VALUES (1,?1,?2,'active','library',0,0)")
                .bind(format!("GG-{i:04}")).bind(format!("prompt {i}")).execute(&mut *tx).await.unwrap();
        }
        sqlx::query("INSERT INTO ref_images (id,name,file_path,thumb_path,width,height,file_size,created_at) VALUES (1,'r','/nonexistent','/t',1,1,1,0)").execute(&mut *tx).await.unwrap();
        for (id, conc) in keys {
            sqlx::query("INSERT INTO api_keys (id,name,keyring_account,base_url,model,concurrency_limit,enabled,created_at) VALUES (?1,'k',?2,'http://x/v1','m',?3,1,0)")
                .bind(id).bind(format!("acct-{id}")).bind(*conc as i64).execute(&mut *tx).await.unwrap();
        }
        tx.commit().await.unwrap();

        create_batch(
            &pool,
            "/out",
            "{}",
            &[RefMapping {
                ref_image_id: 1,
                prompt_group_id: 1,
            }],
        )
        .await
        .unwrap();

        let peak = Arc::new(AtomicI64::new(0));
        let cur = Arc::new(AtomicI64::new(0));
        let factory = Arc::new(FakeFactory {
            peak: peak.clone(),
            cur,
            hold,
            outcome,
            jpeg: Arc::new(tiny_jpeg()),
        });
        let sink = CollectingSink::shared();
        let dirs = Arc::new(DataDirs::new(_dir.path()));
        dirs.init().unwrap();
        let sched = Arc::new(Scheduler::new(
            pool.clone(),
            dirs,
            factory,
            sink.clone(),
            strategy,
            user_retry,
            false,
        ));
        let configs: Vec<KeyConfig> = keys
            .iter()
            .map(|(id, conc)| KeyConfig {
                id: *id,
                base_url: "http://x/v1".into(),
                model: "m".into(),
                api_key: "sk".into(),
                concurrency_limit: *conc,
                enabled: true,
            })
            .collect();
        sched.set_cooldown_base_ms(2); // 测试用极短冷却，避免真实时钟等待
        sched.set_keys(configs);
        Harness {
            sched,
            sink,
            peak,
            pool,
            _dir,
        }
    }

    async fn status_of(pool: &SqlitePool, id: i64) -> String {
        task_repo::get_task(pool, id).await.unwrap().unwrap().status
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn all_tasks_reach_review_on_success() {
        let h = setup(
            8,
            &[(1, 3)],
            Duration::from_millis(50),
            Arc::new(|_| Ok(())),
            Strategy::RoundRobin,
            1,
        )
        .await;
        h.sched.drive_to_idle().await;
        let revs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE status='rev'")
            .fetch_one(&h.pool)
            .await
            .unwrap();
        assert_eq!(revs, 8);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn never_exceeds_per_key_concurrency() {
        let h = setup(
            12,
            &[(1, 2)],
            Duration::from_millis(100),
            Arc::new(|_| Ok(())),
            Strategy::RoundRobin,
            1,
        )
        .await;
        h.sched.drive_to_idle().await;
        assert!(
            h.peak.load(Ordering::SeqCst) <= 2,
            "峰值并发 {} 超过上限 2",
            h.peak.load(Ordering::SeqCst)
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn round_robin_distributes_across_keys() {
        let h = setup(
            8,
            &[(1, 1), (2, 1)],
            Duration::from_millis(30),
            Arc::new(|_| Ok(())),
            Strategy::RoundRobin,
            1,
        )
        .await;
        h.sched.drive_to_idle().await;
        let k1: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM task_attempts WHERE api_key_id=1")
            .fetch_one(&h.pool)
            .await
            .unwrap();
        let k2: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM task_attempts WHERE api_key_id=2")
            .fetch_one(&h.pool)
            .await
            .unwrap();
        assert_eq!(k1 + k2, 8);
        assert!(k1 >= 3 && k2 >= 3, "分配不均：k1={k1} k2={k2}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn timeout_retries_once_then_fails() {
        // 单任务，始终超时 → 重试 1 次 → fail，2 次 attempts
        let h = setup(
            1,
            &[(1, 2)],
            Duration::from_millis(10),
            Arc::new(|_| Err(ProviderErrorKind::Timeout)),
            Strategy::RoundRobin,
            1,
        )
        .await;
        h.sched.drive_to_idle().await;
        assert_eq!(status_of(&h.pool, 1).await, "fail");
        let t = task_repo::get_task(&h.pool, 1).await.unwrap().unwrap();
        assert_eq!(t.retry_count, 1);
        assert_eq!(t.error_type.as_deref(), Some("Timeout"));
        let attempts: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM task_attempts WHERE task_id=1")
                .fetch_one(&h.pool)
                .await
                .unwrap();
        assert_eq!(attempts, 2);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn pause_stops_new_dispatch() {
        let h = setup(
            6,
            &[(1, 2)],
            Duration::from_millis(50),
            Arc::new(|_| Ok(())),
            Strategy::RoundRobin,
            1,
        )
        .await;
        h.sched.pause();
        // 暂停下派发应为 0
        assert_eq!(h.sched.dispatch_once().await, 0);
        let running: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE status IN ('run','rev')")
                .fetch_one(&h.pool)
                .await
                .unwrap();
        assert_eq!(running, 0);
        // 恢复后跑完
        h.sched.resume();
        h.sched.drive_to_idle().await;
        let revs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE status='rev'")
            .fetch_one(&h.pool)
            .await
            .unwrap();
        assert_eq!(revs, 6);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn batch_archived_when_all_terminal() {
        // 全部 Auth 失败（不重试）→ 全 fail → 批次归档
        let h = setup(
            4,
            &[(1, 2)],
            Duration::from_millis(10),
            Arc::new(|_| Err(ProviderErrorKind::Auth)),
            Strategy::RoundRobin,
            1,
        )
        .await;
        h.sched.drive_to_idle().await;
        let fails: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE status='fail'")
            .fetch_one(&h.pool)
            .await
            .unwrap();
        assert_eq!(fails, 4);
        let status: String = sqlx::query_scalar("SELECT status FROM batches WHERE id=1")
            .fetch_one(&h.pool)
            .await
            .unwrap();
        assert_eq!(status, "archived");
    }

    // 1→500 压测（执行计划 2.8）：500 任务 × 6 Key，注入 ~5% 超时/3% 违规/2% Auth。
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[ignore = "压测：cargo test -- --ignored"]
    async fn stress_500_tasks_all_reach_terminal_or_review() {
        let counter = Arc::new(AtomicI64::new(0));
        let outcome: OutcomeFn = Arc::new(move |_key_id: i64| {
            // 计数器 × Knuth 乘法散列 → 稳定分布：~5% 超时 / 3% 违规 / 2% Auth / 90% 成功
            let n = counter.fetch_add(1, Ordering::SeqCst) as u64;
            let r = n.wrapping_mul(2654435761) % 100;
            match r {
                0..=4 => Err(ProviderErrorKind::Timeout),
                5..=7 => Err(ProviderErrorKind::ContentPolicy),
                8..=9 => Err(ProviderErrorKind::Auth),
                _ => Ok(()),
            }
        });
        let keys: Vec<(i64, u32)> = (1..=6).map(|i| (i, 3)).collect();
        let h = setup(
            500,
            &keys,
            Duration::from_millis(5),
            outcome,
            Strategy::SuccessRateFirst,
            1,
        )
        .await;
        h.sched.drive_to_idle().await;
        // 全部达终态（rev/pass/rej/fail），无 q/run/retry 残留
        let stuck: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE status IN ('q','run','retry')")
                .fetch_one(&h.pool)
                .await
                .unwrap();
        assert_eq!(stuck, 0, "有任务未达终态");
        let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks")
            .fetch_one(&h.pool)
            .await
            .unwrap();
        assert_eq!(total, 500);
        // 每个 rev 任务都应有结果图路径
        let rev_no_path: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM tasks WHERE status='rev' AND result_image_path IS NULL",
        )
        .fetch_one(&h.pool)
        .await
        .unwrap();
        assert_eq!(rev_no_path, 0);
        // sink 至少收到状态事件
        assert!(!h.sink.statuses.lock().unwrap().is_empty());
    }
}
