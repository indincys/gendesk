//! IPC 命令层（技术文档 3.2 / 执行计划 §2）。
//!
//! 命令按业务域拆分为子模块，薄壳只做参数校验 + 调用 → 业务真相始终在下层。
//! 全部命令签名经 tauri-specta 自动导出到 `src/lib/ipc/bindings.ts`（禁手改）。

pub mod api_keys;
pub mod misc;
pub mod prompts;
pub mod refs;
pub mod settings;
