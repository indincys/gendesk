# GenDesk

修改代码前阅读 [CLAUDE.md](CLAUDE.md)，遵守 Rust 业务边界、单写者、类型化 IPC 和测试约束。

## 外部 harness 生图

- 完整接入契约只维护在 [命令行生图接入](docs/命令行生图接入.md)，适用 CLI / 应用 0.36.1、工单 version=1；字段、示例、参数与恢复以此为准。提示词 skill 交付边界见 [标准产出](docs/提示词skill标准产出规范.md)。不要从历史文档复制旧规则。
- 外部 Codex、DSH、Claude Code 直接使用 gendesk，无需 computer use。先 gendesk --version / doctor；macOS/Unix 更新本机命令用 pnpm install:cli，应用升级不等于 CLI 已同步。
- 新任务用 organization 声明产品、款式、固定需求和来源；条目整对象覆盖顶层。无 SKU 不编造编号，旧 sku/style 格式继续兼容。同一需求续做不以运行批次改名。
- 一条 UTF-8 正文、一组有序 ref 或 refs、一个任务、一张图。多参考图不拼接触表，不丢环境参考；附件顺序与 Image 1、Image 2 一致。默认 quality=high，比例/尺寸写参数，不只写正文。
- 先生成显式配对清单并 gendesk validate --manifest <清单>；只有已授权生图时才 submit --manifest <清单> --open --wait 30。只写提示词不投单。validate 复用 Rust 只读完整预检（参数、归属、实际图片解码），不写库或投递。新单的应用内重试按检查点只补未完成部分，已有导入后不得改内容；无检查点的历史部分导入或中断单拒绝盲目重放。
- --wait 只等收录，结果在 GenDesk 人工验收。超时/失败保留原 jobId，先 status 查询，不自动换 ID 重投；超阈值需用户明确确认。
- CLI 校验用 pnpm test:cli，含版本一致性、安装副本、新 organization 与旧格式兼容；提交前全门禁 pnpm check。
