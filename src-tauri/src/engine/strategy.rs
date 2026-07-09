//! 调度策略（执行计划 2.4 / 需求 10.2）。
//!
//! RoundRobin 在启用且未满载的 Key 间轮转；SuccessRateFirst 按近 50 次成功率取最高，
//! 同分回退轮转。连续失败的 Key 进入指数退避冷却（30s 起，上限 10 分钟）。

use std::time::Duration;

use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum Strategy {
    RoundRobin,
    SuccessRateFirst,
}

impl Strategy {
    pub fn from_str_or_default(s: &str) -> Self {
        match s {
            "success_rate" => Strategy::SuccessRateFirst,
            _ => Strategy::RoundRobin,
        }
    }
}

/// 可派发的候选 Key（已过滤：启用、未满载、未在冷却）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Candidate {
    pub id: i64,
    /// 近 50 次成功率（0.0–1.0）
    pub success_rate: f64,
}

/// 从候选中选一个 Key。`rr_counter` 承载轮转状态（调度器持有并自增）。
pub fn pick(strategy: Strategy, candidates: &[Candidate], rr_counter: &mut usize) -> Option<i64> {
    if candidates.is_empty() {
        return None;
    }
    // 稳定排序：按 id，保证轮转确定性。
    let mut sorted: Vec<Candidate> = candidates.to_vec();
    sorted.sort_by_key(|c| c.id);

    match strategy {
        Strategy::RoundRobin => {
            let idx = *rr_counter % sorted.len();
            *rr_counter = rr_counter.wrapping_add(1);
            Some(sorted[idx].id)
        }
        Strategy::SuccessRateFirst => {
            // 最高成功率；并列（近似相等）者在其中轮转。
            let best = sorted
                .iter()
                .map(|c| c.success_rate)
                .fold(f64::MIN, f64::max);
            let top: Vec<i64> = sorted
                .iter()
                .filter(|c| (c.success_rate - best).abs() < 1e-9)
                .map(|c| c.id)
                .collect();
            let idx = *rr_counter % top.len();
            *rr_counter = rr_counter.wrapping_add(1);
            top.get(idx).copied()
        }
    }
}

/// 指数退避冷却时长：30s · 2^(fails-1)，上限 10 分钟。
pub fn backoff_duration(consecutive_failures: u32) -> Duration {
    if consecutive_failures == 0 {
        return Duration::ZERO;
    }
    let base = 30u64;
    let shift = (consecutive_failures - 1).min(6); // 2^6=64，30*64=1920s 已超 600s
    let secs = base.saturating_mul(1u64 << shift).min(600);
    Duration::from_secs(secs)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即失败
mod tests {
    use super::*;

    fn cands(v: &[(i64, f64)]) -> Vec<Candidate> {
        v.iter()
            .map(|&(id, r)| Candidate {
                id,
                success_rate: r,
            })
            .collect()
    }

    #[test]
    fn round_robin_rotates_over_available() {
        let c = cands(&[(1, 0.5), (2, 0.9), (3, 0.1)]);
        let mut rr = 0;
        let picks: Vec<i64> = (0..6)
            .map(|_| pick(Strategy::RoundRobin, &c, &mut rr).unwrap())
            .collect();
        assert_eq!(picks, vec![1, 2, 3, 1, 2, 3]);
    }

    #[test]
    fn success_rate_first_picks_highest() {
        let c = cands(&[(1, 0.5), (2, 0.9), (3, 0.1)]);
        let mut rr = 0;
        for _ in 0..3 {
            assert_eq!(pick(Strategy::SuccessRateFirst, &c, &mut rr), Some(2));
        }
    }

    #[test]
    fn success_rate_first_rotates_ties() {
        let c = cands(&[(1, 0.9), (2, 0.9), (3, 0.2)]);
        let mut rr = 0;
        let picks: Vec<i64> = (0..4)
            .map(|_| pick(Strategy::SuccessRateFirst, &c, &mut rr).unwrap())
            .collect();
        assert_eq!(picks, vec![1, 2, 1, 2]);
    }

    #[test]
    fn empty_candidates_none() {
        let mut rr = 0;
        assert_eq!(pick(Strategy::RoundRobin, &[], &mut rr), None);
    }

    #[test]
    fn backoff_grows_and_caps() {
        assert_eq!(backoff_duration(0), Duration::ZERO);
        assert_eq!(backoff_duration(1), Duration::from_secs(30));
        assert_eq!(backoff_duration(2), Duration::from_secs(60));
        assert_eq!(backoff_duration(3), Duration::from_secs(120));
        assert_eq!(backoff_duration(20), Duration::from_secs(600)); // 封顶
    }
}
