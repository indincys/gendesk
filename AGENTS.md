# GenDesk

修改代码前阅读 [CLAUDE.md](CLAUDE.md)，遵守 Rust 业务边界、单写者、类型化 IPC 和测试约束。一个任务一个分支，`pnpm check` 全绿后直接合入 main 并推送，不走 PR。新迁移默认只做加法，破坏性改动要抬高 `reader_floor`（见 CLAUDE.md「启动升级保护」）。

## 外部 Agent 生图

- 完整接入契约只维护在 [命令行生图接入](docs/命令行生图接入.md)，适用 CLI / 应用 0.38.0、控制协议 2、清单 version=1；字段、示例、参数与恢复以此为准。提示词 skill 交付边界见 [标准产出](docs/提示词skill标准产出规范.md)。不要从历史文档复制旧规则。
- Agent 只用 gendesk 命令，不写交接目录、不读写 SQLite、无需 computer use。先 gendesk --version / doctor；macOS/Unix 更新本机命令用 pnpm install:cli，应用升级不等于 CLI 已同步。
- 新任务用 organization 声明产品、款式、固定需求和来源；条目整对象覆盖顶层。无 SKU 不编造编号。同一需求续做不以运行批次改名。
- 一条 UTF-8 正文、一组有序 ref 或 refs、一个任务、一张图。多参考图不拼接触表，不丢环境参考；附件顺序与 Image 1、Image 2 一致。默认 quality=high，比例/尺寸写参数，不只写正文。
- 先生成显式配对清单并 gendesk validate --manifest <清单>；只有已授权生图时才 gendesk submit --manifest <清单>。只写提示词不投单。submit 返回时已收录完毕：hold 先向用户说明张数并取得同意再 gendesk confirm；error 读原因后 gendesk retry（按检查点只补未完成部分）；状态不明用原 jobId 查 status，不换 ID 重投。
- 参考图库：款式 → 分类长期存图，`gendesk library …` 管理；清单 ref/refs 写 `{"category":"正面图"}` 由应用在收录时随机抽图，冷却期内用过的图优先跳过（见规范第 8 节）。
- 图生视频改写：gendesk v2v pending 取条目，写好后 gendesk v2v rewrite --file <结果>；不调用 dreamina。
- CLI 校验用 pnpm test:cli；提交前全门禁 pnpm check。
