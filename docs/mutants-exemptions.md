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

---

## 发布模块（v0.7.0，planner + reconcile）

`cargo mutants -f 'src/publish/planner/set_picker.rs' -f 'src/publish/planner/scheduler.rs'
-f 'src/publish/planner/frequency.rs'` 运行结果的处置。reconcile.rs 的对账三分支/关单逻辑由
`publish::reconcile::e2e`（端到端集成 + 疑似负向断言）与 `classify_all_six`/`parse_time_formats`
覆盖；其 IO/事务编排类同 dispatcher 一样以行为级集成测试约束，不追单行内部变异。

首轮 117 mutants：44 caught / 41 unviable / 31 missed。scheduler.rs 补测后复跑
（46 mutants：27 caught / 2 unviable / 16 missed）。存活体两类，处置如下。

#### 已补测试（catch）：`scheduler.rs::available_points` **点数/空集** + `parse_slot` 零长边界
首轮 proptest `schedule_invariants` 只断言**安全不变量**（有时间则合法：落时段内 + 两两 ≥ gap），
故「把点集变空/上界失控」的变异不违反安全性（行退化为立即发即可），侥幸存活。补两测：
- `assigns_times_within_slots_when_capacity_ample`：容量充足时**全部行必须分到**时段内、
  两两 ≥ gap、点数 = 行数 —— 杀 `available_points -> vec![]`、`<=→>`（空集）等。
- `overflow_rows_get_none`：30 分钟时段按 60 分钟间隔**恰排 1 个**，其余留空 —— 约束点数上界，
  杀 `+= → *=`（步进失控）。
- `parse_slot("12:00-12:00") == None`：杀 `< → <=`（零长时段边界）。

#### 书面豁免（exempt）：确定性 hash/RNG 比特 + 抖动偏移 + 边界等价
剩余 16 存活体全部**行为等价于被测契约**，分三小类：
1. **哈希/伪随机内部比特**（`frequency.rs::id_hash`、`scheduler.rs::str_hash`、`Rng::next_u64`、
   `schedule` 内 `seed ^ str_hash` 的 `^↔|↔&`）：`^↔|↔&`、`>>↔<<`、`str_hash -> 0/1`、
   乘法/加法常量、splitmix64 常量等。
2. **抖动偏移内部**（`available_points` 106/107/108/115 的 `-↔+`、`>↔>=` 等）：这些只改变
   **时段内起始抖动量**，产出仍是「时段内、两两 ≥ gap、互异」的合法点集——而抖动本就是随机的，
   测试断言的是**契约**（合法+间隔+互异）而非具体偏移值，故任意偏移均满足。
3. **边界等价**（`schedule:160` 的 `> → >=`）：`if rows.len() > limit` 与 `>= limit` 仅在
   `len == limit` 处不同，而此时 `rows.len() - limit == 0`、`truncate(limit)` 为空操作，
   两分支产出与裁剪计数完全一致——真等价变异。

**不被杀死是设计使然**：
排期/频率的正确性约束的是**行为**而非**具体比特**——
- `frequency::tests`：热款每日 7/7、温款每周恰 N 天、冷款 10 周轮播覆盖全池、每周至多 1 天；
- `scheduler`：账号不超日限、同平台两两 ≥ min_gap、时间落时段内、行数 = 展开 − 裁剪、上述容量断言；
- `set_picker`：视频无正文/图集需正文/用尽过滤/平台匹配优先/最少使用优先/**同 seed 可复现**。

只要哈希/RNG **确定性**且**分布合理**，上述不变量对任意具体位运算都成立（不同哈希仍给出
「N 个不同周内日」「可复现选取」「不同平台不同抖动」）。故哈希/RNG 内部比特变异属
**行为等价变异**，书面豁免；凡改变**可观测行为**者（频率计数/约束过滤/选取优先级/时段分配）
均已被上述测试杀死。

> 复跑：`cd src-tauri && cargo mutants -f 'src/publish/planner/*.rs' --timeout 90`。
> tag v0.7.0 前（用户触发 /code-review ultra 阶段）应复跑确认无新的行为相关存活体。
