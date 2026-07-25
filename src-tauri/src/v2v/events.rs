//! 视频流水线事件（事件驱动刷新，不轮询 —— 架构铁律 4）。

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri_specta::Event;

/// 七态计数。看板列头、侧栏徽章、开屏提示都读它。
#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct StageCounts {
    pub rewrite: i64,
    pub ready: i64,
    pub run: i64,
    pub rev: i64,
    pub pass: i64,
    pub rej: i64,
    pub fail: i64,
}

impl StageCounts {
    /// 从 `(stage, count)` 列表折叠。未知 stage 忽略（CHECK 约束已挡在库层）。
    pub fn from_rows(rows: &[(String, i64)]) -> Self {
        let mut c = Self::default();
        for (stage, n) in rows {
            match stage.as_str() {
                "rewrite" => c.rewrite = *n,
                "ready" => c.ready = *n,
                "run" => c.run = *n,
                "rev" => c.rev = *n,
                "pass" => c.pass = *n,
                "rej" => c.rej = *n,
                "fail" => c.fail = *n,
                _ => {}
            }
        }
        c
    }

    /// 侧栏徽章数：需要人动手的两处 —— 待提交与待验收。
    ///
    /// 刻意**不含**待改写：那一步在 Claude Code 里做，催也没用；
    /// 也不含已提交：那是机器在跑，人插不上手。徽章只该催人能立刻处理的事。
    pub fn actionable(&self) -> i64 {
        self.ready + self.rev
    }
}

/// `v2v://changed` —— 流水线任何阶段变动即推送。
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct V2vChanged {
    pub counts: StageCounts,
    /// 本次变动涉及的 clip（前端可据此做局部刷新）；批量操作时可能为空。
    pub clip_id: Option<i64>,
}

/// `v2v://progress` —— 已提交条目的轮询进度（队列位次 / 状态原文）。
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct V2vProgress {
    pub clip_id: i64,
    /// 即梦返回的 `gen_status` 原文，直接显示给人看。
    ///
    /// 不映射成自造的中文态：轮询状态是**别人系统的**真相，翻译一层只会在它加了新态时
    /// 显示成「未知」。原文加一行 hint 比一个翻译错的标签有用。
    pub gen_status: String,
    pub queue_idx: Option<i64>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // 测试断言失败即失败
mod tests {
    use super::*;

    #[test]
    fn counts_fold_from_rows() {
        let rows = vec![
            ("rewrite".to_string(), 3),
            ("ready".to_string(), 2),
            ("rev".to_string(), 1),
            ("unknown_future_stage".to_string(), 99),
        ];
        let c = StageCounts::from_rows(&rows);
        assert_eq!(c.rewrite, 3);
        assert_eq!(c.ready, 2);
        assert_eq!(c.rev, 1);
        assert_eq!(c.run, 0);
    }

    // 徽章只催人能立刻处理的事：待提交 + 待验收。
    // 待改写要去 Claude Code 做、已提交是机器在跑，催了也没用。
    #[test]
    fn badge_counts_only_actionable_stages() {
        let c = StageCounts {
            rewrite: 10,
            ready: 2,
            run: 5,
            rev: 3,
            pass: 100,
            rej: 4,
            fail: 1,
        };
        assert_eq!(c.actionable(), 5, "只该是 ready(2) + rev(3)");
    }
}
