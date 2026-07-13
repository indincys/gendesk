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

## 里程碑

M0 骨架门禁 · M1 数据层 · M2 任务引擎 · M3 八大页面 · M4 更新发布链 · M5 收尾（见 CLAUDE.md）。
