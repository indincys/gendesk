# GenDesk · 内部图片生产工具

本地批量图生图流水线：参考图素材库 → 提示词分组 → 批量生成 → 进度追踪 → 人工验收 → 合格图输出归档。
Windows 10/11（x64）与 macOS 12+（Apple Silicon）双端，Tauri 2 + React 19 + Rust。**不支持 Intel 芯片 Mac。**

## 开发

```bash
pnpm install
pnpm tauri dev          # 启动桌面应用（前端 + Rust）
pnpm check              # 全门禁（提交前必跑，= CI 镜像）
```

架构约定与铁律见 [CLAUDE.md](CLAUDE.md)。

## 审稿图片进入 Poster

Poster 接入默认关闭；在设置中开启后，审稿通过才逐批询问是否导入，确认后按已有商品 ID 静默进入图片库，不自动发布。
设置、20 款归属与失败恢复见 [Poster 图片接入](docs/Poster图片接入.md)。Poster 侧为任何工具都可调用的通用图片 CLI，无生成渠道专属规则。

## Harness / 命令行投单

CLI 0.36.1 接收 UTF-8 正文与本地有序参考图，打包投递并返回 JSON；新任务用 organization 按产品、款式、需求归档，无 SKU 不编造编号。旧 sku/style 单仍兼容。

```bash
pnpm install:cli          # macOS/Unix：更新 ~/.local/bin/gendesk 的独立副本
gendesk --version
gendesk doctor
gendesk validate --manifest /路径/场景任务.json
gendesk submit --manifest /路径/场景任务.json --open --wait 30
```

应用升级不会自动同步 CLI；安装器检查版本并原子替换入口。完整字段、单条调用、多参考图、参数继承、稳定去重和回执只维护在 [命令行生图接入](docs/命令行生图接入.md)。validate 通过 Rust 做只读完整预检；新工单失败按检查点恢复，不重复导入已有记录。只写提示词不投单；--wait 只等收录，生成结果在应用中人工验收。skill 交付边界见 [标准产出](docs/提示词skill标准产出规范.md)。

## 发布（M4）

发版 = 改 `src-tauri/tauri.conf.json` 的 `version` + 打 tag `vX.Y.Z` 并推送，其余由
`.github/workflows/release.yml` 自动完成（双端构建、minisign 签名、上传产物与 `latest.json`）。

### 更新签名密钥（重要）

- 应用内更新用 minisign 签名校验。**公钥**已写入 `tauri.conf.json` 的 `plugins.updater.pubkey`。
- **私钥**由本地生成，已备份到 `~/.tauri/gendesk-signing.key`（空密码）。
  - 上传到发布仓库 GitHub Secrets：`TAURI_SIGNING_PRIVATE_KEY`（私钥文件内容）、
    `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`（本例为空字符串）。
  - **另做一份离线备份**：私钥丢失 = 更新链路永久断裂（已发布用户无法再收到更新）。
- 更新 endpoint 指向公开发布仓库（`tauri.conf.json` 的 `plugins.updater.endpoints`，
  当前为 `indincys/gendesk`，该仓库为公开仓库以便匿名访问）。

## 首次安装引导（未签名分发）

- **Windows**：NSIS per-user 安装（免管理员）。首次运行可能触发 SmartScreen：
  「更多信息 → 仍要运行」，仅首次。WebView2 缺失时安装器自动下载。
- **macOS**：拖入「应用程序」后启动。macOS 15+ 首次需「系统设置 → 隐私与安全性 → 仍要打开」，
  仅首次；后续版本经应用内更新替换，不再触发。

## 版本演进

**逐版本记录与设计取舍见 `git log`**（提交信息与代码注释里保留了实测数据与取舍理由）；
[CLAUDE.md](CLAUDE.md) 只记当前仍然成立的架构约束。


0.36.0 起，图片和视频按产品、款式、需求持续归档，不再按验收或运行批次创建成品目录。产品规则不写死在工具里；人工确认卡、CLI 与工单都能提供归属。当前本机作品根为项目根（与 `代码/` 分开），存量整理、快照及外部引用约定见 [作品目录整理](docs/作品目录整理.md)。
