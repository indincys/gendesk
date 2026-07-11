//! 调度器（执行计划 2.4/2.5/2.6）。
//!
//! 单 dispatcher 循环：从队列取待生成/就绪重试任务 → 按策略选可用 Key → 获取该 Key
//! 的 Semaphore 许可 → spawn worker。所有状态迁移由 worker 统一落库；task_attempts 全记录。
//! 暂停 = 停派发；批次全终态自动归档。

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use sqlx::SqlitePool;
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;

use super::classify::{classify, decide, ErrorType};
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
    /// 连续 Auth/欠费失败次数（E18 熔断计数，成功即清零）。
    auth_failures: u32,
    /// 近一分钟派发时刻（E18 RPM 滑动窗口）。
    request_times: VecDeque<Instant>,
}

/// 连续 Auth/欠费失败达到此阈值即自动熔断该 Key（E18）。
const CIRCUIT_BREAK_THRESHOLD: u32 = 3;
/// RPM 滑动窗口长度。
const RPM_WINDOW: Duration = Duration::from_secs(60);

/// 防 Mutex 中毒导致 panic：取 into_inner。
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// RPM 滑动窗口检查（E18）：裁掉窗口外旧记录后，判断近一分钟派发数是否仍低于上限。
/// 无 rpm_limit（None）恒放行。
fn rpm_ok(k: &mut KeyRuntime, now: Instant) -> bool {
    let Some(limit) = k.config.rpm_limit else {
        return true;
    };
    while let Some(front) = k.request_times.front() {
        if now.duration_since(*front) > RPM_WINDOW {
            k.request_times.pop_front();
        } else {
            break;
        }
    }
    (k.request_times.len() as u32) < limit
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
    /// 跨 Key 连续任务失败计数（E05 全局熔断）。任一任务成功即清零。
    global_fail_streak: AtomicU32,
    /// 全局熔断阈值（连续失败达此数自动暂停队列）。0 = 关闭。
    global_fail_threshold: AtomicU32,
    /// 自动暂停原因（E05）；None = 非自动暂停。手动继续队列时清空。
    auto_pause_reason: Mutex<Option<String>>,
    notify: Arc<Notify>,
}

/// 生产默认 Key 冷却基数（对应 strategy::backoff 的 30s 起点）。
const DEFAULT_COOLDOWN_BASE_MS: u64 = 30_000;
/// 全局熔断默认阈值（跨 Key 连续失败 10 次自动暂停）。
const DEFAULT_GLOBAL_FAIL_THRESHOLD: u32 = 10;

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
            global_fail_streak: AtomicU32::new(0),
            global_fail_threshold: AtomicU32::new(DEFAULT_GLOBAL_FAIL_THRESHOLD),
            auto_pause_reason: Mutex::new(None),
            notify: Arc::new(Notify::new()),
        }
    }

    /// 设置全局熔断阈值（E05；0 = 关闭）。
    pub fn set_global_fail_threshold(&self, n: u32) {
        self.global_fail_threshold.store(n, Ordering::SeqCst);
    }

    /// 当前自动暂停原因（None = 非自动暂停）。
    pub fn auto_pause_reason(&self) -> Option<String> {
        lock(&self.auto_pause_reason).clone()
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
                auth_failures: 0,
                request_times: VecDeque::new(),
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
        // 手动继续队列即消费掉自动暂停：清原因 + 重置连续失败计数，避免立刻再次熔断。
        *lock(&self.auto_pause_reason) = None;
        self.global_fail_streak.store(0, Ordering::SeqCst);
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

        // 1) 就绪 Key 快照（启用、未冷却、未超 RPM），并克隆各自 Semaphore。
        let now = Instant::now();
        let eligible: Vec<(i64, Arc<Semaphore>)> = {
            let mut keys = lock(&self.keys);
            keys.iter_mut()
                .filter_map(|k| {
                    if k.config.enabled && k.cooldown_until <= now && rpm_ok(k, now) {
                        Some((k.config.id, k.sem.clone()))
                    } else {
                        None
                    }
                })
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
            // E18：记录本次派发时刻用于 RPM 滑动窗口。
            self.record_request(key_id, Instant::now());
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
                // E16 / D1：取批次生成参数快照，仅透传显式设置项。
                let params = self.batch_params(task.batch_id).await;
                let req = GenRequest {
                    prompt: task.prompt_text_snapshot.clone(),
                    image_path,
                    model: cfg.model.clone(),
                    params,
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
        // E04：仅在批次真正归档（全终态）的那一次发系统通知。
        if task_repo::archive_if_all_terminal(&self.pool, task.batch_id)
            .await
            .unwrap_or(false)
        {
            self.notify_batch_complete(task.batch_id).await;
        }
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
        // 输出处理（任务1）：默认为 jpg；用户保留原格式时 ext 可能是 png。
        let full = results.join(format!("{}.{}", task.id, img.ext));
        let thumb = self.dirs.thumbs().join(format!("result_{}.jpg", task.id));
        // 写盘失败（磁盘满等）不能静默转 rev，否则用户在验收页看到空图；标失败让其可重试。
        if let Err(e) = tokio::fs::write(&full, &img.bytes).await {
            tracing::error!(task_id = task.id, error = %e, "生成结果写盘失败，任务标记为失败");
            let msg = format!("结果写盘失败：{e}");
            if let Some(aid) = attempt_id {
                let _ = task_repo::finish_attempt(
                    &self.pool,
                    aid,
                    now_unix(),
                    "error",
                    Some(ErrorType::Other.as_str()),
                    Some(&msg),
                    None,
                    duration_ms,
                )
                .await;
            }
            let _ =
                task_repo::mark_fail(&self.pool, task.id, ErrorType::Other.as_str(), &msg).await;
            // API 调用本身成功，不惩罚该 Key。
            self.on_key_result(key_id, true);
            // E05：写盘失败也是终态失败，计入全局熔断（磁盘满会连续触发）。
            if self.note_global_outcome(false) {
                self.trip_global_breaker(self.global_fail_threshold.load(Ordering::SeqCst));
            }
            self.emit_status(
                task,
                TaskStatus::Fail,
                Some(key_id),
                Some(ErrorType::Other),
                Some(&msg),
            );
            return;
        }
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
        // E05：一次成功清零跨 Key 连续失败计数。
        self.note_global_outcome(true);
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
            // E05：终态失败计入全局熔断（重试态不计，尚未失败）。
            if self.note_global_outcome(false) {
                self.trip_global_breaker(self.global_fail_threshold.load(Ordering::SeqCst));
            }
        }
        self.on_key_result(key_id, false);
        if et.suggests_disable_key() {
            self.sink.key_health(KeyHealth {
                key_id,
                state: KeyState::AuthFailed,
                used_concurrency: 0,
                success_rate: 0.0,
            });
            // E18：连续 Auth/欠费失败达阈值 → 自动熔断该 Key（停用 + 通知）。
            if self.bump_auth_failure(key_id) {
                self.trip_breaker(key_id).await;
            }
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
        // E41：优先用压缩上传副本（upload_path），无则用原图（file_path）。
        sqlx::query_scalar::<_, String>(
            "SELECT COALESCE(upload_path, file_path) FROM ref_images WHERE id = ?1",
        )
        .bind(ref_image_id)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()
        .map(std::path::PathBuf::from)
    }

    /// 批次生成参数快照（E16 / D1）。查不到或解析失败退化为「全部空」（不传参）。
    async fn batch_params(&self, batch_id: i64) -> crate::provider::GenParams {
        let json = sqlx::query_scalar::<_, String>("SELECT params_json FROM batches WHERE id = ?1")
            .bind(batch_id)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| "{}".into());
        crate::provider::GenParams::from_json(&json)
    }

    /// 更新 Key 连续失败/冷却，并推送健康事件。成功时同时清零熔断计数。
    fn on_key_result(&self, id: i64, success: bool) {
        let now = Instant::now();
        let mut state = KeyState::Ok;
        {
            let mut keys = lock(&self.keys);
            if let Some(k) = keys.iter_mut().find(|k| k.config.id == id) {
                if success {
                    k.consecutive_failures = 0;
                    k.auth_failures = 0;
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

    /// 记录派发时刻（E18 RPM 滑动窗口）。
    fn record_request(&self, id: i64, now: Instant) {
        let mut keys = lock(&self.keys);
        if let Some(k) = keys.iter_mut().find(|k| k.config.id == id) {
            k.request_times.push_back(now);
            // 顺手裁掉窗口外的旧记录，避免无界增长。
            while let Some(front) = k.request_times.front() {
                if now.duration_since(*front) > RPM_WINDOW {
                    k.request_times.pop_front();
                } else {
                    break;
                }
            }
        }
    }

    /// 累加一次 Auth/欠费失败并返回是否达到熔断阈值（E18）。达阈值时同时把运行时
    /// 该 Key 置为停用，避免后续轮次继续派发到它。
    fn bump_auth_failure(&self, id: i64) -> bool {
        let mut keys = lock(&self.keys);
        if let Some(k) = keys.iter_mut().find(|k| k.config.id == id) {
            k.auth_failures += 1;
            if k.auth_failures >= CIRCUIT_BREAK_THRESHOLD {
                k.config.enabled = false; // 运行时立即停用
                k.auth_failures = 0;
                return true;
            }
        }
        false
    }

    /// 熔断 Key（E18）：落库停用 + 置熔断位，推健康事件与系统通知。
    async fn trip_breaker(&self, id: i64) {
        let _ = key_repo::trip_circuit(&self.pool, id).await;
        self.sink.key_health(KeyHealth {
            key_id: id,
            state: KeyState::Disabled,
            used_concurrency: 0,
            success_rate: 0.0,
        });
        self.sink.notify(
            "API Key 已熔断".into(),
            format!("Key #{id} 连续鉴权/欠费失败已达 {CIRCUIT_BREAK_THRESHOLD} 次，已自动停用。请到设置检查并恢复。"),
        );
    }

    /// 记录一次终态任务结果用于 E05 全局熔断：成功清零跨 Key 连续失败计数；
    /// 终态失败累加，达阈值（>0）返回 true 表示应触发全局熔断。
    fn note_global_outcome(&self, success: bool) -> bool {
        if success {
            self.global_fail_streak.store(0, Ordering::SeqCst);
            return false;
        }
        let threshold = self.global_fail_threshold.load(Ordering::SeqCst);
        let streak = self.global_fail_streak.fetch_add(1, Ordering::SeqCst) + 1;
        threshold > 0 && streak >= threshold
    }

    /// 全局熔断（E05）：跨 Key 连续失败达阈值 → 自动暂停队列 + 记原因 + 系统通知。
    /// 已处于自动暂停态则跳过，避免重复通知。
    fn trip_global_breaker(&self, threshold: u32) {
        {
            let mut reason = lock(&self.auto_pause_reason);
            if reason.is_some() {
                return;
            }
            *reason = Some(format!("连续 {threshold} 个任务失败，已自动暂停队列"));
        }
        self.paused.store(true, Ordering::SeqCst);
        self.sink.notify(
            "队列已自动暂停".into(),
            format!("连续 {threshold} 个任务失败，队列已自动暂停以防继续消耗额度。请检查设置后继续队列。"),
        );
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

    /// 批次完成系统通知（E04）：全终态归档的那一次发一条，附通过/未通过/失败计数。
    async fn notify_batch_complete(&self, batch_id: i64) {
        if let Ok(c) = task_repo::counts_for_batch(&self.pool, batch_id).await {
            self.sink.notify(
                format!("批次 #{batch_id} 已完成"),
                format!(
                    "共 {} 个任务：待验收 {} · 通过 {} · 未通过 {} · 失败 {}",
                    c.total, c.review, c.passed, c.rejected, c.failed
                ),
            );
        }
    }

    /// 更新 Dock/任务栏角标（E04）：全库待验收任务数（无人值守时提示「有图待验收」）。
    async fn update_badge(&self) {
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE status = 'rev'")
            .fetch_one(&self.pool)
            .await
            .unwrap_or(0);
        self.sink.set_badge(if n > 0 { Some(n) } else { None });
    }

    /// 主动补发某批次汇总（供命令层在验收改动任务态后驱动徽章）。
    pub async fn emit_summary(&self, batch_id: i64) {
        if let Ok(counts) = task_repo::counts_for_batch(&self.pool, batch_id).await {
            self.sink.batch_summary(BatchSummary {
                batch_id,
                counts,
                active_concurrency: self.active.load(Ordering::SeqCst),
                paused: self.paused.load(Ordering::SeqCst),
                auto_pause_reason: self.auto_pause_reason(),
            });
        }
        // 待验收角标随汇总同步刷新（验收/生成/删除后均经此路径）。
        self.update_badge().await;
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
                    bytes: (*self.jpeg).clone(),
                    ext: "jpg".to_string(),
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
            1,
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
                rpm_limit: None,
            })
            .collect();
        sched.set_cooldown_base_ms(2); // 测试用极短冷却，避免真实时钟等待
        sched.set_global_fail_threshold(0); // 默认关闭全局熔断，E05 用例单独打开
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
        // 全部 ContentPolicy 失败（user_retry=0 → 首次即终态，且不触发 E18 Key 熔断）
        // → 全 fail → 批次归档。
        // 注：原用 Auth 注入，E18（单 Key 连续 3 次 Auth 熔断停用）落地后会把第 4 个任务
        // 卡在 q（无可用 Key），批次不再全终态。本测试意在验「全终态即归档」，改用不触发
        // 熔断的终态错误以隔离该无关特性；断言（4 fail + archived）保持不变。
        let h = setup(
            4,
            &[(1, 2)],
            Duration::from_millis(10),
            Arc::new(|_| Err(ProviderErrorKind::ContentPolicy)),
            Strategy::RoundRobin,
            0,
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

    // E18：连续 Auth 失败达阈值 → Key 自动熔断（DB enabled=0 + circuit_broken=1），
    // 任务切到其它可用 Key 继续完成。
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn key_circuit_breaks_and_tasks_fail_over() {
        // key 1 恒 Auth 失败，key 2 恒成功。
        let h = setup(
            10,
            &[(1, 1), (2, 1)],
            Duration::from_millis(5),
            Arc::new(|kid| {
                if kid == 1 {
                    Err(ProviderErrorKind::Auth)
                } else {
                    Ok(())
                }
            }),
            Strategy::RoundRobin,
            1,
        )
        .await;
        h.sched.drive_to_idle().await;

        let (enabled, broken): (i64, i64) =
            sqlx::query_as("SELECT enabled, circuit_broken FROM api_keys WHERE id = 1")
                .fetch_one(&h.pool)
                .await
                .unwrap();
        assert_eq!((enabled, broken), (0, 1), "连续 Auth 失败后 key1 应被熔断");

        // 熔断后剩余任务切到 key2 成功产出（至少有若干 rev）。
        let revs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE status = 'rev'")
            .fetch_one(&h.pool)
            .await
            .unwrap();
        assert!(revs > 0, "任务应能切到其它 Key 完成，实际 rev={revs}");
    }

    // E05：跨 Key 连续失败达阈值 → 自动暂停队列（在烧完前停住），记录原因，
    // 剩余 q 任务不再派发；resume 后清原因并可继续。
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn global_breaker_pauses_queue_before_burning_all() {
        // 30 任务全部 ContentPolicy（非重试、不停用 Key），阈值 5。
        let h = setup(
            30,
            &[(1, 2)],
            Duration::from_millis(5),
            Arc::new(|_| Err(ProviderErrorKind::ContentPolicy)),
            Strategy::RoundRobin,
            0, // user_retry=0：ContentPolicy 首次即终态失败
        )
        .await;
        h.sched.set_global_fail_threshold(5);

        // 驱动直到自动暂停或超时（避免 drive_to_idle 在暂停态死循环）。
        let mut tripped = false;
        for _ in 0..300 {
            h.sched.dispatch_once().await;
            if h.sched.is_paused() {
                tripped = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(tripped, "连续失败达阈值应自动暂停队列");
        assert!(
            h.sched.is_paused() && h.sched.auto_pause_reason().is_some(),
            "自动暂停应记录原因"
        );
        // 停在烧完之前：仍有排队任务。
        let remaining_q: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE status='q'")
            .fetch_one(&h.pool)
            .await
            .unwrap();
        assert!(
            remaining_q > 0,
            "应在烧完 30 任务前停住，实际剩余 q={remaining_q}"
        );

        // 恢复队列：清原因、重置计数。
        h.sched.resume();
        assert!(
            h.sched.auto_pause_reason().is_none(),
            "resume 应清空自动暂停原因"
        );
        assert!(!h.sched.is_paused());
    }

    // E18：RPM 滑动窗口——达到上限即拒派，窗口滑过后恢复。
    #[test]
    fn rpm_ok_enforces_limit_and_slides() {
        let now = Instant::now();
        let mut k = KeyRuntime {
            config: KeyConfig {
                id: 1,
                base_url: "http://x/v1".into(),
                model: "m".into(),
                api_key: "sk".into(),
                concurrency_limit: 1,
                enabled: true,
                rpm_limit: Some(2),
            },
            sem: Arc::new(Semaphore::new(1)),
            cooldown_until: now,
            consecutive_failures: 0,
            auth_failures: 0,
            request_times: VecDeque::new(),
        };
        assert!(rpm_ok(&mut k, now), "空窗口应放行");
        k.request_times.push_back(now);
        assert!(rpm_ok(&mut k, now), "1 < 2 放行");
        k.request_times.push_back(now);
        assert!(!rpm_ok(&mut k, now), "达上限 2 应拒派");
        // 窗口滑过：旧记录被裁掉，恢复放行。
        let later = now + RPM_WINDOW + Duration::from_secs(1);
        assert!(rpm_ok(&mut k, later), "窗口外旧记录裁掉后应恢复放行");
        assert!(k.request_times.is_empty(), "过期记录应已裁剪");
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
