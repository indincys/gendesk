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
    /// 在跑、但一处计费证据都没有的条数（幽灵疑单，`repo::count_phantom_suspects`）。
    ///
    /// 它是唯一一类**阻在人身上却不在四个待办阶段里**的条目：躺在 `run`（按阶段说
    /// 「机器在跑，人插不上手」），可它恰恰是机器根本没在跑的那些，处置是免费重跑。
    /// 事故那次 18 条挂了十几个小时，而徽章全程是 0。
    pub phantom: i64,
    /// 侧栏徽章数：阻在**人**身上的四处 —— 待改写、待提交、待验收、失败，
    /// 外加藏在 `run` 里的幽灵疑单（见 [`Self::phantom`]）。
    ///
    /// **待改写在里面**（v0.22.0 改的）。旧口径把它排除在外，理由是「那一步在
    /// Claude Code 里做，催也没用」—— 但那恰恰说反了：它只可能由人推动，而 GenDesk
    /// 这边工单早已物化好、什么都不缺。排除它等于让全流水线最大的一处阻塞显示为 0，
    /// 实测 21 条待改写时徽章与「需要我」都是 0，而那 21 条谁也不会自己动。
    ///
    /// 仍**不含**已提交（机器在跑，人插不上手）与已定案的两态。
    ///
    /// 在 Rust 侧算好而不是让前端加：这条「什么算待办」的规则会随流水线演进，
    /// 留在前端就会与后端的判断悄悄分叉。前端的 `MINE` 与这里同义，两处一起改。
    pub actionable: i64,
    /// 验收通过了却没交付到输出目录的条数 —— 成片库那一页的徽章。
    ///
    /// 它是成片这条链上**唯一一处会无声断掉的地方**：验收时的拷贝失败不回滚验收
    /// （判定是人做的，文件可以补），于是「片子做出来了却没落地」是个完全合法、
    /// 界面上又完全看不见的状态。
    pub undelivered: i64,
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
        c.recount();
        c
    }

    /// 补上幽灵疑单数并重算徽章。阶段计数一条 SQL、幽灵一条 SQL，故分两步。
    pub fn with_phantom(mut self, n: i64) -> Self {
        self.phantom = n;
        self.recount();
        self
    }

    fn recount(&mut self) {
        self.actionable = self.rewrite + self.ready + self.rev + self.fail + self.phantom;
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
    /// 我们问到这个答案的时刻。前端据此显示「12 秒前」，从而能把「它在排队」
    /// 与「我们已经问不出话来了」区分开。
    pub polled_at: i64,
}

/// `v2v://activity` —— 执行日志新增一条。
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct V2vActivity {
    pub entry: super::activity::ActivityEntry,
}

/// `v2v://tick` —— 轮询器心跳（每轮一发，无论有没有变化）。
///
/// **心跳与日志是两件事**：日志只在有事发生时才该增长（否则 6 秒一条会把真正的错误
/// 冲出缓冲），而「轮询器还活着吗」恰恰要在**什么都没发生**时也能回答。
/// 一个没有心跳的静默界面，跟一个卡死的轮询器长得一模一样。
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct V2vTick {
    pub at: i64,
    /// 本轮开始时在跑的条数。
    pub running: i64,
    /// 轮询开关（设置里关掉时仍发心跳，否则界面分不清「关了」和「挂了」）。
    pub enabled: bool,
    pub finished: i64,
    pub failed: i64,
    /// 整轮失败的原因（读设置失败、CLI 不可用……）。
    pub error: Option<String>,
    /// 上一次**真的问过即梦**的时刻（`runner::last_sweep_at`）。
    ///
    /// 顶栏那个刷新按钮显示的「上次查询 N 前」读的是这个，**不是 [`Self::at`]**。
    /// 心跳 6 秒一次且纯内存读，拿它写「3 秒前」会让人以为数据是三秒前的新鲜货 ——
    /// 而真实查询是 5/10 分钟一次。这两个时刻差一个数量级，混用就是在骗人。
    pub last_sweep_at: Option<i64>,
}

/// `v2v://refresh` —— 人点了顶栏刷新按钮之后的逐条进度。
///
/// 手动刷新是 O(n) 个 `query_result` 进程（见 `runner::refresh_now`），几十条就要跑
/// 几十秒。没有这条事件的话，那段时间里界面与死机没有区别；有了它，按钮上就是
/// 「正在查 12/78」在走字，而行内的队列位次也随查随更新。
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct V2vRefresh {
    /// 还在跑吗。`false` 即这一轮已经结束（无论成功还是出错）。
    pub active: bool,
    pub done: i64,
    pub total: i64,
    /// 这一轮取回了几条成片。
    pub finished: i64,
    pub error: Option<String>,
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

    // 徽章催的是**阻在人身上**的四处：待改写 + 待提交 + 待验收 + 失败。
    //
    // 待改写在里面，这是 v0.22.0 改的。旧口径把它排除在外，理由是「那一步在
    // Claude Code 里做，催也没用」—— 而那恰恰说反了：它只可能由人推动。实测 21 条
    // 待改写时徽章是 0，而那 21 条谁也不会自己动。
    // 仍不含已提交（机器在跑）与已定案的两态。
    #[test]
    fn badge_counts_stages_blocked_on_the_human() {
        let c = StageCounts::from_rows(&[
            ("rewrite".to_string(), 10),
            ("ready".to_string(), 2),
            ("run".to_string(), 5),
            ("rev".to_string(), 3),
            ("pass".to_string(), 100),
            ("rej".to_string(), 4),
            ("fail".to_string(), 1),
        ]);
        assert_eq!(
            c.actionable, 16,
            "rewrite(10) + ready(2) + rev(3) + fail(1)；不含 run/pass/rej"
        );
    }

    // 幽灵疑单虽然躺在 run 里，却是阻在人身上的：机器根本没在跑它，处置是免费重跑。
    // 事故那次 18 条这样的单挂了十几个小时，而徽章全程是 0 —— 因为按阶段算，
    // 它们属于「机器在跑，人插不上手」的那一类。
    #[test]
    fn badge_counts_phantom_suspects_hiding_inside_run() {
        let c = StageCounts::from_rows(&[("run".to_string(), 19), ("rev".to_string(), 1)]);
        assert_eq!(c.actionable, 1, "没有幽灵时 run 一条都不算待办");
        let c = c.with_phantom(18);
        assert_eq!(c.actionable, 19, "rev(1) + 藏在 run 里的幽灵疑单(18)");
        assert_eq!(
            c.run, 19,
            "run 的口径不变 —— 幽灵是它的子集，不是另一个阶段"
        );
    }
}
