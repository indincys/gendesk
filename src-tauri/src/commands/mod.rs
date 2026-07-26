//! IPC 命令层（技术文档 3.2 / 执行计划 §2）。
//!
//! 命令按业务域拆分为子模块，薄壳只做参数校验 + 调用 → 业务真相始终在下层。
//! 全部命令签名经 tauri-specta 自动导出到 `src/lib/ipc/bindings.ts`（禁手改）。

pub mod api_keys;
pub mod backup;
pub mod batches;
pub mod intake;
pub mod misc;
pub mod prompts;
pub mod publish_accounts;
pub mod publish_assets;
pub mod publish_inbox;
pub mod publish_insights;
pub mod publish_planning;
pub mod publish_reconcile;
pub mod publish_settings;
pub mod publish_skus;
pub mod publish_texts;
pub mod refs;
pub mod review;
pub mod settings;
pub mod stats;
pub mod tasks;
pub mod trash;
pub mod updater;
pub mod v2v;
pub mod works;
