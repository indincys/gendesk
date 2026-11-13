---
name: gendesk-intake
description: 将提示词文件与有序参考图准备为 GenDesk CLI 清单，检查并在已有生成授权内投递，回读收录状态；也把 Codex 等外部已生成的图连同提示词与参考图导入 GenDesk 待验收。用于用户要求送去 GenDesk、跑图、续做已授权批次，或把外部成图交给 GenDesk 验收；只要提示词时仅准备文件和预检，不提交生图。
---

# GenDesk 生图投单

本机 CLI 为 ~/.local/bin/gendesk。调用方负责提示词、参考图配对与内容归属；应用负责密钥、收录、调度、生成、重试、人工验收和成品交付。不要直接读写 SQLite 或代替人工验收。

## 接入规范

投单前读取 [命令行生图接入](/Users/indincys/Workspace/软件开发/GenDesk/代码/docs/命令行生图接入.md)，这是唯一完整字段契约，适用 CLI / 应用 0.38.0、控制协议 2、清单 version=1。skill 写作交付边界见 [提示词标准产出](/Users/indincys/Workspace/软件开发/GenDesk/代码/docs/提示词skill标准产出规范.md)。不要从旧包、历史示例或文件名推断归属和格式，也不要把字段表复制进其他 skill。

## 准备与预检

- 每张图一份非空 UTF-8 纯正文 TXT，清单逐项填写 name、promptFile、ref 或有序 refs。参数与附件路径放 JSON，正文说明 Image 1、Image 2 的角色；多参考图不展开任务、不拼接触表。
- 新任务填写 organization 的产品、款式、固定需求、来源。条目整对象覆盖顶层；非商品任务无需 SKU，sku/style 只在已有商品映射需要时填写真实值。需求名不能用 jobId 或本次运行日期代替。产品、款式与 GenDesk 参考图库里的名字逐字一致（gendesk library overview 可查），Poster 归属也按它们匹配。
- 审稿通过的图进 Poster 商品的哪个库位按需求名自动对应：需求「封面」进封面库，与 Poster 图片类型同名（如「正视图」「礼盒图」）进该类型，其余（如场景图）进未分类。需求名与类型不同名、或要强制封面 / 未分类时，在 organization 写 posterType（类型名、`封面` 或 `未分类`）；拿不准先 gendesk poster resolve --product … --style … --requirement … [--poster-type …] 看会进哪里和 Poster 现有类型，不为凑类型改需求名。
- 参考图已在 GenDesk 参考图库时，ref/refs 写分类引用（如 `{"category":"正面图"}`）由应用收录时随机抽图并跳过近期用过的图，不必导出本地文件；写法见规范「参考图库」一节，返回的 libraryPicks 告诉你抽到了哪张。一类要多张写 `{"category":"正面图","count":2}`；冷却期内图不够时应用自动降级冷却天数，无需你处理。要把一组参考图交给外部执行时用 `gendesk library pack --export <目录>`（导出即记账，导出的文件投回时当普通文件，不改写成 libraryId）。
- 提示词、人读说明、清单写入当前项目约定的 _草稿；源参考图保持原位。实际附件须有可访问的本地路径，不能拿 URL、对话图片标记或虚构路径代替。
- gendesk --version 查看 CLI，gendesk doctor 查看应用是否在线；离线不要紧，submit 会在后台启动应用。
- 执行 gendesk validate --manifest <清单路径>。它调用应用的 Rust 只读完整预检，校验参数、归属与实际图片解码；不写库、不投递、不生图。

## 投递与恢复

仅在已有生成授权内执行 gendesk submit --manifest <清单路径>。CLI 复制文件、按内容去重，并经本机控制通道请求应用收录；**返回时收录已完成**，不需要等待或轮询。不要手动往 ~/GenDesk交接 写文件。

按返回 JSON 的 status 处理并向用户报告 jobId、taskCount：

- ingested：已建批开跑，提醒用户在 GenDesk 审图页验收；gendesk overview 可看还剩多少在跑、多少待审。
- hold：未导入，按 job.message 区分原因。超过阈值时向用户展示条数与配对，得到明确同意后才 gendesk confirm <jobId>，不调整阈值绕过。「Poster 款式对不上」时：款式名写错就改成参考图库的名字、换新 jobId 重投；新款式先向用户确认对应 Poster 商品，再 gendesk poster bind --product <产品> --style <款式> --target <商品名> 后 confirm；用户明确不进 Poster 才直接 confirm。「Poster 图片类型对不上」时：posterType 写错就改成 poster resolve 列出的类型（或去掉它按需求名对应）、换新 jobId 重投；确需新类型时请用户先在 Poster 建好再 confirm。
- error：读 job.message 了解停在哪一步，修正后 gendesk retry <jobId>，按检查点只补未完成步骤；已有导入后不得修改原内容。
- 状态不明或命令中断：gendesk status <原jobId>，不自动换 ID 重投或重跑成功项。确实新增一次生成时才在授权内使用新的 jobId。

用户只要提示词时停在文件交付和预检，不 submit。

## 外部成图进验收（Codex 等）

图已经在 GenDesk 之外生成好（如 Codex 生图）时，不要 submit（那会重新生图）。用同一份清单，每个条目多写 `result`（成图本地路径），先 gendesk validate --manifest <清单>，再 gendesk import --manifest <清单>；单张可用 gendesk import --result <成图> --prompt <正文> --ref <参考图> --product … --style … --requirement … --source Codex。规则见规范第 9 节：

- 每条都写 result，一单不混生图；promptFile 写生成时实际用的正文，ref/refs 写实际用的参考图（本地文件或 libraryId），不写 `{"category":…}` 随机抽；library pack 导出包里的图按普通文件写。
- organization 照常填写，决定成品目录与 Poster 归属。导入不调用生图、不花钱，不需要生成授权，也不受张数阈值限制；Poster 对不上时同样 hold，按上文处理。
- 返回 ingested 即图已在待验收（job.message 写明未调用生图），提醒用户在 GenDesk 审图页验收。CLI 报「不支持外部成图导入」时，让用户更新并重启 GenDesk，再 pnpm install:cli 同步命令。
