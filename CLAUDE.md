# [CLAUDE.md](http://CLAUDE.md)

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## 项目概览

GenDesk：内部图片生产工具。Tauri 2 桌面应用，单实例、纯本地（SQLite + 本地文件），无服务端。
前端 React 19 + Zustand + Vite（`src/`），后端 Rust（`src-tauri/src/`），二者只经
tauri-specta 生成的类型化 IPC 通信。仅支持 Windows 10/11 x64 与 macOS 12+ Apple Silicon。

四条流水线：**生图**（参考图 + 提示词 → 批量生成 → 人工验收 → 输出归档）·
**图生视频 v2v**（验收图 → skill 改写提示词 → 即梦 CLI 提交 → 轮询落盘 → 交付）·
**发布与资产管理**（素材包 → 日内容套装 → 任务单 xlsx → 执行器回执闭环）·
**工单收件 intake**（外部 skill 投单 → 自动导入 → 建批开跑）。

## 常用命令

前置：Node ≥ 22 · pnpm 9.15（`packageManager` 已锁）· Rust stable。

| 目的                    | 命令                                                                         |
| --------------------- | -------------------------------------------------------------------------- |
| 启动双端开发                | `pnpm tauri dev`                                                           |
| **全门禁（CI 镜像，提交前必跑）**  | `pnpm check`                                                               |
| 前端类型检查 / 单测 / lint 修复 | `pnpm typecheck` · `pnpm test` · `pnpm lint:fix`                           |
| 架构铁律检查                | `pnpm guardrails`                                                          |
| Rust 测试 / 静态检查        | `cd src-tauri && cargo test` · `cargo clippy --all-targets -- -D warnings` |
| **重新生成 IPC 绑定**       | `cd src-tauri && cargo test --lib export_bindings`                         |
| 压测（dispatcher）        | `cd src-tauri && cargo test --release -- --ignored`                        |
| 安装 pre-commit         | `pnpm dlx lefthook install`                                                |

**跑单个测试**（迭代时别整套跑）：

```bash
cd src-tauri && cargo test --lib refs_insert_list_setgroup -- --exact --nocapture
```

按模块过滤用子串匹配无需 `--exact`：`cargo test --lib publish::planner::`。
前端：`pnpm vitest run src/routes.test.ts`。

**本机 shell 陷阱**：非交互 shell 里 `cd <repo> && <node/pnpm 命令>` 会被 fnm 的 use-on-cd
钩子打断（`We can't find the necessary environment variables to replace the Node version`），
**报错来自 cd 而非命令本身**。用 `pnpm --dir <repo> <cmd>` 与
`cargo <cmd> --manifest-path <repo>/src-tauri/Cargo.toml` 绕开，别去改 shell 配置。

## 架构地图

### 后端（`src-tauri/src/`）

| 模块           | 职责                                                                                                                                                      |
| ------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `lib.rs`     | **命令/事件的唯一登记点**（`specta_builder()`）+ 启动装配。加命令必改这里。                                                                                                      |
| `state.rs`   | `AppState`：DB 池 · 密钥存储 · 数据目录 · 引擎。业务真相的持有者。                                                                                                            |
| `commands/`  | IPC 命令层（薄）：校验入参 → 调 repo/引擎 → 组视图结构体。按域分文件。                                                                                                             |
| `db/`        | `migrations/` forward-only + `repo/` 薄 SQL 封装。**业务规则不在 repo**。                                                                                          |
| `engine/`    | 任务引擎：`dispatcher`(单循环 + per-Key Semaphore) · `status`(7 态机) · `classify`(错误六类) · `progress`(伪进度) · `recovery` · `events`(EventSink 抽象，故引擎可脱离 Tauri 测试)。 |
| `provider/`  | 生图 Provider 抽象；唯一实现 `openai`（`POST {base}/images/edits`）+ `sanitize`(元数据/C2PA 剥离)。                                                                      |
| `publish/`   | 发布与资产管理（最大子系统）：`paths`(RelPath) · `platform`(五平台单点) · `inbox/` · `planner/` · `xlsx/` · `exporter` · `reconcile` · `ticker`。                            |
| `v2v/`       | 图生视频（状态全在库内）：`dreamina`(即梦 CLI 封装) · `handoff`(交接目录往返) · `runner`(提交/轮询/幽灵判定) · `autofill`(常驻待发队列) · `activity`(执行日志)。                                  |
| `intake/`    | 工单收件：`mod`(结构 + 校验 + 参数归一化) · `ingest`(收录 + 去重 + 移档) · `watcher`。                                                                                       |
| `secrets.rs` | API Key 本地加密文件存储（XChaCha20-Poly1305）。                                                                                                                   |
| `files/`     | 数据目录 · 缩略图 · 命名 · 废纸篓文件搬运。`ids/` 号池，`importer/` txt 解析。                                                                                                 |

`purpose.rs` 与 `publish/platform.rs` 是同一种模式：受控取值的单点定义，在**命令边界**强制
校验而不只靠 UI 给选择器——命令是公开边界，放进自由字符串就会有三种拼法同时进库。

### 前端（`src/`）

`features/<页面>/` 一页一文件（普遍 600–1900 行，就地展开而非过度拆组件）·
`routes.tsx` 是**路由/侧栏/⌘K/快捷键的单一来源** · `stores/` Zustand 只做事件镜像与 UI 态 ·
`lib/ipc/` 唯一 IPC 出入口 · `styles/globals.css` 唯一 token 来源。

`shortcut: number | null` —— 十个数字已用尽，新页无数字快捷键。**不为新页重排既有数字**
（会把肌肉记忆一次性作废）；⌘5 属于已移除的提示词库页，**留空不复用**。

### 一条生图请求的完整链路（改生成相关代码前先读懂这条）

```
GeneratePage 选组/挂靠 → commands::batches::create_batch
  → engine::create_batch（同事务：展开组合 × 抽卡 → tasks + batch_refs + 归档本批组与图）
  → Scheduler 循环取 q 态 → per-Key Semaphore 限并发 → provider.generate(multipart)
  → 落盘 outputs/ + 缩略图 → 状态迁移 rev（待验收）
  → EventSink 推 task://status-changed · task://progress · batch://summary（250ms 节流）
  → stores/engine 镜像 → 任务页/导航徽章
```

## 架构铁律（均有机器检查）

1. **业务真相只在 Rust**：任务状态、编号发放、文件操作、DB 读写全部经 Rust 命令；
   前端不持有可变业务状态。
2. **单写者事务**：所有状态迁移由调度器串行提交；配合 single-instance 禁双开。
3. **前端只经 `src/lib/ipc/` 出入** → guardrails 检查 `invoke(`/`listen(` 仅限该目录。
4. **事件驱动不轮询**，250ms 节流。唯一例外是即梦 CLI（它没有任何推送机制，见下文）。
5. **token 只从 `globals.css` 取** → guardrails 检查 `oklch(` 仅限该文件；
   `classnames.test.ts` 另检查**用到的 class / `var(--x)` 必须真的存在**、
   **定义了的 class 必须有人用**、**不写 Tailwind 工具类**（本仓库全用自定义类）。

## IPC 约定

- `src/lib/ipc/bindings.ts` 由 **tauri-specta 自动生成，禁手改**。改动命令/事件后须在
  `lib.rs` 的 `specta_builder()` 登记 → `cargo test --lib export_bindings` 重新生成 →
  **先 `git add` 再跑 `pnpm check`**（门禁第 8 步用 `git diff --exit-code` 校验同步）。
- 前端只 import `@/lib/ipc`，不直接用 Tauri API 做业务调用。
- 载荷字段 camelCase 由 specta 序列化配置统一保证，不手写 TS 类型。

## 错误处理与视觉

- 统一 `thiserror`（`error.rs`）经 IPC 序列化给前端。业务错误六类：
  Timeout / RateLimited / ContentPolicy / Auth / Interrupted / Other（`engine/classify.rs`；
  Interrupted 是引擎内部态，不来自 provider）。
- **非测试代码禁 `unwrap` / `expect` / `panic`**（Cargo `[lints.clippy]` deny），
  另有 `unsafe_code = "forbid"`。测试内须以带说明注释的 `#[allow(...)]` 局部放开。
- 前端错误经 `reportFrontendError` → `log_frontend_error` 汇入 tracing；guardrails 禁 `console.log`。
- **打包应用没有终端**：启动期致命错误必须弹原生对话框（`fatal_dialog`，用 rfd），
  否则表现只是「双击图标 dock 弹一下就没了」。
- **7 态 → 5 视觉组**：`q`=灰 · `run`+`retry`=蓝(spinner) · `fail`=红 · `rev`(待验收)=琥珀 ·
  `pass`=绿 / `rej`=灰。（枚举与 0001 的 CHECK 都是 7 个，早期文档里的「八态」是笔误。）

## 领域词汇

望文生义会出错的那些。

**生图** — **挂靠**：每张参考图指定一个提示词组，任务数 = Σ(参考图 × 其挂靠组提示词数)，
非笛卡尔积 · **归档(archived\_at)**：只决定生成页选择器是否默认列出，**与删除无关** ·
**批次退休**：任务全落 pass/rej 且无本批结果滞留废纸篓时物理删批次与本批提示词（提示词是
消耗品），故下游必须先存快照（`accepted_works.prompt_code`/`group_name`），编号**不回收** ·
**临时分组(is\_temp)**：导入 txt 产生，任一提示词首次验收通过则整组转正式 ·
**临时上传(ephemeral)**：生成页上传的参考图，仍是 ref\_images 行但图库/选择器/去重基准三处
都不含它（`list_ref_images(include_ephemeral)` 单点控制）· **图库分组(ref\_groups)**：与提示词
组无关，历史列 `ref_images.group_id` 已废弃不读不写 · **用途(purpose)**：标在**提示词组**上
（批次会混组），是筛选默认值**不是门禁** · **号池**：编号递增发放回收优先，与业务写同事务 ·
**废纸篓**：暂存不物理删，**可还原回原位**（作品是唯一「删除即真删行」的实体，故
`trash_items.payload_json` 存整行快照，还原连 id 一起写回）· **伪进度**：生图 API 无真实进度。

**v2v** — **clip**：一张验收图的**一次**视频尝试，七态与 `tasks.status` 同构，`UNIQUE(work_id)`
故重跑是就地 `attempt+1` 不新增行 · **交接目录**：GenDesk 自动物化工单，skill 写回后 watcher
收录移档；组目录名对同一组恒定（`g{group_id}`），且**只监听「已改写」**（连「待改写」一起监听
会形成物化→收录→物化的自激循环）· **skill 边界**：只改写提示词，**不调 dreamina** ·
**幽灵单**：即梦给了 submit\_id 但 `queue_idx` 与 `credit_count` 双双缺席、从未计费——与超时
**处置相反**（超时「已扣费 → 继续等待」，幽灵「没扣费 → 直接重跑」），结论由 Rust 下发
（`clip_looks_phantom`），前端不得手抄判据 · **本地队列 vs 即梦队列**：两个位次**绝不混成一个
数字**（本地第 3 vs 即梦第 4485）· **封面**：clip 自己的文件副本，绝不指向
`accepted_works.thumb_path`（清废纸篓会物理删 file\_paths）· **交付**：验收通过即**拷贝**（不移动）
成片到 `outputs/视频/`，拷贝失败**不回滚验收**，故 `undelivered` 是合法状态兼侧栏徽章。

**发布** — **SKU**：款式一级分类，下挂素材/标题/正文三池 · **素材包**：视频型=1 视频(+封面)，
图集型=N 图(+封面)，**入库即 active**；「已用尽/冷却中」是台账派生态不落库 · **日内容套装**：
某天某 SKU 选定的（素材包+标题\[+正文]），当天全平台全账号统一 · **任务单**：一天一张，
草稿→已确认→已导出→回收中→已关闭 · **任务包**：xlsx(22 列) + 素材 + 说明 + 回执截图/ +
**最后写的 READY.txt** · **回执**：执行器只写 xlsx 第 20–22 列，故**重导出会抹掉回执**（导出预检
里有保护）· **查重窗口**：同素材包同平台最短复用间隔（默认 30 天），**按平台逐个剔除** ·
**待认领**：收件箱关联不上 SKU 的内容，人工指认不丢弃 · **疑似已发**：超时无回执，
**绝不自动重发** · **RelPath**：库内只存根目录内相对路径，导出是唯一绝对路径转换点 ·
**五平台**：`douyin/xhs/kuaishou/shipinhao/bilibili`，映射单点在 `publish/platform.rs` ·
时刻一律**按本地时区**解析（`and_utc()` 会把北京时间当 UTC）。

**intake** — **工单**：一个目录 = 一次投单，`提示词.txt` + `images/` + **最后写的 READY.txt**
（没有它一律不碰，skill 可能还在写）· 方向与 v2v **相反**（skill 出工单、GenDesk 收），
两者共用 `publish::inbox::watcher::coalesce` · **组头键**（`参考图:`/`比例:`/`尺寸:`/`格式:`/
`抽卡:`/`用途:`）**只在该组第一条正文之前生效**，故挂靠是**位置绑定**，改组名不会让它断掉；
正文是长叙事，「比例：3:4 的竖构图」不该被当成元信息吃掉 · **挂靠不猜**：多组工单每组必须
写 `参考图:` · **一份工单 → 多个批次**：`params_json`/`draws` 是批次级的，按 (参数, 抽卡) 分桶 ·
**job\_id 去重**：收录恰好一次，磁盘标记不可靠而重复收录 = 重复花钱；记账在**动手之前** ·
**阈值(默认 100 张)**：超了转 `hold` **什么都不导入** + 弹应用内可视化确认卡 ·
**归一化只改拼法不改取值**（`jpg→jpeg` 是拼法；边长非 16 倍数是取值 → **拒单**，
静默改值正是「我明明写了 9:16 却不生效」的成因）· **写路径只有一条**：提示词走
`commands::prompts::{build_preview_from_parsed, commit_preview}`，参考图走
`commands::refs::ingest_one`，与手动导入同一套前缀分配/用途判定/缩略图口径 ·
**失败回执必须写明已导入到哪一步**（这串动作没有一步能整体回滚，谎称「没有导入任何东西」
会让人点重试拿到第二份提示词）· skill 装在用户级 `~/.claude/skills/`。

## 高代价陷阱（改代码前必读）

实测得出、代价不对称、且从代码结构上看不出来的事实。

**生图参数** — 进 multipart 的只有三个（`provider::GenParams`）：`aspect_ratio` / `size` /
`output_format`，外加恒定 `n=1`。**画幅两个字段都要发**：实测单发 `aspectRatio: "9:16"` 回来
整批是 1024×1024 正方形，单发 `size: "1080x1920"` 则整批 400（边长非 16 倍数）；配套值单点
`provider::RATIO_SIZES`（同时满足「正好是该比例」与「两边都是 16 的倍数」）。**回来的像素不由
我们定**（发 `1088*1920` 上游给 941×1672），这些值是**比例的载体**不是交付分辨率。
**抽卡 k 次 = k 个任务**，不是发 `n=k`（一次响应里 k 张只有一张能落进当前任务，其余照样计费）。

**并发认领** — 三处窗口，代价都是钱。`claim_ready`（ready → run + 检查 `rows_affected`）
必须在提交**之前**，`UNIQUE(work_id)` 拦不住双提交（自始至终只有一行，第二次只是覆盖
submit\_id）。`mark_running` 带 `AND status = ?` 谓词并返回是否认领成功，0 行不 spawn worker；
谓词用调用方读到的**状态原文**而非写死 `'q'`（重试任务走 `retry → run`）。
单 Key 并发上限单点 `db::repo::api_keys::MAX_CONCURRENCY`，写入侧与引擎 Semaphore 容量
都必须引用它（分叉过一次，症状是设置页与 DB 都显示 50、只有真正跑的信号量是 10）。

**额度** — 提交成功即写 submit\_id 并置 run；顺序反了会留下认不出主人的孤儿，而恢复只能退回
重提 = 花两份钱。中断恢复**只**动无 submit\_id 的条目；回执异常也照收，判死统一交给轮询。
即梦并发上限是**账户级**的：实测非 VIP 同时只跑得下 1 条，超出回 `ret=1310
ExceedConcurrencyLimit` —— 那**不是失败**（一分钱没扣），走 `requeue_after_reject` 放回本地队首
且 `attempt` 退回去；`OBSERVED_LIMIT` 进程内自收敛**不落库**。VIP 同规格**贵 5.5 倍**（8 vs 44
额度）且只买到不排队，故默认模型必须显式，判定单点 `dreamina::is_vip`。常驻队列 autofill 四道
闸都是机制：默认关 · 模型必须非 VIP（保存那一刻就拒）· 日额度按**提交**时刻切窗（用出片时刻
切的话，补单器能在任何一条出片之前把一天额度提交光）· 余额兜底。

**轮询（铁律 4 的唯一例外）** — 实跑确认即梦 CLI 无任何推送机制（`--poll=N` 只是把轮询搬进
子进程，进程被杀即丢）。单位是**一整页**不是一条：`list_task` 一个进程回全部在跑任务的状态，
进程数与在跑条数**脱钩**。故频率是纯粹的成本旋钮（含 VIP 300s / 全非 VIP 600s，
`SWEEP_VIP_SECS`）；逐条退避是回落路径，下限抬到同一常数——它是 O(n) 的，没道理比整表还勤。
`list_task` 缺两样东西各自决定一段代码：无 `videos[].path`（出片仍要单发 `query_result --download_dir`）· 无 `queue_info`（故幽灵判定拆宽判据 `phantom_suspect` / 权威回体
`is_phantom`，**确认查询失败就这一轮不判**——问不出话 ≠ 判死）。未知 `gen_status` 判 Running
而非 Failed（判死会把已扣费正在跑的任务标死）。计费证据一律 `COALESCE` 写回，**只增不抹**。

**事务粒度** — `accept_tasks` 明确**不做单事务**（与直觉相反）：拷贝无法回滚，第 150 张失败会把
前 149 条作品记录回滚掉而文件已在 outputs/ 里；现在的顺序（先整批拷完、任一张失败就一行库都
不写）反而更接近原子。

**迁移** — `src-tauri/migrations/` **forward-only，发布后不可改**（`db/repo/mod.rs` 里「重放
0019 搬运语句」那条测试正建立在这个前提上）。废弃列一律**保留不读不写**。默认留在事务内，
只有必须 `PRAGMA foreign_keys=OFF` 的表重建才用 `-- no-transaction`。改动建议先在**真实库副本**
上跑 `foreign_key_check` + `integrity_check`。`tag_bindings` 是无外键的多态表，删分组时必须手动
清（分组 id 被复用时旧绑定会把用途安到无关的组头上）。

**dev 与打包应用共用同一个 `app_data_dir`**，而 `sqlx::migrate!` 在**编译期**内嵌迁移。所以
`pnpm tauri dev` 跑在新 main 上会把库迁到最新版，装在 `/Applications` 的旧包**从此再也开不
起来**（`VersionMissing`，这是保护不是故障）。排查看 `latest_embedded_migration()` 而非
`CARGO_PKG_VERSION`。**另一条通往同样症状的路径**：single-instance 插件在已有实例时静默
exit(0)——区分方法是看日志有没有当次的 `logging initialized`（单实例踢出发生在日志初始化之前，
一个字都不留；迁移失败则必留一条 ERROR）。

**跑 GUI 之前**：`pnpm tauri dev` 可能**真花钱** —— intake 默认 `enabled: true`，启动会扫描
收件目录、自动收录工单并建批。先把待收工单挪开，或在设置里关掉收件。

## 门禁与协作

`pnpm check` 9 步（guardrails · Biome ci · tsc strict · vitest · vite build · cargo fmt ·
clippy -D warnings · cargo test 含 bindings 同步 · cargo check），与
`.github/workflows/check.yml` 互为镜像；`lefthook.yml` 是它的秒级子集，装了 pre-commit
也仍需在提交前跑全门禁。CI 另有覆盖率闸门（engine / ids / importer / publish 纯逻辑
≥ 85% 行，IO/glue 经 `--ignore-filename-regex` 排除）与每周 `cargo audit`。

sqlx 用运行时校验查询（`query`/`query_as`），SQL 由针对临时库的 `cargo test` 集成测试覆盖
（比仅编译期检查更强），故未接入 `cargo sqlx prepare --check`。

前端测试只覆盖**派生逻辑与机械规则**（`routes.test.ts` · `stores/settings.test.ts` ·
`features/review/layout.ts` · `features/v2v/model.ts` · `styles/classnames.test.ts`）——
业务真相在 Rust，测试也在 Rust，别为了补前端覆盖率去测 UI 壳。

- **实现与审查分离**：实现完成后由**全新上下文**会话执行 `/code-review`（引擎/数据层用 high）。
  同一上下文自审会继承同样盲点。发现项修复后才 merge。
- **测试完整性规则**：**禁止**为让 CI 变绿而修改/删除/放宽既有测试断言。确需改动（例如某条
  断言的前提被实测推翻）须在测试注释与 PR 描述里写明理由。
- 一个任务 = 一个分支 = 实现 + 测试 + 门禁全绿 + 审查通过 = 一次 merge。main 受保护，
  即使单人开发也走 PR。
- 发版流程与更新签名密钥见 README.md。

参考物：`docs/prototype/*.dc.html`（像素基准）· `docs/prototype/Design-Tokens.md` ·
`docs/V2-backlog.md`（含**交付前人工收尾清单**）· `docs/收件箱收录格式规范.md` ·
`docs/提示词skill标准产出规范.md`。

**版本演进史不在本文件里**，去 `git log`（提交信息与代码注释保留了实测数据与取舍理由；
早期 UX 条目编号可用 `git log --grep=E30b` 定位）。本文件只记**当前仍然成立的约束**。
