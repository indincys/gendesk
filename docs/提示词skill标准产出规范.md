# 提示词 skill 标准产出规范

适用 GenDesk 0.38.0。完整字段、示例、参数表、回执与恢复规则统一维护在 [命令行生图接入](命令行生图接入.md)，本文件只规定提示词 skill 的交付边界，不再复制整份接口规范。

## 产出

- 每张图一份 UTF-8 纯正文 TXT；批量另写 JSON 清单，显式关联 promptFile 和 ref 或有序 refs。正文不混路径说明、SKU、标题或分析。
- 清单顶层或条目填写 organization（产品、款式、需求、来源），同需求持续沿用同名；没有 SKU 不编造编号。条目归属整对象覆盖顶层，完整规则读接入规范。
- 生图参数写清单，默认 quality=high；比例和尺寸不能只写正文。多参考图顺序与 Image 1、Image 2 的描述一致，不合成接触表。
- 按当前项目文件管理约定将提示词、人读说明和清单放入 _草稿；原素材只引用，不搬动。不要手动往 `~/GenDesk交接` 里写任何东西，投单只经 gendesk submit。

## 收尾

用户只要求写词、研究或本地检查时，交付文件并可运行 validate，**不投单**。用户要求生图或明确授权续做时，先检查 gendesk --version / doctor，再执行：

```bash
gendesk validate --manifest /绝对路径/场景任务.json
gendesk submit --manifest /绝对路径/场景任务.json
gendesk status cli-返回的工单编号
```

validate 复用应用 Rust 只读入口，完整校验文件、正文、参数、归属与实际图片解码，不投递、不生图。submit 返回时已收录完毕：ingested 只表示已建批开跑，生成结果在 GenDesk 人工审图；hold 是超阈值待确认，向用户说明张数并取得同意后才 gendesk confirm；error 读原因后 gendesk retry，沿原身份检查点只补未完成步骤，已有导入后不得修改原内容。状态不明用原 jobId 查 status，不换随机 ID 重投。

新写或更新 prompts-* skill 时引用本文件和接入规范，**不再抄一份打包脚本或字段表**，避免下一次升级留下多份冲突规则。skill 前缀不代表生成授权。
