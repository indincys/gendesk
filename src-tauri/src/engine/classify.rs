//! 错误分类与重试策略（执行计划 2.3 / 技术文档 4.2）。

use std::time::Duration;

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::provider::ProviderErrorKind;

/// 六类错误（落 `tasks.error_type` / `task_attempts.error_type`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub enum ErrorType {
    Timeout,
    RateLimited,
    ContentPolicy,
    Auth,
    Interrupted,
    Other,
}

impl ErrorType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ErrorType::Timeout => "Timeout",
            ErrorType::RateLimited => "RateLimited",
            ErrorType::ContentPolicy => "ContentPolicy",
            ErrorType::Auth => "Auth",
            ErrorType::Interrupted => "Interrupted",
            ErrorType::Other => "Other",
        }
    }

    /// Auth 建议前台停用该 Key。
    pub fn suggests_disable_key(&self) -> bool {
        matches!(self, ErrorType::Auth)
    }
}

/// Provider 错误 → 六类。
pub fn classify(kind: ProviderErrorKind) -> ErrorType {
    match kind {
        ProviderErrorKind::Timeout => ErrorType::Timeout,
        ProviderErrorKind::RateLimited => ErrorType::RateLimited,
        ProviderErrorKind::ContentPolicy => ErrorType::ContentPolicy,
        ProviderErrorKind::Auth => ErrorType::Auth,
        ProviderErrorKind::Network | ProviderErrorKind::BadResponse | ProviderErrorKind::Other => {
            ErrorType::Other
        }
    }
}

/// 重试处置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryDecision {
    /// 是否重试（false = 终态 fail）
    pub retry: bool,
    /// 重试时是否换 Key
    pub switch_key: bool,
    /// 重试前冷却（RateLimited 退避）
    pub cooldown: Duration,
}

impl RetryDecision {
    fn terminal() -> Self {
        Self {
            retry: false,
            switch_key: false,
            cooldown: Duration::ZERO,
        }
    }
    fn retry(switch_key: bool, cooldown: Duration) -> Self {
        Self {
            retry: true,
            switch_key,
            cooldown,
        }
    }
}

/// 依据错误类型、已用重试次数、用户配置重试数，决定处置（技术文档 4.2）。
///
/// `retries_used` 为该任务已发生的重试次数（`tasks.retry_count`）。
pub fn decide(err: ErrorType, retries_used: u32, user_retry_count: u32) -> RetryDecision {
    match err {
        // 超时：重试 1 次，换 Key
        ErrorType::Timeout => {
            if retries_used < 1 {
                RetryDecision::retry(true, Duration::ZERO)
            } else {
                RetryDecision::terminal()
            }
        }
        // 限流：重试 1 次，换 Key，退避
        ErrorType::RateLimited => {
            if retries_used < 1 {
                RetryDecision::retry(true, Duration::from_secs(18))
            } else {
                RetryDecision::terminal()
            }
        }
        // 违规：重试 1 次（同 Key/同提示词），仍失败则终态并标注
        ErrorType::ContentPolicy => {
            if retries_used < 1 {
                RetryDecision::retry(false, Duration::ZERO)
            } else {
                RetryDecision::terminal()
            }
        }
        // 鉴权失败：不重试，建议停用该 Key
        ErrorType::Auth => RetryDecision::terminal(),
        // 中断：不自动重试（用户一键重试）
        ErrorType::Interrupted => RetryDecision::terminal(),
        // 其它：按用户设置的重试次数，换 Key
        ErrorType::Other => {
            if retries_used < user_retry_count {
                // 5xx / 网络断开 / 上游暂无渠道都归在 Other。立即重试会在 250 并发下
                // 把同一批失败原样再打一遍；留一个短退避，让 Key 级失败波次先收敛。
                let secs = 5u64.saturating_mul(1u64 << retries_used.min(4));
                RetryDecision::retry(true, Duration::from_secs(secs))
            } else {
                RetryDecision::terminal()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_maps_kinds() {
        assert_eq!(classify(ProviderErrorKind::Timeout), ErrorType::Timeout);
        assert_eq!(
            classify(ProviderErrorKind::RateLimited),
            ErrorType::RateLimited
        );
        assert_eq!(
            classify(ProviderErrorKind::ContentPolicy),
            ErrorType::ContentPolicy
        );
        assert_eq!(classify(ProviderErrorKind::Auth), ErrorType::Auth);
        assert_eq!(classify(ProviderErrorKind::Network), ErrorType::Other);
        assert_eq!(classify(ProviderErrorKind::BadResponse), ErrorType::Other);
    }

    #[test]
    fn timeout_retries_once_then_terminal() {
        assert!(decide(ErrorType::Timeout, 0, 3).retry);
        assert!(decide(ErrorType::Timeout, 0, 3).switch_key);
        assert!(!decide(ErrorType::Timeout, 1, 3).retry);
    }

    #[test]
    fn ratelimited_has_cooldown_and_switch() {
        let d = decide(ErrorType::RateLimited, 0, 3);
        assert!(d.retry && d.switch_key && d.cooldown > Duration::ZERO);
        assert!(!decide(ErrorType::RateLimited, 1, 3).retry);
    }

    #[test]
    fn content_policy_retries_same_key_once() {
        let d = decide(ErrorType::ContentPolicy, 0, 3);
        assert!(d.retry && !d.switch_key);
        assert!(!decide(ErrorType::ContentPolicy, 1, 3).retry);
    }

    #[test]
    fn auth_and_interrupted_never_auto_retry() {
        assert!(!decide(ErrorType::Auth, 0, 3).retry);
        assert!(!decide(ErrorType::Interrupted, 0, 3).retry);
    }

    #[test]
    fn only_auth_suggests_disabling_key() {
        assert!(ErrorType::Auth.suggests_disable_key());
        for e in [
            ErrorType::Timeout,
            ErrorType::RateLimited,
            ErrorType::ContentPolicy,
            ErrorType::Interrupted,
            ErrorType::Other,
        ] {
            assert!(!e.suggests_disable_key(), "{e:?} 不应建议停用 Key");
        }
    }

    #[test]
    fn error_type_str_roundtrip() {
        assert_eq!(ErrorType::Timeout.as_str(), "Timeout");
        assert_eq!(ErrorType::Auth.as_str(), "Auth");
        assert_eq!(ErrorType::Interrupted.as_str(), "Interrupted");
    }

    #[test]
    fn other_follows_user_retry_count() {
        let first = decide(ErrorType::Other, 0, 2);
        let second = decide(ErrorType::Other, 1, 2);
        assert!(first.retry && first.cooldown == Duration::from_secs(5));
        assert!(second.retry && second.cooldown == Duration::from_secs(10));
        assert!(!decide(ErrorType::Other, 2, 2).retry);
        // 用户设 0 次 → 不重试
        assert!(!decide(ErrorType::Other, 0, 0).retry);
    }
}
