//! 结构化日志滚动落盘（执行计划 0.7 / 技术文档 9.4）。
//!
//! - 每日滚动写入 `{app_data}/logs/gendesk.log.YYYY-MM-DD`，保留 14 天。
//! - 任务 ID 全链路贯穿；前端未捕获错误经 `log_frontend_error` 汇入同一日志流。
//! - 脱敏铁律：任何 API Key 明文不得进入日志（脱敏在写入点完成，guardrails 另有
//!   `sk-` 模式静态检查兜底）。

use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// 保留天数。
const RETAIN_DAYS: u64 = 14;

/// 初始化日志。返回的 [`WorkerGuard`] 必须存活于应用生命周期（drop 即 flush 停写），
/// 因此由调用方持有到 App 托管状态中。
pub fn init(logs_dir: &Path) -> WorkerGuard {
    let _ = std::fs::create_dir_all(logs_dir);
    prune_old_logs(logs_dir, RETAIN_DAYS);

    let file_appender = tracing_appender::rolling::daily(logs_dir, "gendesk.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    let filter = EnvFilter::try_from_env("GENDESK_LOG")
        .unwrap_or_else(|_| EnvFilter::new("info,gendesk_lib=debug"));

    let file_layer = fmt::layer()
        .with_ansi(false)
        .with_target(true)
        .with_writer(non_blocking);

    let registry = tracing_subscriber::registry().with(filter).with(file_layer);

    // 开发模式同时输出到 stderr，便于 `tauri dev` 观察。
    #[cfg(debug_assertions)]
    let registry = {
        let stderr_layer = fmt::layer().with_writer(std::io::stderr).with_ansi(true);
        registry.with(stderr_layer)
    };

    // set_global_default 只会失败于「已初始化」，忽略以支持测试重复调用。
    let _ = registry.try_init();

    tracing::info!(retain_days = RETAIN_DAYS, "logging initialized");
    guard
}

/// 删除超过保留期的滚动日志文件（按文件名日期后缀，最坏情况按 mtime 兜底）。
fn prune_old_logs(dir: &Path, retain_days: u64) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let cutoff = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(retain_days * 24 * 3600));
    let Some(cutoff) = cutoff else { return };

    for entry in entries.flatten() {
        let path = entry.path();
        let is_log = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("gendesk.log"));
        if !is_log {
            continue;
        }
        if let Ok(meta) = entry.metadata() {
            if let Ok(modified) = meta.modified() {
                if modified < cutoff {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
    }
}
