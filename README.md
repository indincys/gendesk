# GenDesk · 内部图片生产工具

本地批量图生图流水线：Agent 经 CLI 投单 → 批量生成 → 进度追踪 → 人工审图 → 合格图归档，可选自动转实况图并导入 Poster。
Windows 10/11（x64）与 macOS 12+（Apple Silicon）双端，Tauri 2 + React 19 + Rust。**不支持 Intel 芯片 Mac。**

## 开发

```bash
pnpm install
pnpm tauri dev          # 启动桌面应用（前端 + Rust）
pnpm check              # 全门禁（提交前必跑，= CI 镜像）
```

架构约定与铁律见 [CLAUDE.md](CLAUDE.md)。

## 审稿后自动化

设置 › 审稿后自动化有两个开关，都默认关闭、打开后不再询问：

- **自动转实况图**（macOS）：应用内置转换助手，后台把通过的图转成 JPG + MOV 实况照片，放在 `成品/实况照片`。
- **自动导入 Poster**：通过的图按款式进入 Poster 对应商品的图片库存；两项都开时先转实况图，配对完成后连同视频导入。

归属、失败处理与恢复见 [Poster 图片接入](docs/Poster图片接入.md)。

## Agent CLI

外部 Agent 只通过 `gendesk` 命令与应用交互：CLI 打包清单，经本机控制通道请求收录，应用不监听、不扫描任何目录。GenDesk 未运行时 CLI 在后台启动它。

```bash
pnpm install:cli          # macOS/Unix：更新 ~/.local/bin/gendesk 的独立副本
gendesk doctor
gendesk validate --manifest /路径/场景任务.json
gendesk submit --manifest /路径/场景任务.json
gendesk import --manifest /路径/成图清单.json   # Codex 等外部已生成的图，直接进待验收
gendesk status <jobId>
gendesk v2v pending
```

应用升级不会自动同步 CLI；安装器检查版本并原子替换入口。完整字段、单条调用、多参考图、参数继承、去重、待确认与重试只维护在 [命令行生图接入](docs/命令行生图接入.md)。只写提示词不投单；生成结果在应用里人工审图。skill 交付边界见 [标准产出](docs/提示词skill标准产出规范.md)。

## 发布（M4）

发版只要一条命令（在 main、工作区干净、与 origin/main 一致）：

```bash
pnpm release 0.39.0
```

`scripts/release.mjs` 会：
- 改好 `package.json`、`src-tauri/tauri.conf.json`、`src-tauri/Cargo.toml`（及 `Cargo.lock`）、`scripts/gendesk.mjs` 的版本号并提交。
- 推送 main 和 tag `v0.39.0`。GitHub 的 `.github/workflows/release.yml` 随即开始打 **Windows** 包。
- 同时在本机打 **Mac** 包（Apple 芯片），签名后上传到同一个 Release。完成后 Mac 电脑就能收到更新，通常几分钟。
- 两端各写自己的 `updater-<平台>.json`，再拼成 `latest.json`（`scripts/updater-manifest.mjs`），并行也不会互相覆盖。Windows 那端完成后自动补进去。最后 `verify-updater` 会等两个平台都齐、更新包能下载。
- 更新本机 `gendesk` 命令。

先想看 Mac 包能不能打成：`pnpm release 0.39.0 --dry-run`，只在本机打包，不改版本、不推送。
中途失败修好后用同一个版本号重跑，会从打包那一步继续。
本机要有更新签名私钥 `~/.tauri/gendesk-signing.key` 和钥匙串里的代码签名证书「GenDesk Self-Signed」。

带新迁移的版本：启动时自动把旧库快照到数据目录 `database-backups/` 再升级；升级失败的弹窗可直接「检查更新」装修复版。
迁移只做加法时旧版本仍能打开升级后的库，破坏性迁移须抬高 `reader_floor`（规则见 CLAUDE.md「启动升级保护」）。

应用内更新：应用启动 20 秒后与每 6 小时自动检查 `latest.json`，有新版就后台下载，标题栏与
设置 › 通用 › 更新出现「重启安装」；有生图任务在跑时先确认再重启。

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

图片和视频按产品、款式、需求持续归档，不再按验收或运行批次创建成品目录。产品规则不写死在工具里；人工确认卡、CLI 与工单都能提供归属。当前本机作品根为项目根（与 `代码/` 分开），存量整理、快照及外部引用约定见 [作品目录整理](docs/作品目录整理.md)。
