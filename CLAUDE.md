# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## GenDesk — 内部图片生产工具 · 开发约定

> 本地批量图生图流水线：参考图素材库 → 提示词分组 → 批量生成 → 进度追踪 → 人工验收 →
> 合格图输出归档；v0.7.0 起并入发布与资产管理模块。
>
> 本文件是 AI 开发本仓库的操作手册。**每条铁律都有 guardrails/CI 对应检查**；
> 改规则必须同步改检查（否则等于没规则）。
>
> **V1 的四份规划文档（需求/技术选型/开发执行计划/UX 优化计划）已于 445b396 删除**——
> 内容浓缩进本文末尾的「里程碑进度」，那里就是它们的现存形态，**不要去找那些文件**。
> 现存参考物：`docs/prototype/prototype.dc.html`（V1 八页像素基准）·
> `docs/prototype/publish.dc.html`（发布模块原型）· `docs/prototype/Design-Tokens.md`（token 基准）·
> 发布模块三份（`内部图片生产工具_发布与资产管理需求文档.md` / `..._发布模块开发执行计划.md` /
> `..._发布模块优化执行计划.md`）· `内部图片生产工具_密钥存储迁移执行计划.md` ·
> `内部图片生产工具_验收与体验修复执行计划.md` · `发布模块_设计输入摘要.md` ·
> `docs/V2-backlog.md`（含**交付前人工收尾清单**）· `docs/mutants-exemptions.md` ·
> `docs/收件箱收录格式规范.md`。

## 常用命令

前置：Node ≥ 22 · pnpm 9.15（`packageManager` 已锁）· Rust stable。
仅支持 Windows 10/11 x64 与 macOS 12+ Apple Silicon（**Intel Mac 不支持**）。

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

**本机 shell 陷阱**：非交互 shell 里 `cd <repo> && <node/pnpm 命令>` 会被 fnm 的 use-on-cd
钩子打断（`We can't find the necessary environment variables to replace the Node version`），
**报错来自 cd 而非命令本身**。用 `--dir` 绕开，别去改 shell 配置：

```bash
pnpm --dir /Users/indincys/Documents/Code/GenDesk check
```

同理 Rust 侧用 `cargo <cmd> --manifest-path <repo>/src-tauri/Cargo.toml`。
（表格里的 `cd src-tauri && ...` 在交互终端里正常，仅 AI 的非交互 shell 需绕行。）

**跑单个测试**（迭代时别整套跑，Rust 全量 ~7s、`pnpm check` ~1min）：

```bash
cd src-tauri && cargo test --lib refs_insert_list_setgroup -- --exact --nocapture
```

- 按模块过滤：`cargo test --lib publish::planner::`（子串匹配，无需 `--exact`）。
- 前端单测：`pnpm vitest run src/routes.test.ts` 或 `pnpm test:watch`。
- 前端测试极少（仅 `routes.test.ts` / `stores/settings.test.ts`）——**业务真相在 Rust，
  测试也在 Rust**，别为了「补前端覆盖率」去测 UI 壳。

## 架构地图

**形态**：Tauri 2 桌面应用，单实例、纯本地（SQLite + 本地文件），无服务端。
前端 React 19 + Zustand + Vite（`src/`），后端 Rust（`src-tauri/src/`），
二者只经 tauri-specta 生成的类型化 IPC 通信。

### 后端（`src-tauri/src/`）

| 模块           | 职责                                                                     |
| ------------ | ---------------------------------------------------------------------- |
| `lib.rs`     | **命令/事件的唯一登记点**（`specta_builder()`）+ 应用启动装配。加命令必改这里。                    |
| `state.rs`   | `AppState`：DB 池 · 密钥存储 · 数据目录 · 引擎。业务真相的持有者。                            |
| `commands/`  | IPC 命令层（薄）：校验入参 → 调 repo/引擎 → 组视图结构体。按域分文件。                             |
| `db/`        | `migrations/` forward-only + `repo/` 薄 SQL 封装。**业务规则不在 repo**。          |
| `engine/`    | 任务引擎：`dispatcher`（单循环 + per-Key Semaphore）· `status`（7 态机）· `strategy` · `classify`（错误六类）· `progress`（伪进度）· `recovery`（中断恢复）· `events`（EventSink 抽象，故引擎可脱离 Tauri 测试）。 |
| `provider/`  | 生图 Provider 抽象；V1 唯一实现 `openai`（`POST {base}/images/edits`）+ `sanitize`（元数据/C2PA 剥离）。 |
| `publish/`   | 发布与资产管理（最大子系统）：`paths`(RelPath) · `platform`(五平台单点) · `inbox/` · `planner/` · `xlsx/` · `exporter` · `reconcile` · `ticker`。 |
| `v2v/`       | 图生视频流水线（v0.15.0 起状态在库内）：`dreamina`(即梦 CLI 封装) · `handoff`(交接目录工单往返) · `runner`(提交/轮询/落盘) · `watcher`(监听改写结果) · `events`。另留 v0.13.0 的一次性导出包（`common_affixes`/`write_pack`）。 |
| `purpose.rs` | 用途（管线）受控取值单点，同 `publish/platform.rs` 的模式。                               |
| `secrets.rs` | API Key 本地加密文件存储（XChaCha20-Poly1305）+ 一次性 keyring 迁移。                   |
| `files/`     | 数据目录 · 缩略图 · 命名 · 废纸篓文件搬运。`ids/` 号池，`importer/` txt 解析。                  |

### 前端（`src/`）

`features/<页面>/` 一页一文件（普遍 600–1900 行，就地展开而非过度拆组件）·
`routes.tsx` 是**路由/侧栏/⌘K/快捷键的单一来源** · `stores/` Zustand（engine 镜像事件、
generate 选择态、ui、settings、publish）· `lib/ipc/` 唯一 IPC 出入口 ·
`styles/globals.css` 唯一 token 来源。

### 一条请求的完整链路（改生成相关代码前先读懂这条）

```
GeneratePage 选组/挂靠 → commands::batches::create_batch
  → engine::create_batch（同事务：展开组合 × 抽卡 → tasks + batch_refs + 归档本批组与图）
  → Scheduler 循环取 q 态 → per-Key Semaphore 限并发 → provider.generate(multipart)
  → 落盘 outputs/ + 缩略图 → 状态迁移 rev（待验收）
  → EventSink 推 task://status-changed · task://progress · batch://summary（250ms 节流）
  → stores/engine 镜像 → 任务页/导航徽章
```

**进 multipart 的生成参数只有三个**（`provider::GenParams`，v0.15.2）：`aspect_ratio` /
`size` / `output_format`，外加恒定的 `n=1`。**画幅走 `aspect_ratio` 而非 `size`**；
端点文档里的 quality / response_format / background / output_compression / extra_fields
**一律不做**（用户明确不需要——参数摆在界面上却没人用，只会让「到底哪个在起作用」更难回答）。
批次的 `params_json` 比它宽（还有 `draws`/本地去水印档位 `watermark`/输出处理开关等纯 UI 键，
供「再来一批」还原），Rust 侧静默忽略。**故意不加 `deny_unknown_fields`**：严格解析会让整份
快照退化成「全部空」，用户选的比例反而一个字段都发不出去。入口侧另有 `parse_checked`
（严格 + 预检），见下文 v0.15.2。

## 架构铁律（均有机器检查）

1. **业务真相只在 Rust**：任务状态、编号发放、文件操作、DB 读写全部经 Rust 命令；
   前端不持有可变业务状态，Zustand 只做事件镜像与 UI 态。
2. **单写者事务**：所有状态迁移由调度器串行提交；配合 single-instance 禁双开。
3. **前端只经 `src/lib/ipc/` 出入** → guardrails 检查 `invoke(`/`listen(` 仅限该目录。
4. **事件驱动不轮询**：进度/汇总/健康经 Tauri 事件推送，250ms 节流；导航徽章由事件驱动。
5. **token 只从 `src/styles/globals.css` 取** → guardrails 检查 `oklch(` 硬编码仅限该文件。
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
- **非测试代码禁 `unwrap` / `expect` / `panic`** → Cargo `[lints.clippy]` 强制（deny）。
  测试内允许，但须以带说明注释的 `#[allow(...)]` 局部放开（guardrails 校验说明）。
- 前端未捕获错误经 `reportFrontendError` → `log_frontend_error` 命令汇入统一 tracing 日志。
- 业务错误六类：Timeout / RateLimited / ContentPolicy / Auth / Interrupted / Other
  （分类器 `engine/classify.rs`；Interrupted 为引擎内部态，不来自 provider）。

## 视觉规范

- 颜色/圆角/间距/字号/阴影/动效时长全部为 `globals.css` 的 CSS 变量（oklch 原值仅此一处）。
- **7 态 → 5 视觉组映射**：待生成 `q`=灰；生成中 `run`+重试中 `retry`=蓝(spinner)；
  失败 `fail`=红；成功 `rev`(待验收)=琥珀；已通过 `pass`=绿 / 未通过 `rej`=灰。
  （`TaskStatus` 与 0001 的 CHECK 都是 7 个：q/run/retry/rev/pass/rej/fail。早期文档与
  0001 注释里的「八态」是沿用初版规划的笔误，以枚举为准。）
- 动效尊重 `prefers-reduced-motion`，另有设置页「标准/减弱」开关。

## 领域词汇表

- **批次(batch)**：一次「开始生成」创建的任务集合；全部达终态后自动 `archived`。
- **挂靠**：生成页每张参考图指定一个提示词组；任务数 = Σ(参考图 × 其挂靠组提示词数)（非笛卡尔积）。
- **临时分组(is\_temp)**：生成页导入 txt 产生；该组任一提示词首次验收通过 → 整组转正式。
- **归档(archived\_at, 0016)**：批次开跑后自动给本批的提示词组与参考图打戳。**只**决定
  生成页选择器是否默认列出，库里仍在、可查、可一键取消。与「删除」无关。
- **图库分组(ref\_groups, 0019)**：参考图库自己的目录，**与提示词组无关**。历史列
  `ref_images.group_id`（指向 prompt\_groups）已废弃，不读不写。
- **临时上传(ephemeral, 0019)**：生成页上传的参考图，只作本批附件。仍是 ref\_images 行
  （tasks/batch\_refs/accepted\_works 以它为父表），但图库页、「从参考图库选择」、
  去重基准三处都不含它。
- **用途标签(purpose)**：标在**提示词组**上（不在图上、不在批次上——批次会混组）；
  受控取值单点 `purpose.rs`，当前只有「图生视频」。是筛选默认值，**不是门禁**。
  v0.15.0 起在**导入预览**里就能选（关键词预猜 B-Roll/分镜/首帧，标琥珀「疑似」）。
- **号池**：编号 `前缀-0001` 递增发放，回收优先；发放/回收与业务写同事务。输出文件名去连字符。
- **废纸篓**：未通过/删除内容暂存（留缩略图+提示词记录，删原图）；清理=物理删+级联删+编号回收，不可恢复。
- **伪进度**：生图 API 无真实进度；排队 0→请求 10%→elapsed/expected 线性至 90%→下载 90-98%→落盘 100%。

### 发布模块词汇（v0.7.0）

- **SKU**：款式一级分类，下挂素材/标题/正文三池；内置「通用」分组收纳无 SKU 文本。
- **素材包(asset_pack)**：一次可发布素材单元；视频型=1 视频(+封面)，图集型=N 图(+封面)。
  存储态 new|active|retired，**入库即 active**（文件齐备即可发；new 留给未来需人工过目的来源，
  UI 上一键转 active）；「已用尽/冷却中/回可用」为台账 + 查重窗口派生态（不落库）。
- **日内容套装(daily_set)**：某天某 SKU 选定的（素材包+标题[+正文]）；当天全平台全账号统一。
- **任务单(task_sheet)**：某天全部发布任务集合，一天一张；草稿→已确认→已导出→回收中→已关闭。
- **任务包**：任务单.xlsx(22 列) + 素材(按 SKU 一份) + 执行说明.md + 回执截图/ + READY.txt(最后写)。
- **回执**：执行器回写的任务状态 + RPA 信息(链接｜原因｜时间) + 截图；只写 xlsx 第 20–22 列。
- **使用台账(usage_ledger)**：套装粒度发布记录，驱动查重窗口 + 素材生命周期 + 发布历史。
- **查重窗口**：同素材包同平台最短复用间隔（默认 30 天）；窗口内全部目标平台有发布 → 用尽。
- **收件箱**：根目录 `收件箱/`，Claude/Codex TXT 与外部 AI 图片落盘后自动收录（notify + 2s 防抖）。
- **待认领**：收件箱无法关联已知 SKU 的内容，进队列由人工指认，不丢弃。
- **疑似已发**：超时无回执标记（琥珀）；**绝不自动重发**，只能人工核实后定态（硬性 §6.4）。
- **相对路径是真相**：库内/包内只存根目录内相对路径（RelPath）；导出是唯一绝对路径转换点。
- **五平台**：`douyin/xhs/kuaishou/shipinhao/bilibili`（抖音/小红书/快手/视频号/B站），
  中文名↔枚举映射单点在 `publish/platform.rs`；文本平台标签另有 `general`（通用）。

### 视频流水线词汇（v0.15.0）

- **clip(v2v\_clips)**：一张验收图的**一次**视频尝试。七态与 `tasks.status` 同构：
  `rewrite` 待改写 → `ready` 待提交 → `run` 已提交 → `rev` 待验收 → `pass`/`rej`/`fail`。
  `UNIQUE(work_id)` —— 一张图同时只有一条在跑；重跑是就地 `attempt+1`，不新增行。
- **交接目录**：默认 `~/GenDesk交接/v2v/`。`待改写/index.jsonl` + `待改写/<组>/manifest.jsonl`
  由 GenDesk **自动物化**（队列一变就重写，不需要点导出）；skill 写回
  `已改写/<组>/rewrite.jsonl`，watcher 收录后移档到 `_已收录/`。
  **组目录名对同一组恒定**（`g{group_id}`），否则 skill 每轮都看见「新」目录重复改写。
- **skill 的职责边界**：只把生图提示词改写成图生视频提示词。**不调 dreamina**——提交/轮询/
  下载/重试/验收都在 GenDesk 里（那些不是智能任务，让 LLM 轮询既慢又贵还不可靠）。
  故 v0.13.0 那份 `ledger.jsonl` 已取消：真相在库里，少一个真相来源是收益。
- **封面(poster)**：clip 自己的文件（首帧缩略图的**副本**，`clips/clip{id}.jpg`）。
  绝不指向 `accepted_works.thumb_path` —— 清空废纸篓会物理删 file_paths，
  删一条未通过的视频就会顺手删掉还活着的那张作品的缩略图。
- **重跑 vs 退回改写**：视频不通过多半是**没抽中**而不是提示词不对，故「重跑（同提示词）」
  是默认动作；「退回改写」才清掉 video\_prompt 让 skill 重写。
- **额度不可撤回**：提交成功即写 submit\_id 并置 run（顺序反了会留下认不出主人的孤儿，
  而恢复只能退回重提 = 花两份钱）。中断恢复**只**动无 submit\_id 的条目。

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

`pnpm check` 与 CI（`.github/workflows/check.yml`）互为镜像；`lefthook.yml` 是它的秒级
子集（guardrails + biome + tsc + fmt + clippy），装了 pre-commit 也仍需在提交前跑全门禁。

**注意**：门禁第 8 步会校验 `bindings.ts` 已同步（`git diff --exit-code`）。改过 Rust
契约后若只重新生成而未 `git add`，这步会红——**先 stage 再跑 `pnpm check`**。

**数据层实现说明**：sqlx 采用运行时校验查询（`query`/`query_as`），SQL 由针对临时库的
`cargo test` 集成测试覆盖（比仅编译期检查更强）；故未接入 `cargo sqlx prepare --check`。
CI 已接入 `cargo llvm-cov`（**engine / ids / importer / publish 纯逻辑 ≥ 85%** 行闸门，
check.yml；`commands/` · `db/repo/` · `provider/` · publish 的 IO/glue 由集成测试覆盖，
经 `--ignore-filename-regex` 排除出闸门）与每周 `cargo audit`（`--ignore` 无修复传递告警，
audit.yml）。

**迁移约定**：`src-tauri/migrations/` forward-only，发布后不可改（`db/repo/mod.rs` 里
「重放 0019 搬运语句」那条测试正是建立在这个前提上）。默认留在事务内；只有必须
`PRAGMA foreign_keys=OFF` 的表重建才用 `-- no-transaction`（见 0017 的教训）。

## 里程碑进度

- [x] **M0 骨架与门禁** — Tauri2+React19 骨架、设计 tokens、窗口壳、命令面板、质量门禁全套。
- [x] **M1 数据层与基础域** — migration 0001 全 schema、号池(proptest)、files(缩略图/命名/废纸篓)、importer(GBK/两段式)、settings/api\_keys(当时用 keyring，v0.10.0 已改本地加密文件)/refs/prompts 域命令 + 前端 settings store。
- [x] **M2 任务引擎** — 状态机(proptest)、Provider(OpenAI 兼容, wiremock 7 用例)、错误分类六类、
  调度器(per-Key Semaphore + 两策略 + 指数退避)、伪进度、中断恢复、1→500 压测；batches/tasks
  域命令 + 4 事件(status/progress/summary/keyHealth)。M2 出口 cargo-mutants 已跑（见 M5）。
- [x] **M3 业务页面** — 八大页面全部按原型实现（设置/生成/任务/验收/作品/提示词/参考图/废纸篓）；
  review/works/trash + 提示词库/参考图详情 后端域；前端引擎事件 store + 导航徽章(运行/验收/废纸篓) +
  ⌘K 操作补全 + 共享 UI。核心闭环「配 Key→生成→实时任务→验收→输出/废纸篓」端到端可用。
- [x] **M4 更新发布链** — tauri-plugin-updater/process；minisign 密钥（公钥入 conf，私钥 `~/.tauri/`）；
  createUpdaterArtifacts + NSIS per-user + WebView2 bootstrapper + macOS 12+；check/install 命令 +
  `update://state` 事件 + 标题栏 pill + 设置手动检查 + 启动自动检查；release.yml（tag v\* → 双端 + latest.json）。
- [x] **M5 收尾质量关** — pnpm/cargo audit 清零（无修复传递告警书面豁免）；cargo-mutants(engine/+ids/)
  存活体全部补测试或书面豁免（docs/mutants-exemptions.md）；八页 UI 冒烟 + reduced-motion；
  §7 V2 预留自检 + V2 backlog（docs/V2-backlog.md）。**人工收尾清单**（AI 不可替代，交付前执行）见 V2-backlog.md。
- [x] **UX 优化阶段 E01–E41（v0.3.0）** — 三批 41 条 UX 评审建议全数落地（计划文档已随 445b396
  删除；四阶段为 M6 安全修正 → M7 生成调度 → M8 验收工作台 → M9 资产运维）。migration 到 0009；
  单分支 `feat/ux-overhaul` 小步提交（每项/组一 commit 注明 Exx，保 bisect），一次 PR 合入 main。
  **代码里 `E07`/`E30b`/`E41` 这类编号仍在注释中大量出现**，指的就是这批条目——文档没了，
  但 `git log --grep=E30b` 能找到对应 commit。
- [x] **生成输出处理 + 6 项 UX（v0.4.0）** — 输出元数据/C2PA 剥离（provider::sanitize）、生成页两栏消留白、
  缩略图瀑布流、废纸篓详情、任务多选删/重试、提示词 txt 宽泛解析等（PR #5）。
- [x] **图片生成页 1:1 重构（v0.5.0）** — 按 Claude Design handoff 原型将生成页由堆叠卡片改为两栏就地挂靠：
  左栏彩色可展开词组卡（配色 gc0–gc4，可拖拽 + 悬停交叉高亮），右栏参考图就地弹层/拖放挂靠，
  生成参数移入底栏「参数 ▾」弹层；提示词原文改上一条/下一条弹窗。纯前端（GeneratePage + globals.css），
  无 migration/无新 IPC，删除旧生成页样式。
- [x] **发布与资产管理模块（v0.7.0）** — 三阶段（P1 资产管理 / P2 编排导出 / P3 回执闭环）。
  migration 0010（skus/asset_packs/text_items/accounts/daily_sets/task_sheets/publish_tasks/
  usage_ledger/inbox_items + 内置通用分组）。新顶层模块 `publish/`：paths(RelPath/四分区/ASCII/
  win-mac 拼接)、platform(五平台单点)、inbox(parser 三类 TXT+话题+SKU 三冗余、notify watcher+2s 防抖、
  ingest 收录事务+媒体归集)、planner(set_picker/scheduler[proptest 五不变量]/frequency/generate_sheet)、
  xlsx(writer 22 列+reader 表头定位)、exporter(任务包+READY 最后写)、reconcile(三分支+六类处置+疑似已发+
  关单日报)、ticker(应用内定时+补跑)、events(3 事件)。~40 IPC 命令（publish_settings/skus/texts/assets/
  inbox/accounts/planning/reconcile 域）。前端两新页（资产库/发布计划三页签）+ 设置「发布与同步」区块 +
  导航两项(⌘9/⌘0)+徽章 + publish store + 作品库「入资产库」。覆盖率闸门扩展至 publish 纯逻辑目录。
  67 publish 测试（含 proptest + 端到端 + 疑似负向断言）。发版节奏三阶段三 PR 一次发版，全程不打 tag，
  P3 收尾后一次性 bump 0.7.0 + tag v0.7.0。
- [x] **发布模块优化 35 项 A–F（v0.9.0）** — 三轮全量审查产出的 35 项，六批次全数落地
  （《内部图片生产工具_发布模块优化执行计划.md》）。migration 到 0015。249 Rust 测试。
  - **A 发版阻断**：素材包入库即 active（原 `new` 使排期永远选不到包，每个 SKU 恒报「无可用素材包」）；
    回执/计划时刻按**本地时区**解析（原 `and_utc()` 把北京时间当 UTC，疑似已发晚 8 小时才标）；
    收件箱丢弃改移档（原来只删 DB 行，下轮 rescan 就复活）+ 逐文件容错 + 事件去重；
    未知 SKU 的媒体进待认领；RelPath 剔除 `..`、SKU 编码拒 Windows 保留名、编码大小写唯一。
  - **B 回执与导出**：导出预检（素材齐备/路径长度/账号在用/**重导出回执保护**——xlsx 是双侧唯一
    契约，覆盖=抹掉执行器回执）；取消只允许 pending（suspect 不得绕过 §6.4）+ `cancel_kind` 区分
    人工/风控；timeout 次日补排 + content 退役素材包 + login 上报；写路径事务化 + `HH:MM` 统一校验；
    回执 xlsx 快照留底。
  - **C 排期算法**：查重窗口按平台剔除（原来只要有一个平台没发过，包就展开到**全部**平台，
    直接违反「同素材包同平台 30 天」）；日限裁剪按日轮转（原来天天裁掉 id 大的同一批）；
    冷款轮播步进 M；每步独立抖动 + 跨平台错峰；账号级时段 + 无账号进缺料 + hotDaily 改开关。
  - **D UI 闭环**：事件驱动刷新（sheetRev/inboxRev，不轮询）；账号/时段/文本/SKU 平台覆盖编辑；
    素材缩略图；选择器搜索 + 列表防抖。
  - **E 稳健与性能**：导出走 spawn_blocking + 进度事件；N+1 批量化；watcher 文件大小稳定探测；
    入库原子回滚 + 删除引用校验；归档保留期；草稿保护（`edited` + 重生成确认）；暂停排期；
    classify_fail 把 timeout 提到 content 之前（「上传素材超时」归成 content 会白白退役好素材）。
  - **F 新功能**：补料提示词（模板由反向 parser 测试守住）· 回执截图 · 资产跑道 · 排期预演 ·
    发布月历 · 开屏晨报 · 拖放直投 · 看板日期切换 · 同步链路健康 · 素材包使用统计。
- [x] **密钥存储迁移（v0.10.0）** — API Key 由系统钥匙串迁到本地加密文件，根治自签名下
  每次更新/重编译反复弹 Keychain 授权（无可信签名身份 → 按应用 ACL 的「始终允许」无法跨版本
  存活，系统固有限制）。`secrets::FileStore`：`secrets.key`(32B 主密钥) + `secrets.enc`
  (XChaCha20-Poly1305，24B 随机 nonce 前置)，原子写 + 0600 + 损坏自愈留证；启动时
  `migrate_from_keyring` 幂等搬运（先写目的地再删源，单条失败不删源、不中断启动）。
  **安全水位如实记录**：防误不防恶（防备份/截图/grep 出明文），主密钥与密文同目录，
  不构成独立安全边界；爆炸半径 = 可轮换的第三方 API Key。无 migration / 无新 IPC / 无前端改动。
- [x] **生成页归档 + 并发 100 + 验收批次序（v0.11.0）** — 四项用户反馈。migration 0016（归档位，
  事务内）+ 0017（api_keys 重建，`-- no-transaction`）。
  - **生成页开始即归档**：`engine::create_batch` 同事务给本批参考图与提示词组打 `archived_at`；
    归档**只**决定生成页两个选择器是否列出它，库里仍在、可查、可一键取消归档（提示词库分组菜单 /
    参考图详情）。选择器加「显示已归档 · N」开关，**打开弹窗时已选中的项恒可见**（取 initial
    selected 而非实时 sel，否则取消勾选会让卡片当场消失）——「按此配置再来一批」照常可用。
  - **单 Key 并发 10 → 100**：api_keys 是 tasks / task_attempts 的**父表**（ON DELETE SET NULL），
    FK 开启时 DROP 父表触发隐式 DELETE 会把子表 api_key_id 整列置空（成功率统计 + 验收「按 Key」
    分组一并报废），RENAME 又会改写子表 REFERENCES；故 0017 走 `PRAGMA foreign_keys=OFF` 的官方
    12 步（`legacy_alter_table` 在事务内无效，已被测试抓到）。测试断言子表 schema 仍写
    `REFERENCES api_keys` —— 守迁移方式而非上限数字。行内步进器改直接输入。
  - **验收按批次倒序**：`ORDER BY t.batch_id DESC, t.id ASC`；前端新增「按批次」聚类并设为默认。
  - **修复 Key 行「编辑/删除」被裁**：`.kline` 十列定宽合计 ≈773px > `.swrap` 内容宽 720px，
    `.klist` 又是 `overflow:hidden` → 末两列被整齐切掉，表现为「没有删除和编辑功能」。
    文本列改 fr 自适应 + `.klist` 兜横向滚动；`.kline .inp` 补 `width:100%`（否则 number 输入
    按内在宽度撑出格子压到「成功率」列）。
- [x] **修复并发上限只生效到 10（v0.11.1）** — 上条「10 → 100」漏改引擎装载处：
  `engine::load_key_configs` 仍是 `clamp(1, 10)`，于是设置页填 50 → 命令层按 100 夹取通过 →
  DB 真存 50 → **引擎读出来夹回 10** → `set_keys` 据此建 `Semaphore::new(10)`，其余任务恒卡 `q`。
  症状有欺骗性：设置页与 DB 查出来都是 50，只有真正跑的信号量是 10。修法是消除重复定义而非改
  数字 —— `MAX_CONCURRENCY` 单点定义在 `db/repo/api_keys.rs`（与 0017 的 CHECK 同文件），
  写入侧（命令层夹取）与执行侧（引擎 Semaphore 容量）都引用它。**回归测试取样值必须 >10**：
  既有那条 `load_key_configs_*` 用 5 取样，夹到 10 和夹到 100 下都通过，正是它放过了这个回归。
  无 migration / 无 IPC / 无前端改动；已存的 50 不必重填，重启即生效。
- [x] **导入分组识别 + 预览可编辑（v0.12.0）** — 用户实测「10 次导入 8 次分组识别失败」。
  根因不是解析崩了（正文条数一直是对的），而是分组头**只认语法标记**（`分组:` / 独立括号行），
  而手写 txt 最常见的写法是「首行一个裸标题 + 下面全是长段落」——一个标记都没有，
  于是整份塌进「未分组导入」，还回一句「可在文件开头加一行『分组: 名称』」，把改格式的活推给人。
  - **形态推断**（`importer`）：判层依据从「写没写关键字」换成「**这行管着几条正文**」——
    管 ≥2 条 → 分组头，恰好 1 条 → 那条的小标题。仅在**全文一个显式标记都没有**时启用
    （`heuristic_mode`），文档一旦自己表过态就完全听它的。门槛保守：正文 75 分位 ≥60 字、
    标题 ≤40 字且 ≤ 基准 1/3、无句末标点、无前导序号 —— 只在长段落文档上生效，不啃短句正文。
  - **基准取 75 分位而非中位数**：标题行自身也在样本里，两层结构（标题/小标题/长正文）下
    短行可占一半，中位数会被拉到标题长度，推断当场失效（`plain_heading_two_levels` 抓到过）。
  - **猜错不丢内容**：推断出来却没管到任何正文的组名，由 `salvage_empty_inferred` 还原成
    提示词挂回相邻组。形态推断最坏只是「分组分歧」，绝不静默吞条。
  - 裸括号补齐同一规则（`【某某】` 下跟 ≥2 条正文且此前无分组 → 认作分组，原来一律当小标题）；
    无线索时用**文件名**兜底命名（剥尾部日期/`(1)`/副本），不再叫「未分组导入」；
    「正文在分组标记前」告警只在旁边确实还有别的分组时才报，且不再要求回去改文件。
  - **预览弹窗从只读改为可编辑**：改组名/前缀、`↑↓` 并入相邻组、`✂` 按条拆新组、改小标题与
    正文、删条删组；推断出的组标琥珀「疑似 · 就这样」一键确认。认错了当场改，不必回去改 txt 重导。
  - 新 IPC `repreview_import`：结构性改动后重算前缀/编号区间/是否并入已有组。commit 侧按最终态
    兜底校验（空组跳过、空正文剔除、`sanitize_prefix` 规整），**不信任前端结构**。
  - 既有 24 条解析测试**一字未改**全部通过 + 新增 11 条（用户真实文件形态、1:1 交替不误判、
    长短混排不丢条、文件名清洗）。无 migration。

- [x] **用途标签 + 图生视频包导出（v0.13.0）** — 起点是一个反例：作品库积着不同用途的图，
  只有一小部分需要做视频。实测 batch 15 的 19 张与其余 92 张**零重叠**——全库含「动势」14 条、
  「这一帧」11 条、「9:16」19 条，全部落在那一批，它是唯一为视频而写的批次。migration 0018。
  - **用途标在提示词组上**：一张图的用途由它的提示词决定，提示词的用途由那份 txt 决定，
    而一份 txt = 一个组。批次会混组（batch 7 混了几十个组），所以批次不是用途单元。
    机制此前已建好 80%（tags/tag_bindings 表 + importer 解析 `标签:` + 提示词库按标签筛选），
    但**唯一写入口开在导入 txt 那一刻**，而用户的 txt 从不带语法标记（v0.12.0 形态推断正为此
    而生）→ 全库 tags 表长期一条记录都没有。本次补上实际会走的那条写路径。
    `purpose.rs` 受控用途单点（同 `publish/platform.rs`）；取值在**命令边界**强制校验而非只靠
    UI 给选择器——命令是公开边界，放进自由字符串就会「图生视频/图转视频/v2v」三种拼法同时进库。
    `set_prompt_group_purposes` **只替换用途标签、保留 txt 导入的自由标签**（两套东西恰好共用
    一张 tags 表）；`bind_group_tags` 导入与 UI 共用，标签名规整只此一处。
  - **一包一组**：不是为了目录整齐——同组分镜最后要剪进同一条成片，运镜语言与时长必须统一，
    跨组混包改写风格会飘。（曾按全库 50 个组的碎片分布判断「不该按组切」，那是把不同用途的组
    混在一起看造成的错觉；限定用途后分布是 7/5/4/3。）
  - **主键取 `accepted_works.id`（`W{id}`）而非文件名**：输出名 `..._BR140010_1.JPG` 的编号已
    去连字符，`BR140010` 反推不出是 `BR14-0010` 还是 `BR1-40010`——文件名本来就不可逆；历史批次
    更早于抽卡序号落地，连结构都不一致。中文原名只作 `displayName` 留在 manifest 给人看。
  - **组内公共前后缀剥离**：四个组各有 147/165/384/305 字逐字相同的产品保真尾巴（「哪个环穿
    哪个孔、谁挂在谁之上一律不得移动」）。图已是首帧、产品已画对，再喂 300 字配件穿接关系只会
    把改写带偏。按 **char** 而非 byte 切（按字节切会切碎中文）；单条不剥离（它跟自己的公共缀
    就是全文）；剥完为空回退全文。公共缀取自该组**全部**验收作品而非本次所选——超集的公共缀
    必是子集公共缀的前缀，取超集更保守。剥离只是提示不是契约：manifest 同时给 `sourcePrompt`
    全文与剥离字数，猜错不丢信息。实测剥后可变部分占全文 51–82%，起始正是场景描述。
  - **包结构**：`manifest.jsonl` 一行一条（可 grep、可 `head -n` 分片，skill 不必整包读进上下文）
    + `ledger.jsonl`（留给 skill 追加，同 id 最后一条即当前态）+ `images/`（喂即梦）
    + `thumbs/`（喂模型读图，384×512 约 260 token，比原图省一个量级）+ **READY.txt 最后写**。
  - **跨包去重台账 `work_exports`**：包内 ledger 只管得住包内，包被移走/删掉就失忆，同一张图会被
    反复导出反复花额度。只新增表不 ALTER 既有表；`channel` 留给未来别的下游，**不给
    accepted_works 加一次性布尔列**（第二个下游来了就要再加一个）。不设 FK：作品进废纸篓后台账
    仍要答得出「这张图当时导出过」。**台账在包写成之后才记**——反过来会留下「记了没导出」的假
    记录，而「隐藏已导出」正是靠它筛，假记录会让那张图从候选里永久消失。
  - 用途是**筛选默认值不是门禁**：作品库照旧允许手选任意作品导出，堵死了就得改代码。
  - 294 Rust 测试。**后续**（本次未做）：Claude Code 侧 skill（prep/run/pull）、视频回流与
    视频验收页签（`video_clips` + 废纸篓 `clip` 类 + CSP 补 `media-src` + poster 须独立成文件，
    否则清空废纸篓会删掉还活着的作品缩略图）。
- [x] **参考图库独立 + 上传进度 + 参数自检（v0.14.0）** — 六条用户反馈。migration 0019。299 Rust 测试。
  - **上传静默是主症**：导入一次十几张，后端逐张「拷贝 + 解码 + 缩略图 + hash + 压缩副本」，
    十几秒里界面一声不吭，还全跑在异步执行器上（纯 CPU 活占着 IPC 线程，连别的命令都卡）。
    用户以为没点上，反复重按 → 同一批图进库五六遍。修法三件：`ingest_one` 移进
    `spawn_blocking`；逐张推 `refs://import-progress`（含当前文件名与失败计数）；前端
    `useRefImport` 的重入锁用 **ref 而非 state**（`useState` 的 busy 要等下一次渲染才生效，
    挡不住同一帧内的连点）。顺带逐张容错：一张坏图只记一次 failed，不再中断整批。
  - **生成页上传即临时（ephemeral）**：随手拖一张跑一次的图不该长住长期图库。仍是 ref_images
    行（tasks/batch_refs/accepted_works 都以它为父表，不落库不行），但图库页与「从参考图库
    选择」都不列它，**去重基准也剔除它**——否则用户正式导入一张自己刚在生成页试过的图，
    会收到一句莫名其妙的「重复」。`list_ref_images` 仍返回它：切在后端，生成页当场就显示
    不出自己刚传的图，过滤只能在消费端做。
  - **图库分组与提示词组解绑**：`ref_images.group_id` 原本指向 **prompt_groups**——图库的目录
    一直跟着「一份 txt = 一个组」的节奏变形，还混进临时组。新建 `ref_groups`（NOCASE 唯一）
    + `ref_group_id`，并把既有归属**按同名搬过去**而非丢进未分组：眼前的结构一张不动，只是
    链子断了。历史列 `group_id` 保留不读不写。前端加「管理分组」（新建/改名/删除，删组不删图）。
    既有两条 refs 测试改断言 `ref_group_id` —— 不是放宽，是语义换了：原样跑会直接撞外键
    （FK 恰好抓住了这次切换）。
  - **toast 移到右上**：右下角是各页主操作按钮（开始生成/导出/确认/删除）的固定位置，黑色
    toast 压在上面只能干等。`offset.top=56` 让开 44px 标题栏（否则改压住「跳转」与 Windows 窗控），
    驻留 2.6s。
  - **生成参数自检**：size/quality 的透传链（params_json → batches → dispatcher → multipart）
    实测完好，wiremock 两条测试已守住。真问题是**抽卡次数不进快照**——「按此配置再来一批」
    把 ×3 悄悄还原成 ×1，任务数对不上而没人知道为什么；现随快照记录并夹取 1..=5。
    另新增 9:16 竖幅（1080×1920）+ **自定义尺寸直填**（不同兼容端点认的取值枚举不一样，
    写死预设等于赌），并在参数弹层与开始生成确认卡里直书**「实际发往接口的字段」**——
    「设置了远端却没收到」这类怀疑，只能靠把请求内容摆到确认之前来消除。
    GenParams 明确**不加** `deny_unknown_fields`：快照比它宽（draws/watermark 等纯 UI 键），
    严格解析会让整份快照退化成「全部空」，用户选的 9:16 反而一个字段都发不出去。
- [x] **图生视频流水线 + 作品库重构（v0.15.0）** — 起点是一句「感觉很乱」。诊断出来的病根
  不是「验收放在哪」，是**导出即失去身份**：v0.13.0 把状态交给包内 `ledger.jsonl`，包一被
  移走/删掉/重建就失忆；而视频的**终点本来就在库内**（发布模块的视频型素材包 = 1 视频 + 封面）。
  终点在里面、中段在外面 → 两边各拥有一半真相 → 没有任何一处能回答「这批视频做到哪了」。
  migration 0020。355 Rust 测试。
  - **边界重划**：GenDesk 全程持有流水线状态；Claude Code / Codex 侧的 skill 退化成
    **无状态的改写服务**（读工单 → 写回改写结果），提交/轮询/下载/重试/验收全在本机。
    理由是分工而非洁癖：轮询不是智能任务，让 LLM 在 agent 循环里干这个既慢又贵还不可靠。
    `ledger.jsonl` 随之取消 —— 少一个真相来源是收益。
  - **`v2v_clips` 七态与 `tasks.status` 同构**（rewrite/ready/run/rev/pass/rej/fail），学一次用两处。
    `UNIQUE(work_id)`：一张图同时只有一条在跑，重跑是就地 `attempt+1` 而非新增行，
    否则看板堆出同一张图的多条重影，「这张图做到哪了」又变成没有答案。
  - **交接是自动的，不是一个按钮**：「验收通过后不需要点导出」要成立，就必须由**状态变化**
    触发物化。组目录名对同一组恒定（`g{group_id}`）——带时间戳的新目录会让 skill 每轮都
    看见「没见过的」目录、重复改写同一批。READY.txt 最后写；收录后移档留证（同 v0.9.0）。
    只监听「已改写」：连「待改写」一起监听会形成 物化→事件→收录→物化 的自激循环。
  - **额度是一次性的**，故顺序不能反：提交成功→立刻写 submit_id 并置 run。反过来会留下
    「跑着但认不出是哪条」的孤儿，而恢复只能退回重提 = 花两份钱买同一条视频。
    `recover_orphan_submits` 因此**只**动无 submit_id 的条目。
  - **未知 `gen_status` 判 Running 而非 Failed**：CLI 加一个新中间态时，判失败会把额度已扣、
    正在跑的任务当场标死；判运行最坏多轮询几轮，由 45 分钟超时兜底。只认落盘 `path`
    不认 `video_url`（签名会过期，存进库等于存一条几小时后必然 404 的引用）。
  - **封面 = 首帧缩略图的副本**（`clips/clip{id}.jpg`，独立成文件）。image2video 的第一帧
    就是那张图，语义正确且不必依赖 ffmpeg；更关键的是不能指向 `accepted_works.thumb_path`
    —— 清空废纸篓会物理删 file_paths，删一条未通过的视频会顺手删掉还活着的作品缩略图。
  - **命令行摆到确认之前**：`dreamina::command_line` 是执行与展示的同一来源。CLI 的 flags
    会随版本变（skill 文档自己就写「不要硬编码模型支持」），对策不是赌它不变，而是让
    「我设了却没生效」这类怀疑无处可生。提交前本地预检模型/时长/分辨率组合——半套组合是
    最容易踩的坑，而 CLI 的拒绝发生在花钱之后，批量 20 条会连报 20 次同样的错。
  - **重跑是不通过后的默认动作**：视频不通过多半是没抽中，不是提示词不对。「退回改写」
    才清掉 video_prompt。
  - **作品库：分组从「轴」降级为「筛选」**。根因不是分组太多，是「一份 txt = 一个组」让分组
    天然是**出货单位**而非分类法——它只会越来越多，永远不会是好的浏览轴（旧代码把全部分组
    平铺成 segmented control，实测 187 组时物理上不可用）。改为默认按**批次**倒序分节
    （实测 132 张作品 → 5 节，batch 7 一节混了 42 个组），节头带「全选本节」；分组变可搜索
    popover；新增全文搜索（编号/组名/参考图/正文一次覆盖，人搜时并不知道自己记住的是哪一处）；
    接上分页。排序改 `batch_id DESC, id ASC`（同验收页）——按 accepted_at 排会让隔天补验收的
    同一批被切散到两个日期，分节当场失效。
  - **用途在导入那一刻定**：一份 txt 是为一个用途写的，那是唯一 100% 知道答案的时刻。
    预览弹窗行内选择器 + 关键词预猜（B-Roll/分镜/首帧，标琥珀「疑似」），**只看组名/场景/标签
    不扫正文**——正文里偶然出现「首帧」不该把整组标成视频用途，预猜错的代价必须低于不猜。
  - **存量补标**（`backfill_group_purposes`）：导入侧只覆盖以后的 txt，而实测存量 `tags` 表
    一条记录都没有，187 组里 33 个组名带 `B-Roll`/`分镜`、覆盖 40 张验收图。不补就等于
    「验收自动入队」对全部历史资产失效，手点 33 次是白干的活。**只增不减**，已标过的跳过
    （人手动取消掉的用途不该在下一轮补标里复活）。
  - 路由 `shortcut: number | null`：十个数字已用尽，新页无数字快捷键。**不为新页重排既有数字**
    —— 那会把肌肉记忆一次性作废，代价远大于少一个快捷键。
  - **验证**：migration 0019+0020 在**真实库副本**（132 作品 / 187 分组）上跑通，
    `foreign_key_check` 与 `integrity_check` 均干净；新 WORK_SELECT 与新排序在真实数据上验证。
    覆盖率 89.3%（闸门 85%），`v2v/watcher.rs` 按既有惯例（同 `publish/inbox/watcher.rs`）
    排除出闸门。**未做**：独立上下文 `/code-review`（§1.4 要求），建议合并后补跑。
- [x] **即梦 CLI 定位 + 改写规范按官方指南重写（v0.15.1）** — 用户装好 CLI、终端里 `dreamina`
  跑得通，应用里却报「找不到即梦 CLI「」」。**两个原因叠在一起**，各修一个都还是不通：
  - **空串直接当 argv[0]**：`bin` 的 serde 默认值只在字段**缺失**时生效，而设置页那个输入框
    可编辑 + onBlur 即存，于是存进来一个空串 —— 报错连名字都没有（`「」`），最难查的正是
    这一点：错误信息没有指向任何东西。
  - **GUI 进程根本没有终端的 PATH**：实测正在跑的 GenDesk.app 是
    `PATH=/usr/bin:/bin:/usr/sbin:/sbin`，而 dreamina 装在 `~/.local/bin`。所以旧文案
    「dreamina（走 PATH）」对打包应用是**一句空话**；而从终端 `pnpm tauri dev` 起的开发实例
    继承了完整 PATH，恰好把这个坑藏起来——「开发能跑、装上就不行」的典型成因。
  - 修法是 `dreamina::resolve_bin`：留空/裸名 → 先 PATH 再翻 `~/.local/bin`、`/opt/homebrew/bin`
    等常见位置；**填了路径就只认它**，不存在就直说是哪个路径不存在（偷偷回退到探测结果，
    会让用户填错了路径也「跑起来」，换台机器再神秘失败）。找不到时把翻过的目录一并报出来——
    这个错误的全部价值就在「我找过哪儿」。三个执行入口（`user_credit`/`submit`/`query`）与
    提交预览统一走它，故确认卡里显示的绝对路径就是即将 exec 的那一个。设置页直接显示
    「实际会执行：<绝对路径>」+ 文件选择器，**不让用户去回答「路径填什么」**。
  - `resolve_in` 把搜索目录做成参数才可测：本仓库 `-F unsafe-code`，而 Rust 2024 起
    `env::set_var` 是 unsafe fn，测试没法改 PATH。6 条新测试，含「空串走默认名」这条回归。
  - **改写 skill 按官方提示词指南重写**（火山方舟 Seedance 1.0 / 1.5 pro 提示词指南）：
    官方公式「主体+运动+环境+运镜+美学描述」，图生视频**略掉主体与环境的外观描写**（首帧图
    已定死），只写运动 + 运镜 + 约束；官方运镜词表（推/拉/摇/移/跟/升/降/甩/环绕/旋转/变焦）、
    运镜公式（起幅+运镜+幅度+落幅）、景别语法「主体+景别」、「善用程度副词」、
    「用特征指定主体且全程一致」。手持质感改用官方术语**「带轻微手持的呼吸感」**（原先只靠
    「手机实拍质感」隐式带出来）。
  - **动静分层**是本项目的核心规则，也是对旧 skill 的一次纠正：旧版一律要求「只给一处运动 +
    镜头固定」，但用户挑出来的两条最满意的成片恰恰一条是**镜头推近**、一条是**背景人影走动**
    —— 真实场景元素（人/宠物/路人/光影/机位）**可以动**，那正是真实感的来源；只有**产品本体**
    必须锁死，逐项点名配件 + 明写**不发生形变**。CLI 无 `camera_fixed` flag（`ratio` 也由首帧图
    推断），故镜头是否动只能写在提示词里。
  - `改写说明.md`（GenDesk 自动物化进交接目录的那份）同步带上这套要点：用户也在 Codex 里跑，
    那边没有 Claude skill，工单必须自解释。
- [x] **画幅改走 aspect_ratio + 生成参数收敛到三项（v0.15.2）** — 用户报「设了 9:16 报错
  `edges must be multiples of 16 (got "1080x1920")`，不设又常回 1:1，可我每条提示词都写了 9:16」。
  两件事叠在一起：
  - **1080 不是 16 的倍数**（1920 是）。v0.14.0 那个「9:16 竖」预设 `1080x1920` 是精确比例，
    却撞上端点对边长的硬要求，于是这个预设**从来就没成功过**。
  - **更根本的是参数选错了**：该端点的 gpt-image-2 系列用 `aspect_ratio` 控制画幅
    （1:1/16:9/9:16/4:3/3:4/3:2/2:3/21:9，**仅保证比例**，像素由上游定），而我们只发 `size`。
    不发比例参数 → 模型默认 1:1；**提示词里写「9:16」对模型不构成约束**，那是描述不是参数。
  - **参数只做三项**（用户定的范围）：`aspect_ratio` / `size` / `output_format`，外加恒定
    `n=1`。文档里的 quality / response_format / background / output_compression /
    extra_fields **一律不做**——先按文档全做了一版，用户看完直说「绝大部分用不到」；参数
    摆在界面上却没人用，只会让「到底哪个在起作用」更难回答。删的时候连 `GenParams` 字段
    一起删，不留「界面上没有、后台仍在发」的隐形参数。
  - **抽卡不是 `n`**：抽卡 k 次 = **k 个任务**（各自独立重试与验收），不是发 `n=k`——
    一次响应里 k 张图只有一张能落进当前任务，其余直接丢掉且照样计费。故 `n` 恒为 1。
  - **`output_format` 同时决定本地交付格式**（`openai::deliver`）：默认那条「清元数据 +
    去 C2PA 全开 → 统一重编码 JPEG」的规则会把选中的 PNG 悄悄变成 JPG，而「选了 PNG 拿到
    JPG」是纯粹的失信。故显式选择优先：PNG 走容器级剥离（抹 tEXt/zTXt/iTXt/eXIf 与 caBX），
    远端给的不是 PNG 就重编码成 PNG；未选格式则一字不改沿用旧规则（有防回归测试）。
  - **入口严格、内部宽松**：`parse_checked`（命令边界，键类型不对就报错 + 受控取值/边长
    预检）vs `from_json`（调度器读已落库批次，坏了也要能跑）。预检放在 create_batch
    是因为端点的拒绝发生在**计费之后**，一批 20 个任务会连报 20 次同一个错。
  - 生成页参数弹层收敛为：比例（首项，旁边直说「提示词里写 9:16 对模型不构成约束」）·
    精确尺寸（选填，边长非 16 倍数当场标红并给出可用值，同时禁用「开始生成」）· 输出格式
    （PNG/JPG）· 抽卡次数，再加本地的去水印/AI 元数据/C2PA。「实际发往接口的字段」由构建
    请求用的那份 wire 记录直接渲染——展示与执行同一来源。
  - 371 Rust 测试。无 migration / 无 IPC 改动（`params_json` 是字符串，bindings 不变）。
