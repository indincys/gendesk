//! 收件箱收录子系统（发布模块执行计划 §5.1 inbox/）。
//!
//! parser：TXT 三类解析 + 【话题】+ SKU 三冗余识别（纯函数，proptest 不 panic）。
//! watcher/ingest：notify 监听 + 大小稳定防抖 + 收录事务（P1 后续任务接入）。

pub mod ingest;
pub mod parser;
pub mod watcher;
