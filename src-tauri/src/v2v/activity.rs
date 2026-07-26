//! 视频流水线执行日志 —— 让「进行到哪一步 / 有没有报错」在应用里看得见。
//!
//! ## 为什么需要它
//!
//! 这条链路上全部的操作信号原先都进了 `tracing::warn!`：即梦查询失败、成片改名失败、
//! 「报了成功却没返回落盘路径」、CLI 拒绝提交时打在 stdout 里的那句原因……在打包好的
//! GUI 里**一条都看不到**。于是看板显示「已提交 19」，旁边没有任何东西能回答
//! 「它们到底在干嘛、有没有报错」——这正是用户报的第一个问题。
//!
//! 日志不是错误处理的替代品：条目该置 fail 的照样置 fail。它回答的是另一个问题——
//! 「刚才那几分钟里，这个程序替我做了什么」。
//!
//! ## 为什么是环形缓冲而不是落库
//!
//! 这是诊断信号，不是业务真相。轮询每 6 秒一轮，落库意味着无谓地产生写事务，
//! 而它能回答的问题（「刚才发生了什么」）本来就只需要最近几百条。
//! 需要跨重启留存的那部分（每条 clip 此刻的即梦状态）另有归宿：0021 的三列。
//!
//! ## 为什么显式传句柄而不是全局单例
//!
//! 全局单例（如 `tracing` 那样）会更省事，但那样测试就无法断言「这一步到底记了什么」，
//! 而「记没记下来」正是这个模块存在的唯一意义。句柄可 clone、无 app 时静默丢弃，
//! 测试里造一个空的即可。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::db::now_unix;

/// 环形缓冲容量。够回答「刚才那几轮发生了什么」，又不至于让内存里躺着一份日志文件。
pub const CAP: usize = 500;

/// 单条消息与详情的长度上限（按**字符**截，不按字节 —— 中文按字节切会切出乱码）。
const MSG_MAX: usize = 400;
const DETAIL_MAX: usize = 2000;

/// 一条执行日志。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ActivityEntry {
    /// 单调递增序号。前端据此去重与增量追加（事件可能与快照重叠）。
    pub seq: i64,
    pub at: i64,
    /// `info` / `warn` / `error`。
    pub level: String,
    /// `cli` 调用 · `submit` 提交 · `poll` 轮询 · `media` 落盘 · `handoff` 交接目录。
    pub phase: String,
    pub clip_id: Option<i64>,
    /// 条目编号（如 `BR46-0003`），让日志能和看板上的卡片对上。
    pub code: String,
    pub message: String,
    /// 命令行原文 / CLI 输出片段等「想细看才看」的内容。
    pub detail: Option<String>,
}

#[derive(Default)]
struct Inner {
    seq: AtomicI64,
    buf: Mutex<VecDeque<ActivityEntry>>,
}

/// 可 clone 的日志句柄。无 `AppHandle` 时只入缓冲、不推事件（测试与后台早期阶段）。
#[derive(Clone, Default)]
pub struct Activity {
    inner: Arc<Inner>,
    app: Option<tauri::AppHandle>,
}

/// 日志里指代某条 clip 的最小信息：id 用于跳转，编号用于人读。
pub type Who<'a> = Option<(i64, &'a str)>;

impl Activity {
    /// 带事件推送的句柄（应用运行时用这个）。
    pub fn new(app: tauri::AppHandle) -> Self {
        Self {
            inner: Arc::default(),
            app: Some(app),
        }
    }

    /// 只入缓冲、不推事件（测试用）。
    #[cfg(test)]
    pub fn silent() -> Self {
        Self::default()
    }

    /// 供调用方在批量操作中顺带推别的事件（如逐条提交后刷新看板）。
    pub fn app(&self) -> Option<&tauri::AppHandle> {
        self.app.as_ref()
    }

    pub fn info(
        &self,
        phase: &str,
        who: Who<'_>,
        message: impl Into<String>,
        detail: Option<String>,
    ) {
        self.push("info", phase, who, message.into(), detail);
    }

    pub fn warn(
        &self,
        phase: &str,
        who: Who<'_>,
        message: impl Into<String>,
        detail: Option<String>,
    ) {
        self.push("warn", phase, who, message.into(), detail);
    }

    pub fn error(
        &self,
        phase: &str,
        who: Who<'_>,
        message: impl Into<String>,
        detail: Option<String>,
    ) {
        self.push("error", phase, who, message.into(), detail);
    }

    fn push(
        &self,
        level: &str,
        phase: &str,
        who: Who<'_>,
        message: String,
        detail: Option<String>,
    ) {
        let entry = ActivityEntry {
            seq: self.inner.seq.fetch_add(1, Ordering::Relaxed) + 1,
            at: now_unix(),
            level: level.to_string(),
            phase: phase.to_string(),
            clip_id: who.map(|(id, _)| id),
            code: who.map(|(_, c)| c.to_string()).unwrap_or_default(),
            message: clip_chars(&message, MSG_MAX),
            detail: detail.map(|d| clip_chars(&d, DETAIL_MAX)),
        };
        // 同一条同时进 tracing：日志面板回答「刚才发生了什么」，tracing 文件回答
        // 「上周三那次是怎么回事」。两者受众不同，不是重复。
        match level {
            "error" => tracing::error!(phase, code = %entry.code, "{}", entry.message),
            "warn" => tracing::warn!(phase, code = %entry.code, "{}", entry.message),
            _ => tracing::info!(phase, code = %entry.code, "{}", entry.message),
        }
        {
            let mut buf = lock(&self.inner.buf);
            buf.push_back(entry.clone());
            while buf.len() > CAP {
                buf.pop_front();
            }
        }
        if let Some(app) = &self.app {
            use tauri_specta::Event;
            let _ = super::events::V2vActivity { entry }.emit(app);
        }
    }

    /// 当前缓冲快照（打开日志面板时一次取完；之后靠事件增量追加）。
    pub fn snapshot(&self) -> Vec<ActivityEntry> {
        lock(&self.inner.buf).iter().cloned().collect()
    }

    pub fn clear(&self) {
        lock(&self.inner.buf).clear();
    }
}

/// 中毒的锁照样用（`into_inner`）：日志写一半 panic 最坏是丢一条记录，
/// 为此让**调用方**跟着失败，等于让诊断设施把它要诊断的流程搞挂。
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// 按字符截断（中文按字节切会切出乱码）。
fn clip_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max).collect();
    format!("{head}…")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即失败
mod tests {
    use super::*;

    #[test]
    fn entries_are_kept_in_order_with_monotonic_seq() {
        let log = Activity::silent();
        log.info("poll", None, "第一条", None);
        log.warn("cli", Some((7, "BR46-0003")), "第二条", Some("cmd".into()));
        let snap = log.snapshot();
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[0].seq, 1);
        assert_eq!(snap[1].seq, 2);
        assert_eq!(snap[1].level, "warn");
        assert_eq!(snap[1].clip_id, Some(7));
        assert_eq!(snap[1].code, "BR46-0003");
    }

    // 环形缓冲：满了丢最旧的，而不是无限长大（这是常驻进程，日志面板不是日志文件）。
    #[test]
    fn ring_buffer_drops_oldest_beyond_cap() {
        let log = Activity::silent();
        for i in 0..(CAP + 10) {
            log.info("poll", None, format!("第 {i} 条"), None);
        }
        let snap = log.snapshot();
        assert_eq!(snap.len(), CAP);
        assert_eq!(snap[0].seq, 11, "最旧的 10 条该被挤掉");
        assert_eq!(snap[CAP - 1].seq, (CAP + 10) as i64);
    }

    // 按**字符**截断：即梦的失败原因与我们的提示词全是中文，按字节切会切出乱码，
    // 而看不懂的错误信息等于没有错误信息。
    #[test]
    fn truncation_is_char_wise_so_chinese_stays_readable() {
        let log = Activity::silent();
        let long = "极缓地".repeat(500); // 1500 字符
        log.error("submit", None, long.clone(), Some(long));
        let e = &log.snapshot()[0];
        assert!(e.message.chars().count() <= MSG_MAX + 1);
        assert!(e.message.ends_with('…'));
        assert!(
            e.message.contains("极缓地"),
            "不得切出半个字：{}",
            e.message
        );
        assert!(e.detail.as_ref().unwrap().chars().count() <= DETAIL_MAX + 1);
    }

    // 句柄是共享的：轮询器持有的那份与命令层读的那份必须是同一个缓冲，
    // 否则日志面板永远是空的，而这个模块的全部意义就在于「看得见」。
    #[test]
    fn clones_share_one_buffer() {
        let a = Activity::silent();
        let b = a.clone();
        b.info("poll", None, "来自克隆", None);
        assert_eq!(a.snapshot().len(), 1);
        a.clear();
        assert!(b.snapshot().is_empty());
    }
}
