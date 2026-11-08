---
name: gendesk-intake
description: 将提示词文件与有序参考图准备为 GenDesk CLI 清单，检查并在已有生成授权内投递，回读收录状态。用于用户要求送去 GenDesk、跑图或续做已授权批次；只要提示词时仅准备文件和预检，不提交生图。
---

# GenDesk 生图投单

本机 CLI 为 ~/.local/bin/gendesk。调用方负责提示词、参考图配对与内容归属；应用负责密钥、收录、调度、生成、重试、人工验收和成品交付。不要直接读写 SQLite 或代替人工验收。

## 接入规范

投单前读取 [命令行生图接入](/Users/indincys/Workspace/软件开发/GenDesk/代码/docs/命令行生图接入.md)，这是唯一完整字段契约，适用 CLI / 应用 0.36.1、工单 version=1。skill 写作交付边界见 [提示词标准产出](/Users/indincys/Workspace/软件开发/GenDesk/代码/docs/提示词skill标准产出规范.md)。不要从旧包、历史示例或文件名推断归属和格式，也不要把字段表复制进其他 skill。

## 准备与预检

- 每张图一份非空 UTF-8 纯正文 TXT，清单逐项填写 name、promptFile、ref 或有序 refs。参数与附件路径放 JSON，正文说明 Image 1、Image 2 的角色；多参考图不展开任务、不拼接触表。
- 新任务填写 organization 的产品、款式、固定需求、来源。条目整对象覆盖顶层；非商品任务无需 SKU，旧 sku/style 仅为兼容或已有商品映射使用。需求名不能用 jobId 或本次运行日期代替。
- 提示词、人读说明、清单写入当前项目约定的 _草稿；源参考图保持原位。实际附件须有可访问的本地路径，不能拿 URL、对话图片标记或虚构路径代替。
- 使用 gendesk --version 查看 CLI，doctor 检查应用连接快照和能力；快照不代表在线。缺少能力时保留文件，先查明版本或入口，不修改快照冒充支持。
- 执行 gendesk validate --manifest <清单路径>。它调用 Rust 只读完整预检，校验参数、归属与实际图片解码；不启动 GUI、不写库、不投递、不生图。缺少新版预检能力时保留输入并说明原因，不降级为文件预检。

## 投递与恢复

仅在已有生成授权内执行 gendesk submit --manifest <清单路径> --open --wait 30。--open 在 macOS 按需后台启动应用，--wait 只等待收录。CLI 自动复制、写 READY、按内容去重并查找移档回执，不再手写分钟级工单目录。

回读 JSON，报告 jobId、状态及回执里的 batchIds / taskCount。ingested 不是生图或验收完成，结果去 GenDesk 查看。用户只要提示词时停在文件交付和预检，不创建 READY。

超时、error、staging 或状态不明先 gendesk status <原jobId>，按回执核对导入阶段与数量，不自动换 ID 重投或重跑成功项。0.36.1 新工单的应用内“重试”复用身份检查点，只补未完成步骤，保留已有提示词与参考图 ID；已有导入后不得修改原内容。无检查点的历史部分导入或 running 工单不会盲目重放，保留回执核对遗留。确实新增一次生成时才在授权内使用新的 jobId。

超过应用阈值后向用户展示条数和配对，得到明确同意才在应用放行或写确认.txt，不调整阈值绕过。无 CLI 时的手动工单仅按接入规范的兼容章节执行。
