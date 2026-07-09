# cargo-mutants 存活变异体处理（执行计划 §1.5）

`cargo mutants --file 'src/ids/**' --file 'src/engine/**'` 运行结果的处置。
原则：能补测试的补测试；纯编排/委托/事件发射类以书面豁免（下附理由）。

## 已补测试（catch，见对应 `#[cfg(test)]`）

| 位置 | 变异 | 新增测试 |
| --- | --- | --- |
| `classify.rs:35` | `suggests_disable_key -> true/false` | `only_auth_suggests_disabling_key` |
| `ids/mod.rs:61` | `peek_next -> Ok(0/1/-1)` | `peek_next_reflects_pool_without_consuming` |
| `engine/mod.rs:89` | `load_key_configs` 的 `!= → ==`（enabled 判定） | `load_key_configs_maps_enabled_and_skips_secretless` |
| `engine/mod.rs:80` | `load_key_configs → vec![]` | 同上（断言非空 + 跳过无密钥） |
| `engine/mod.rs:116` | `create_batch` 任务数 `+= → -=` | `create_batch_expands_task_count_correctly` |

## 书面豁免（exempt）

### `Engine` 门面薄委托（`engine/mod.rs`：pause/resume/is_paused/set_strategy/set_user_retry/kick）
这些是对 `Scheduler` 同名方法的**单行转发**。其真实行为由调度器层测试覆盖：
`dispatcher::tests::pause_stops_new_dispatch`（暂停/恢复）、`*_distributes_across_keys`（策略）、
`timeout_retries_once_then_fails`（user_retry）。为门面转发再建 mock 引擎收益极低，豁免。

### `dispatcher.rs` 调度编排 / 事件发射（28 项）
存活变异集中在：`emit_status`/`progress`/`batch_summary`/`key_health` 事件发射、进度 ticker、
Key 运行时冷却字段细节、worker 收尾顺序等。引擎的**可观测正确性不变量**已由集成测试严格约束：
`never_exceeds_per_key_concurrency`（并发上限）、`round_robin_distributes_across_keys`（分配）、
`timeout_retries_once_then_fails`（重试→终态 + attempts 计数）、`pause_stops_new_dispatch`、
`batch_archived_when_all_terminal`、`stress_500_tasks_all_reach_terminal_or_review`（500 压测：
全部达终态、结果图路径非空、事件非空）。这些断言的是 **DB 状态 + 并发峰值 + 终态收敛**，
而非「每一次事件是否发射」——后者是 UI 呈现细节，不改变业务真相，且真机冒烟（§M5）另有覆盖。
因此事件发射/内部 helper 返回值类的变异豁免，符合「业务真相在 Rust、UI 为镜像」的架构定位。

> 复跑：`cd src-tauri && cargo mutants --file 'src/ids/**' --file 'src/engine/**'`。
> v1.0.0 前应复跑一次，确认新增/改动代码未引入未覆盖的关键存活体。
