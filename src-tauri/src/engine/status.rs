//! 任务状态机（执行计划 2.1 / 需求 10.3）。
//!
//! 八态：待生成 q → 生成中 run →（重试中 retry → run）* → 成功=待验收 rev
//! → 已通过 pass / 未通过 rej；失败 fail 为可手动重试的终态。
//! 合法迁移表为纯函数并配 proptest（任意迁移序列不产生非法状态）。

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    /// 待生成
    Q,
    /// 生成中
    Run,
    /// 重试中（退避/排队等待再次生成）
    Retry,
    /// 成功（即待验收）
    Rev,
    /// 已通过
    Pass,
    /// 未通过
    Rej,
    /// 失败（可手动重试的终态）
    Fail,
}

impl TaskStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskStatus::Q => "q",
            TaskStatus::Run => "run",
            TaskStatus::Retry => "retry",
            TaskStatus::Rev => "rev",
            TaskStatus::Pass => "pass",
            TaskStatus::Rej => "rej",
            TaskStatus::Fail => "fail",
        }
    }

    /// 是否为「批次归档」意义上的终态（pass/rej/fail）。rev 需人工验收，不算。
    pub fn is_terminal_for_archive(&self) -> bool {
        matches!(self, TaskStatus::Pass | TaskStatus::Rej | TaskStatus::Fail)
    }

    /// 是否可手动重试（fail 终态）。
    pub fn can_manual_retry(&self) -> bool {
        matches!(self, TaskStatus::Fail)
    }

    /// 是否处于调度活跃态（占用/将占用 Key 并发）。
    pub fn is_active(&self) -> bool {
        matches!(self, TaskStatus::Run | TaskStatus::Retry)
    }
}

/// 非法迁移错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("非法状态迁移：{from:?} → {to:?}")]
pub struct IllegalTransition {
    pub from: TaskStatus,
    pub to: TaskStatus,
}

/// 合法迁移表（纯函数）。
pub fn can_transition(from: TaskStatus, to: TaskStatus) -> bool {
    use TaskStatus::*;
    matches!(
        (from, to),
        (Q, Run)            // 派发
            | (Run, Rev)    // 生成成功 → 待验收
            | (Run, Retry)  // 可重试失败 → 进入重试
            | (Run, Fail)   // 不可重试 / 重试耗尽 → 失败
            | (Retry, Run)  // 冷却结束再次派发
            | (Retry, Fail) // 重试放弃
            | (Rev, Pass)   // 验收通过
            | (Rev, Rej)    // 验收不通过
            | (Fail, Q) // 手动重试 / 中断恢复：重新入队
    )
}

/// 校验并返回目标状态；非法迁移返回错误（供调度器落库前守卫）。
pub fn transition(from: TaskStatus, to: TaskStatus) -> Result<TaskStatus, IllegalTransition> {
    if can_transition(from, to) {
        Ok(to)
    } else {
        Err(IllegalTransition { from, to })
    }
}

impl fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for TaskStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "q" => TaskStatus::Q,
            "run" => TaskStatus::Run,
            "retry" => TaskStatus::Retry,
            "rev" => TaskStatus::Rev,
            "pass" => TaskStatus::Pass,
            "rej" => TaskStatus::Rej,
            "fail" => TaskStatus::Fail,
            other => return Err(format!("未知任务状态：{other}")),
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即失败
mod tests {
    use super::TaskStatus::*;
    use super::*;
    use proptest::prelude::*;

    const ALL: [TaskStatus; 7] = [Q, Run, Retry, Rev, Pass, Rej, Fail];

    #[test]
    fn legal_transitions_exact_set() {
        let legal: Vec<(TaskStatus, TaskStatus)> = vec![
            (Q, Run),
            (Run, Rev),
            (Run, Retry),
            (Run, Fail),
            (Retry, Run),
            (Retry, Fail),
            (Rev, Pass),
            (Rev, Rej),
            (Fail, Q),
        ];
        for &from in &ALL {
            for &to in &ALL {
                let expected = legal.contains(&(from, to));
                assert_eq!(can_transition(from, to), expected, "{from:?}→{to:?}");
            }
        }
    }

    #[test]
    fn terminal_states_have_no_outgoing_except_fail_requeue() {
        // pass / rej 完全终态；fail 仅可回到 q
        assert!(ALL.iter().all(|&to| !can_transition(Pass, to)));
        assert!(ALL.iter().all(|&to| !can_transition(Rej, to)));
        assert!(ALL.iter().all(|&to| can_transition(Fail, to) == (to == Q)));
    }

    #[test]
    fn roundtrip_str() {
        for &s in &ALL {
            assert_eq!(s.as_str().parse::<TaskStatus>().unwrap(), s);
        }
    }

    fn status() -> impl Strategy<Value = TaskStatus> {
        prop_oneof![
            Just(Q),
            Just(Run),
            Just(Retry),
            Just(Rev),
            Just(Pass),
            Just(Rej),
            Just(Fail)
        ]
    }

    proptest! {
        // 属性：从 q 出发，任意「只走合法迁移」的序列，永不到达非法组合，
        // 且一旦进入 pass/rej 便不再有任何后继。
        #[test]
        fn walk_never_produces_illegal(seq in proptest::collection::vec(status(), 0..50)) {
            let mut cur = Q;
            for next in seq {
                if can_transition(cur, next) {
                    // pass/rej 不应有合法后继
                    prop_assert!(cur != Pass && cur != Rej, "终态 {:?} 不应有后继", cur);
                    cur = transition(cur, next).unwrap();
                }
            }
        }
    }
}
