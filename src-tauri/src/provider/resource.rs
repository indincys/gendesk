//! Provider 成功结果的本地资源闸门。
//!
//! 远端请求可以有 250 路并发，但成功响应回到本机后会同时经历 JSON/base64、结果下载、
//! 图片重编码与落盘。若 250 份压缩图和 base64 字符串一起物化，调度并发就直接变成内存
//! 并发。这里仅限制“消费成功结果”的数量，不限制远端生成请求数：远端通道仍能持续填满，
//! 本地则按可预测的小窗口收结果。

use std::sync::{Arc, OnceLock};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// 同时允许在本机完整物化的 provider 成功结果数。
///
/// 四格足以喂满 `files::decode` 的 2–3 个解码槽，并多留一格做网络读取；更多只会堆积
/// base64 字符串与压缩图。双核机器收紧为 2，避免解码之外的 JSON/base64 抢光前台。
pub(crate) fn max_buffered_results() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2)
        .clamp(2, 4)
}

fn semaphore() -> &'static Arc<Semaphore> {
    static SEM: OnceLock<Arc<Semaphore>> = OnceLock::new();
    SEM.get_or_init(|| Arc::new(Semaphore::new(max_buffered_results())))
}

pub(crate) struct ResultPermit {
    _permit: Option<OwnedSemaphorePermit>,
}

/// 信号量不会主动 close；不可达错误分支选择单次放行，避免结果链永久卡死。
pub(crate) async fn acquire_result() -> ResultPermit {
    let permit = Arc::clone(semaphore()).acquire_owned().await.ok();
    ResultPermit { _permit: permit }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // 测试断言失败即失败
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[test]
    fn result_budget_is_small_and_nonzero() {
        assert!((2..=4).contains(&max_buffered_results()));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn result_gate_never_exceeds_budget() {
        let live = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let mut jobs = Vec::new();
        for _ in 0..(max_buffered_results() * 3) {
            let live = Arc::clone(&live);
            let peak = Arc::clone(&peak);
            jobs.push(tokio::spawn(async move {
                let _permit = acquire_result().await;
                let now = live.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(15)).await;
                live.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        for job in jobs {
            job.await.unwrap();
        }
        assert!(peak.load(Ordering::SeqCst) <= max_buffered_results());
    }
}
