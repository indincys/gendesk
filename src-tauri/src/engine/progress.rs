//! 阶段型伪进度（执行计划 2.6 / R4）。
//!
//! 生图 API 无真实进度：排队 0% → 请求发起 10% → elapsed/expected 线性到 90% 封顶
//! → url 下载 90–98% → 落盘 100%。expected = 该 Key 近 20 次成功耗时均值（无记录用 60s）。

use std::time::Duration;

use serde::{Deserialize, Serialize};
use specta::Type;

const DEFAULT_EXPECTED_SECS: u64 = 60;
const HISTORY_WINDOW: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    Queued,
    RequestStarted,
    Generating,
    Downloading,
    Saved,
}

impl Phase {
    pub fn as_str(&self) -> &'static str {
        match self {
            Phase::Queued => "queued",
            Phase::RequestStarted => "requestStarted",
            Phase::Generating => "generating",
            Phase::Downloading => "downloading",
            Phase::Saved => "saved",
        }
    }
}

/// 依据阶段计算伪进度百分比（0–100）。
///
/// - `elapsed` / `expected` 仅在 Generating 阶段有意义；
/// - `download_fraction` 仅在 Downloading 阶段有意义（0.0–1.0）。
pub fn compute_pct(
    phase: Phase,
    elapsed: Duration,
    expected: Duration,
    download_fraction: f32,
) -> u8 {
    match phase {
        Phase::Queued => 0,
        Phase::RequestStarted => 10,
        Phase::Generating => {
            let exp = expected.as_secs_f64().max(1.0);
            let frac = (elapsed.as_secs_f64() / exp).clamp(0.0, 1.0);
            // 10% 起，线性到 90% 封顶
            (10.0 + frac * 80.0).min(90.0).round() as u8
        }
        Phase::Downloading => {
            let f = download_fraction.clamp(0.0, 1.0) as f64;
            (90.0 + f * 8.0).min(98.0).round() as u8
        }
        Phase::Saved => 100,
    }
}

/// 由该 Key 近 20 次成功耗时（ms）估算 expected；无记录用 60s。
pub fn expected_from_history(success_durations_ms: &[i64]) -> Duration {
    let window: Vec<i64> = success_durations_ms
        .iter()
        .rev()
        .take(HISTORY_WINDOW)
        .copied()
        .filter(|d| *d > 0)
        .collect();
    if window.is_empty() {
        return Duration::from_secs(DEFAULT_EXPECTED_SECS);
    }
    let mean = window.iter().sum::<i64>() as f64 / window.len() as f64;
    Duration::from_millis(mean.round().max(1.0) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phases_have_expected_bases() {
        let e = Duration::from_secs(60);
        assert_eq!(compute_pct(Phase::Queued, Duration::ZERO, e, 0.0), 0);
        assert_eq!(
            compute_pct(Phase::RequestStarted, Duration::ZERO, e, 0.0),
            10
        );
        assert_eq!(compute_pct(Phase::Saved, Duration::ZERO, e, 0.0), 100);
    }

    #[test]
    fn generating_caps_at_90() {
        let e = Duration::from_secs(10);
        assert_eq!(compute_pct(Phase::Generating, Duration::ZERO, e, 0.0), 10);
        assert_eq!(
            compute_pct(Phase::Generating, Duration::from_secs(5), e, 0.0),
            50
        );
        // 超过 expected 也封顶 90
        assert_eq!(
            compute_pct(Phase::Generating, Duration::from_secs(100), e, 0.0),
            90
        );
    }

    #[test]
    fn download_segment_90_to_98() {
        assert_eq!(
            compute_pct(
                Phase::Downloading,
                Duration::ZERO,
                Duration::from_secs(60),
                0.0
            ),
            90
        );
        assert_eq!(
            compute_pct(
                Phase::Downloading,
                Duration::ZERO,
                Duration::from_secs(60),
                1.0
            ),
            98
        );
        // 越界夹紧
        assert_eq!(
            compute_pct(
                Phase::Downloading,
                Duration::ZERO,
                Duration::from_secs(60),
                5.0
            ),
            98
        );
    }

    #[test]
    fn expected_defaults_to_60s_without_history() {
        assert_eq!(expected_from_history(&[]), Duration::from_secs(60));
        assert_eq!(expected_from_history(&[0, 0]), Duration::from_secs(60));
    }

    #[test]
    fn expected_averages_recent_window() {
        // 均值 2000ms
        assert_eq!(
            expected_from_history(&[1000, 3000]),
            Duration::from_millis(2000)
        );
        // 只取近 20 次
        let mut v: Vec<i64> = (1..=30).map(|i| i * 1000).collect(); // 1000..30000
        v.reverse();
        // 近 20 个是 v[0..20] = 30000..11000，均值应 > 10000
        let d = expected_from_history(&v);
        assert!(d > Duration::from_secs(10));
    }
}
