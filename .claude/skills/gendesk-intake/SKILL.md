---
name: gendesk-intake
description: 将提示词文件与有序参考图准备为 GenDesk CLI 清单，检查并在已有生成授权内投递，回读收录状态。用于用户要求送去 GenDesk、跑图或续做已授权批次；只要提示词时仅准备文件和预检，不提交生图。
---

# GenDesk 生图投单

本机 CLI 为 ~/.local/bin/gendesk。调用方负责提示词、参考图配对与内容归属；应用负责密钥、收录、调度、生成、重试、人工验收和成品交付。不要直接读写 SQLite 或代替人工验收。

## 接入规范

投单前读取 [命令行生图接入](/Users/indincys/Workspace/软件开发/GenDesk/代码/docs/命令行生图接入.md)，这是唯一完整字段契约，适用 CLI / 应用 0.38.0、控制协议 2、清单 version=1。skill 写作交付边界见 [提示词标准产出](/Users/indincys/Workspace/软件开发/GenDesk/代码/docs/提示词skill标准产出规范.md)。不要从旧包、历史示例或文件名推断归属和格式，也不要把字段表复制进其他 skill。

## 准备与预检

- 每张图一份非空 UTF-8 纯正文 TXT，清单逐项填写 name、promptFile、ref 或有序 refs。参数与附件路径放 JSON，正文说明 Image 1、Image 2 的角色；多参考图不展开任务、不拼接触表。
- 新任务填写 organization 的产品、款式、固定需求、来源。条目整对象覆盖顶层；非商品任务无需 SKU，sku/style 只在已有商品映射需要时填写真实值。需求名不能用 jobId 或本次运行日期代替。
- 参考图已在 GenDesk 参考图库时，ref/refs 写分类引用（如 `{"category":"正面图"}`）由应用收录时随机抽图并跳过近期用过的图，不必导出本地文件；写法见规范「参考图库」一节，返回的 libraryPicks 告诉你抽到了哪张。
- 提示词、人读说明、清单写入当前项目约定的 _草稿；源参考图保持原位。实际附件须有可访问的本地路径，不能拿 URL、对话图片标记或虚构路径代替。
- gendesk --version 查看 CLI，gendesk doctor 查看应用是否在线；离线不要紧，submit 会在后台启动应用。
- 执行 gendesk validate --manifest <清单路径>。它调用应用的 Rust 只读完整预检，校验参数、归属与实际图片解码；不写库、不投递、不生图。

## 投递与恢复

仅在已有生成授权内执行 gendesk submit --manifest <清单路径>。CLI 复制文件、按内容去重，并经本机控制通道请求应用收录；**返回时收录已完成**，不需要等待或轮询。不要手动往 ~/GenDesk交接 写文件。

按返回 JSON 的 status 处理并向用户报告 jobId、taskCount：

- ingested：已建批开跑，提醒用户在 GenDesk 审图页验收；gendesk overview 可看还剩多少在跑、多少待审。
- hold：超过阈值未导入。向用户展示条数与配对，得到明确同意后才 gendesk confirm <jobId>，不调整阈值绕过。
- error：读 job.message 了解停在哪一步，修正后 gendesk retry <jobId>，按检查点只补未完成步骤；已有导入后不得修改原内容。
- 状态不明或命令中断：gendesk status <原jobId>，不自动换 ID 重投或重跑成功项。确实新增一次生成时才在授权内使用新的 jobId。

用户只要提示词时停在文件交付和预检，不 submit。
