# GenDesk V2 Backlog 与 V1 收尾自检

## §7 V2 架构预留自检（V1 完成时逐项确认未被破坏）

- [x] **多态标签未写死为分组专用**：`tag_bindings(entity_type TEXT, entity_id)`、`trash_items(entity_type)`
  均为开放字符串；V1 用 `'prompt_group'`/`'task'`/`'work'`/`'ref'`/`'prompt'`，V2 可复用到视频/文案/标题。
- [x] **资产可扩展「外部路径引用」**：`ref_images.file_path` 已是路径引用模型；V2 视频不入库场景
  （存路径+指纹）可平滑加列，不破坏现有 schema。
- [x] **引擎/Provider/commands 模块边界清晰**：`provider::ImageProvider` trait + `ProviderFactory`
  已抽象，V2 新增兼容模型 = 新 trait 实现；sidecar 进程管理可作为新模块加入 `src-tauri/src/`。
- [x] **ID 稳定 + 输出确定性**：批次/任务/作品 ID 自增稳定；输出目录 `outputs/{批次}/` 与命名
  `参考图名_YYMMDD_编号.JPG` 确定，天然是 RPA 任务单契约基础。

## V2 规划（V1 不实现，架构已预埋）

1. **Python 开源项目集成**：sidecar 子进程（PyInstaller/内置运行时），stdio JSON-RPC / 本地 HTTP，
   Rust 管生命周期；不用 PyO3 嵌入。
2. **视频/图文/文案/标题内容管理**：多态标签表复用；资产「外部路径引用」；检索加 SQLite FTS5；
   ffmpeg/ffprobe sidecar 截帧与元数据。
3. **RPA 发布任务单导出**：查库 → 导出 xlsx/csv（影刀类 RPA 对 Excel 支持最好）+ JSON；
   素材列写绝对路径；预留回执导入闭环。

## M5 人工收尾清单（AI 不可替代，交付前由用户执行）

- [ ] 双端真机 UI 冒烟：macOS(WKWebView) + Windows(WebView2) 渲染差异排查，重点
  oklch 色彩、backdrop、滚动条样式降级、自绘窗控（Win）/交通灯（mac）。
- [ ] 键盘全图走查：⌘K / ⌘1–8 / 验收 ⏎⌫R←→ / Esc 逐层关闭。
- [ ] reduced-motion 走查（系统开启后动效降级；设置页「减弱」开关）。
- [ ] 500 任务真机端到端（可用本地 wiremock 顶替真实 API）：内存/磁盘无异常增长、虚拟滚动生效。
- [ ] 日志脱敏抽查：`logs/` 无明文 Key（脱敏为 `name(****后4位)`；guardrails 另有 `sk-` 静态检查）。
- [ ] 应用内更新真机演练：旧版→新版应用内更新、更新失败不影响使用、生成中强杀→重启中断恢复。
- [ ] 首次安装引导：SmartScreen（Win）/ Gatekeeper（mac）放行，仅首次（见 README）。
- [ ] **签名私钥已入 GitHub Secrets + 离线备份**（`~/.tauri/gendesk-signing.key`；丢失=更新链断裂）。
- [ ] 更新 endpoint 改为真实公开 releases 仓库（当前 `gendesk/gendesk-releases` 为占位）。

## 里程碑级深审（用户手动触发，计费操作）

- [ ] M2 引擎分支 `/code-review ultra`
- [ ] M4 发布链分支 `/code-review ultra`
- [ ] v1.0.0 候选分支 `/code-review ultra`
