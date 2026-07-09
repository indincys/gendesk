# CLAUDE.md — GenDesk 开发约定

> 本文件是 AI 开发本仓库的操作手册。**每条铁律都有 guardrails/CI 对应检查**；
> 改规则必须同步改检查（否则等于没规则）。权威文档：
> `内部图片生产工具_V1需求文档.md`（功能）· `..._技术选型定稿.md`（技术）·
> `..._开发执行计划.md`（任务拆解 + DoD）· 原型 `docs/prototype/prototype.dc.html`（像素基准）·
> `docs/prototype/Design-Tokens.md`（token 基准）。

## 常用命令

| 目的                   | 命令                                                          |
| -------------------- | ----------------------------------------------------------- |
| 启动双端开发               | `pnpm tauri dev`                                            |
| **全门禁（CI 镜像，提交前必跑）** | `pnpm check`                                                |
| 前端类型检查               | `pnpm typecheck`                                            |
| 前端单测                 | `pnpm test`                                                 |
| 前端 lint/format 修复    | `pnpm lint:fix`                                             |
| 架构铁律检查               | `pnpm guardrails`                                           |
| Rust 测试              | `cd src-tauri && cargo test`                                |
| Rust 静态检查            | `cd src-tauri && cargo clippy --all-targets -- -D warnings` |
| **重新生成 IPC 绑定**      | `cd src-tauri && cargo test --lib export_bindings`          |
| 压测（M2 起）             | `cd src-tauri && cargo test --release -- --ignored`         |
| 变异测试（里程碑关卡）          | `cd src-tauri && cargo mutants -- --package gendesk`        |
| 安装 pre-commit        | `pnpm dlx lefthook install`                                 |

## 架构铁律（均有机器检查）

1. **业务真相只在 Rust**：任务状态、编号发放、文件操作、DB 读写全部经 Rust 命令；
   前端不持有可变业务状态，Zustand 只做事件镜像与 UI 态。
2. **单写者事务**：所有状态迁移由调度器串行提交；配合 single-instance 禁双开。
3. **前端只经&#x20;********`src/lib/ipc/`********&#x20;出入** → guardrails 检查 `invoke(`/`listen(` 仅限该目录。
4. **事件驱动不轮询**：进度/汇总/健康经 Tauri 事件推送，250ms 节流；导航徽章由事件驱动。
5. **token 只从&#x20;********`src/styles/globals.css`********&#x20;取** → guardrails 检查 `oklch(` 硬编码仅限该文件。
6. **视觉以原型 HTML 源码为准**（读源码，不截图猜测）。

## IPC 约定

- `src/lib/ipc/bindings.ts` 由 **tauri-specta 自动生成，禁手改**（guardrails 校验生成头；
  CI `git diff --exit-code` 校验已同步）。
- 新增/改动命令或事件后：在 `src-tauri/src/lib.rs` 的 `specta_builder()` 登记，然后
  `cargo test --lib export_bindings` 重新生成并提交 `bindings.ts`。
- 前端只 import `@/lib/ipc`（`index.ts` 薄封装），不直接 import `bindings.ts` 以外的 Tauri API 做业务调用。
- 载荷字段 **camelCase** 由 specta 序列化配置统一保证，不手写 TS 类型。

## 错误处理规范

- 统一 `thiserror` 错误类型（`src-tauri/src/error.rs`）经 IPC 序列化给前端。
- **非测试代码禁&#x20;********`unwrap`********/********`expect`********/\*\*\*\*****`panic`** → Cargo `[lints.clippy]` 强制（deny）。
  测试内允许，但须以带说明注释的 `#[allow(...)]` 局部放开（guardrails 校验说明）。
- 前端未捕获错误经 `reportFrontendError` → `log_frontend_error` 命令汇入统一 tracing 日志。
- 业务错误六类：Timeout / RateLimited / ContentPolicy / Auth / Interrupted / Other（技术文档 4.2）。

## 视觉规范

- 颜色/圆角/间距/字号/阴影/动效时长全部为 `globals.css` 的 CSS 变量（oklch 原值仅此一处）。
- **8 态 → 5 视觉组映射**：待生成 `q`=灰；生成中 `run`+重试中 `retry`=蓝(spinner)；
  失败 `fail`=红；成功 `rev`(待验收)=琥珀；已通过 `pass`=绿 / 未通过 `rej`=灰。
- 动效尊重 `prefers-reduced-motion`，另有设置页「标准/减弱」开关。

## 领域词汇表

- **批次(batch)**：一次「开始生成」创建的任务集合；全部达终态后自动 `archived`。
- **挂靠**：生成页每张参考图指定一个提示词组；任务数 = Σ(参考图 × 其挂靠组提示词数)（非笛卡尔积）。
- **临时分组(is\_temp)**：生成页导入 txt 产生；该组任一提示词首次验收通过 → 整组转正式。
- **号池**：编号 `前缀-0001` 递增发放，回收优先；发放/回收与业务写同事务。输出文件名去连字符。
- **废纸篓**：未通过/删除内容暂存（留缩略图+提示词记录，删原图）；清理=物理删+级联删+编号回收，不可恢复。
- **伪进度**：生图 API 无真实进度；排队 0→请求 10%→elapsed/expected 线性至 90%→下载 90-98%→落盘 100%。

## 审查协议（§1.4）

- **实现与审查分离**：任务实现完成后，由**全新上下文**会话执行 `/code-review`
  （默认 medium；引擎/数据层用 high）。同一上下文自审会继承同样盲点。发现项修复后才 merge。
- **测试完整性规则**：**禁止**为让 CI 变绿而修改/删除/放宽既有测试断言；确需改动须在 PR 描述单独说明理由，审查重点核对。
- **里程碑深审**：M2（引擎）、M4（发布链）、v1.0.0 前，由用户触发 `/code-review ultra` 云端多智能体深审。
- 小步提交 + 分支 PR 流：一个任务 = 一个分支 = 实现 + 测试 + 门禁全绿 + 审查通过 = 一次 merge。
  main 受保护（check.yml 全绿才可合并），即使单人开发也走 PR。

## 提交前门禁清单（`pnpm check` 覆盖）

guardrails · Biome ci · tsc strict · vitest · vite build · cargo fmt · clippy -D warnings ·
cargo test（含 bindings 同步）· cargo check。

**数据层实现说明**：sqlx 采用运行时校验查询（`query`/`query_as`），SQL 由针对临时库的
`cargo test` 集成测试覆盖（比仅编译期检查更强）；故未接入 `cargo sqlx prepare --check`。
`cargo llvm-cov`（engine/ids/importer ≥ 85%）待 M2 引擎落地后统一接入 CI。

## 里程碑进度

- [x] **M0 骨架与门禁** — Tauri2+React19 骨架、设计 tokens、窗口壳、命令面板、质量门禁全套。
- [x] **M1 数据层与基础域** — migration 0001 全 schema、号池(proptest)、files(缩略图/命名/废纸篓)、importer(GBK/两段式)、settings/api_keys(keyring)/refs/prompts 域命令 + 前端 settings store。
- [x] **M2 任务引擎** — 状态机(proptest)、Provider(OpenAI 兼容, wiremock 7 用例)、错误分类六类、
  调度器(per-Key Semaphore + 两策略 + 指数退避)、伪进度、中断恢复、1→500 压测；batches/tasks
  域命令 + 4 事件(status/progress/summary/keyHealth)。**待做**：M2 出口 cargo-mutants + `/code-review ultra`（用户触发）。
- [ ] M3 业务页面 · [ ] M4 更新发布链 · [ ] M5 收尾质量关。

