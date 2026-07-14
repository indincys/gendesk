//! 回执读取（发布模块执行计划 §3.1）。按表头行定位列（容忍整文件重写/转存），
//! 只取任务 ID + 第 20–22 列。RPA 回写信息 `链接｜原因｜时间` 解析。

use std::path::Path;

use calamine::{Data, Reader, Xlsx};

use crate::error::{AppError, AppResult};

/// 一行回执（执行器回写）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiptRow {
    pub task_code: String,
    /// 第 20 列任务状态（已发布 | 失败 | 待执行 | …）。
    pub status_zh: String,
    /// 第 21 列 RPA 回写信息原文。
    pub rpa_info: String,
    /// 第 22 列截图文件名。
    pub screenshot: String,
}

/// 解析后的 RPA 回写信息。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedRpa {
    pub url: Option<String>,
    pub reason: Option<String>,
    pub time: Option<String>,
}

/// 解析 `链接｜原因｜时间`（全角 ｜ 或半角 |）。按内容分类，字段可缺省。
pub fn parse_rpa(info: &str) -> ParsedRpa {
    let mut out = ParsedRpa::default();
    for part in info.split(['｜', '|']) {
        let p = part.trim();
        if p.is_empty() {
            continue;
        }
        if (p.contains("://") || p.starts_with("http")) && out.url.is_none() {
            out.url = Some(p.to_string());
        } else if looks_like_time(p) && out.time.is_none() {
            out.time = Some(p.to_string());
        } else if out.reason.is_none() {
            out.reason = Some(p.to_string());
        }
    }
    out
}

/// 粗略时间样式：含 `:` 且主要为数字/分隔符（如 `2026-07-15 12:30` / `12:30`）。
fn looks_like_time(s: &str) -> bool {
    s.contains(':')
        && s.chars()
            .all(|c| c.is_ascii_digit() || matches!(c, ':' | '-' | '/' | ' ' | 'T' | 'Z' | '.'))
}

/// 是否为应忽略的文件名（Excel 锁文件 / 同步冲突副本）。
// 供任务包 watcher 过滤冲突副本；canonical `任务单.xlsx` 读路径已隐式忽略，故保留待用。
#[allow(dead_code)]
pub fn is_ignored_xlsx(filename: &str) -> bool {
    filename.starts_with("~$")
        || filename.starts_with('.')
        || filename.contains("conflicted")
        || filename.contains("冲突")
        || filename.contains("(1)")
}

fn cell_string(range: &calamine::Range<Data>, r: usize, c: usize) -> String {
    range.get((r, c)).map(|d| d.to_string()).unwrap_or_default()
}

/// 读取回执：按表头定位列，返回每行 (任务ID, 状态, RPA信息, 截图)。
/// 表头必须含「任务ID」；缺失则报错（不是任务单）。
pub fn read_receipts(path: &Path) -> AppResult<Vec<ReceiptRow>> {
    let mut wb: Xlsx<_> =
        calamine::open_workbook(path).map_err(|e| AppError::Io(format!("打开 xlsx 失败：{e}")))?;
    let range = wb
        .worksheet_range_at(0)
        .ok_or_else(|| AppError::Io("xlsx 无工作表".into()))?
        .map_err(|e| AppError::Io(format!("读取工作表失败：{e}")))?;

    if range.height() < 1 {
        return Ok(Vec::new());
    }
    // 表头定位（trim + 去空格容错）。
    let norm = |s: String| s.trim().replace([' ', '　'], "");
    let width = range.width();
    let col_of = |name: &str| -> Option<usize> {
        (0..width).find(|&c| norm(cell_string(&range, 0, c)) == name)
    };
    let c_id = col_of("任务ID")
        .ok_or_else(|| AppError::InvalidInput("xlsx 缺「任务ID」表头，非任务单".into()))?;
    let c_status = col_of("任务状态");
    let c_rpa = col_of("RPA回写信息");
    let c_shot = col_of("截图文件名");

    let mut out = Vec::new();
    for r in 1..range.height() {
        let task_code = cell_string(&range, r, c_id).trim().to_string();
        if task_code.is_empty() {
            continue;
        }
        out.push(ReceiptRow {
            task_code,
            status_zh: c_status
                .map(|c| cell_string(&range, r, c).trim().to_string())
                .unwrap_or_default(),
            rpa_info: c_rpa.map(|c| cell_string(&range, r, c)).unwrap_or_default(),
            screenshot: c_shot
                .map(|c| cell_string(&range, r, c).trim().to_string())
                .unwrap_or_default(),
        });
    }
    Ok(out)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即测试失败，是期望行为
mod tests {
    use super::*;
    use crate::publish::xlsx::writer::{write_sheet, XlsxRow};

    #[test]
    fn parse_rpa_success_and_fail() {
        let s = parse_rpa("https://xhs.com/abc｜｜2026-07-15 12:30");
        assert_eq!(s.url.as_deref(), Some("https://xhs.com/abc"));
        assert_eq!(s.time.as_deref(), Some("2026-07-15 12:30"));
        assert_eq!(s.reason, None);

        let f = parse_rpa("风控拦截｜2026-07-15 20:00");
        assert_eq!(f.url, None);
        assert_eq!(f.reason.as_deref(), Some("风控拦截"));
        assert_eq!(f.time.as_deref(), Some("2026-07-15 20:00"));

        // 半角分隔 + 仅原因
        assert_eq!(parse_rpa("登录失效").reason.as_deref(), Some("登录失效"));
    }

    #[test]
    fn ignore_lock_and_conflict_files() {
        assert!(is_ignored_xlsx("~$任务单.xlsx"));
        assert!(is_ignored_xlsx("任务单 (conflicted copy).xlsx"));
        assert!(is_ignored_xlsx("任务单-冲突.xlsx"));
        assert!(!is_ignored_xlsx("任务单.xlsx"));
    }

    // 模拟执行器：整文件重写（只回写 20–22 列）→ 对账按表头读回正确。
    #[test]
    fn read_receipts_after_executor_rewrite() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("任务单.xlsx");
        // 用 writer 造一份，然后手动改第 20–22 列模拟回写（这里直接写带回执的行）。
        let mut row = XlsxRow {
            task_id: "T260715-001".into(),
            task_date: "2026-07-15".into(),
            platform_zh: "小红书".into(),
            account_name: "主号".into(),
            sku_code: "SF-1".into(),
            title: "标题".into(),
            content_kind_zh: "视频".into(),
            media_filename: "video.mp4".into(),
            status_zh: "已发布".into(),
            rpa_info: "https://xhs.com/x｜｜2026-07-15 12:30".into(),
            screenshot: "T260715-001.png".into(),
            ..Default::default()
        };
        row.topics[0] = "沙发".into();
        let row2 = XlsxRow {
            task_id: "T260715-002".into(),
            status_zh: "失败".into(),
            rpa_info: "风控拦截｜2026-07-15 20:00".into(),
            ..Default::default()
        };
        write_sheet(&path, &[row, row2]).unwrap();

        let receipts = read_receipts(&path).unwrap();
        assert_eq!(receipts.len(), 2);
        assert_eq!(receipts[0].task_code, "T260715-001");
        assert_eq!(receipts[0].status_zh, "已发布");
        assert_eq!(receipts[0].screenshot, "T260715-001.png");
        let rpa = parse_rpa(&receipts[0].rpa_info);
        assert_eq!(rpa.url.as_deref(), Some("https://xhs.com/x"));
        assert_eq!(receipts[1].status_zh, "失败");
    }

    #[test]
    fn read_receipts_rejects_non_sheet() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("x.xlsx");
        // 写一份没有「任务ID」表头的空表。
        let mut wb = rust_xlsxwriter::Workbook::new();
        let ws = wb.add_worksheet();
        ws.write_string(0, 0, "别的表头").unwrap();
        wb.save(&path).unwrap();
        assert!(read_receipts(&path).is_err());
    }
}
