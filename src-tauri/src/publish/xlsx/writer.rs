//! 任务单 xlsx 写入（22 列，需求 §5）。只写值与列宽，不做花哨样式（执行器只读值）。

use std::path::Path;

use rust_xlsxwriter::Workbook;

use crate::error::{AppError, AppResult};
use crate::publish::xlsx::HEADERS;

/// 一行任务的 22 列值（GenDesk 侧写前 19 列 + 第 20 列初值「待执行」；21/22 留空给执行器）。
#[derive(Debug, Clone, Default)]
pub struct XlsxRow {
    pub task_id: String,
    pub task_date: String,
    pub planned_time: String,
    pub platform_zh: String,
    pub account_name: String,
    pub style_name: String,
    pub sku_code: String,
    pub product_name: String,
    /// 视频 | 图文
    pub content_kind_zh: String,
    /// `video.mp4` 或 `img_01.jpg…img_06.jpg`
    pub media_filename: String,
    pub material_path: String,
    pub cover_path: String,
    pub title: String,
    pub body_path: String,
    /// 话题一~五（不足留空）。
    pub topics: [String; 5],
    /// 初值「待执行」。
    pub status_zh: String,
    pub rpa_info: String,
    pub screenshot: String,
}

impl XlsxRow {
    /// 按列顺序展开为 22 个字符串。
    pub fn cells(&self) -> [&str; 22] {
        [
            &self.task_id,
            &self.task_date,
            &self.planned_time,
            &self.platform_zh,
            &self.account_name,
            &self.style_name,
            &self.sku_code,
            &self.product_name,
            &self.content_kind_zh,
            &self.media_filename,
            &self.material_path,
            &self.cover_path,
            &self.title,
            &self.body_path,
            &self.topics[0],
            &self.topics[1],
            &self.topics[2],
            &self.topics[3],
            &self.topics[4],
            &self.status_zh,
            &self.rpa_info,
            &self.screenshot,
        ]
    }
}

/// 写任务单 xlsx（表头 + 数据行）到 path。
pub fn write_sheet(path: &Path, rows: &[XlsxRow]) -> AppResult<()> {
    let mut wb = Workbook::new();
    let ws = wb.add_worksheet();

    for (c, h) in HEADERS.iter().enumerate() {
        ws.write_string(0, c as u16, *h)
            .map_err(|e| AppError::Io(format!("写 xlsx 表头失败：{e}")))?;
    }
    for (r, row) in rows.iter().enumerate() {
        for (c, val) in row.cells().iter().enumerate() {
            ws.write_string((r + 1) as u32, c as u16, *val)
                .map_err(|e| AppError::Io(format!("写 xlsx 行失败：{e}")))?;
        }
    }
    // 关键列稍加宽，便于人工核对（非样式契约）。
    let _ = ws.set_column_width(0, 12); // 任务ID
    let _ = ws.set_column_width(12, 32); // 标题
    let _ = ws.set_column_width(10, 40); // 素材本地路径

    wb.save(path)
        .map_err(|e| AppError::Io(format!("保存 xlsx 失败：{e}")))?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即测试失败，是期望行为
mod tests {
    use super::*;
    use calamine::{Data, Reader, Xlsx};

    fn sample_row() -> XlsxRow {
        XlsxRow {
            task_id: "T260715-001".into(),
            task_date: "2026-07-15".into(),
            planned_time: "12:30".into(),
            platform_zh: "小红书".into(),
            account_name: "主号".into(),
            style_name: "云朵沙发".into(),
            sku_code: "SF-YD-201".into(),
            product_name: "商品A".into(),
            content_kind_zh: "视频".into(),
            media_filename: "video.mp4".into(),
            material_path: "D:\\视频发布\\任务包\\20260715\\素材\\SF-YD-201".into(),
            cover_path: String::new(),
            title: "小户型也能拥有的云朵感沙发".into(),
            body_path: String::new(),
            topics: [
                "沙发".into(),
                "家居".into(),
                String::new(),
                String::new(),
                String::new(),
            ],
            status_zh: "待执行".into(),
            rpa_info: String::new(),
            screenshot: String::new(),
        }
    }

    #[test]
    fn roundtrip_22_columns() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("任务单.xlsx");
        let rows = vec![sample_row()];
        write_sheet(&path, &rows).unwrap();

        let mut wb: Xlsx<_> = calamine::open_workbook(&path).unwrap();
        let range = wb.worksheet_range_at(0).unwrap().unwrap();
        // 表头行 22 列逐字一致。
        for (c, h) in HEADERS.iter().enumerate() {
            let cell = range.get((0, c)).unwrap();
            assert_eq!(cell.to_string(), *h, "第 {} 列表头", c + 1);
        }
        // 数据行逐格一致。
        let expect = sample_row();
        let cells = expect.cells();
        for (c, v) in cells.iter().enumerate() {
            let cell = range.get((1, c)).map(|d| d.to_string()).unwrap_or_default();
            assert_eq!(&cell, v, "第 {} 列数据", c + 1);
        }
        // 共 22 列。
        assert_eq!(range.get_size().1, 22);
        let _ = Data::Empty; // 触发 calamine::Data 引用（避免未用告警）
    }
}
