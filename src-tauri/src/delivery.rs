//! 交付：验收通过的图**按确定的名字**交出去，与 `自动发布` 的唯一契约。
//!
//! ```text
//! <交付根>/
//! └── 交付-260804-1530/
//!     ├── 王一博-55/
//!     │   ├── W-王一博-55-正面-hu72.jpg
//!     │   └── W-王一博-55-侧面-k3p9.jpg
//!     ├── manifest.jsonl
//!     └── READY.txt          ← 最后写
//! ```
//!
//! 一次验收 = 一个交付批。与 intake 工单形制对称，`自动发布` 收完可整目录移走。
//!
//! ## 契约是目录 + manifest，不是共享数据库
//!
//! 让对面直连 GenDesk 的 SQLite 会把 forward-only 迁移变成两个软件的公共约束：
//! 这边每动一次表，那边就得跟着改。目录是稳定面。
//!
//! **下游永远读 manifest，不解析文件名。** 文件名只是人工核对的双保险 ——
//! 从中文文件名反推 SKU 是静默失败（`王一博` / `王一博 ` / `王一博A` 迟早出现，
//! 而分错目录不报错）。
//!
//! ## 写盘协议
//!
//! 整批拷完 → 写 `manifest.jsonl` → **最后**写 `READY.txt`。对方没看到 READY
//! 就一个字节都不碰 —— 与 intake 那一侧同一条规矩，方向相反而已。

use std::path::{Path, PathBuf};

use serde::Serialize;

/// 清单文件名（一行一图的 JSON Lines）。
pub const MANIFEST: &str = "manifest.jsonl";
/// 就绪标记。**最后写**。
pub const READY: &str = "READY.txt";
/// SKU / 风格都不知道时的落脚目录（生成页手动建的批、历史数据）。
///
/// **不猜**：从中文文件名反推 SKU 会分错目录且不报错。宁可全堆在一个明说
/// 「没归类」的目录里，让人一眼看见。
pub const UNSORTED: &str = "未分类";

/// 默认交付根。
pub fn default_root() -> PathBuf {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join("GenDesk交付")
}

/// 本次交付批的目录名：`交付-YYMMDD-HHMM`。
///
/// 与工单目录 `投单-260804-1530` 同形，两边摆在一起读得出对应关系。
pub fn batch_dir_name(at: i64) -> String {
    format!("交付-{}", crate::files::stamp_yymmdd_hhmm(at))
}

/// SKU 子目录名：`{风格}-{SKU}`。
///
/// 键来自作品的快照列（`style` / `sku_code`），**不从文件名解析**。两者缺一
/// 就整个退回 [`UNSORTED`] —— 半个键分出来的目录比没分更难收拾。
pub fn sku_dir_name(style: &str, sku_code: &str) -> String {
    let (s, k) = (style.trim(), sku_code.trim());
    if s.is_empty() || k.is_empty() {
        return UNSORTED.to_string();
    }
    crate::files::sanitize_filename(&format!("{s}-{k}"))
}

/// manifest 的一行。字段名是与 `自动发布` 的契约，改名 = 改契约。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestRow {
    /// 交付批内的相对路径，如 `王一博-55/W-王一博-55-正面-hu72.jpg`。
    pub file: String,
    /// 语义名（不含 UID）。
    pub name: String,
    /// 4 位 UID，全局唯一 —— 下游可以只存它当主键。
    pub uid: String,
    pub sku: String,
    pub style: String,
    /// 受控用途的**短码**（`i2v`）；没有用途时是空串。
    pub purpose: String,
    /// 参考图名（人工核对用）。
    pub r#ref: String,
    /// 实际发出去的那一版提示词正文。
    pub prompt: String,
    /// 这张图来自哪一次投单。
    pub job_id: String,
    pub work_id: i64,
    pub accepted_at: i64,
}

/// 把一批 manifest 行写成 JSON Lines。
pub fn manifest_body(rows: &[ManifestRow]) -> Result<String, String> {
    let mut s = String::new();
    for r in rows {
        let line = serde_json::to_string(r).map_err(|e| format!("manifest 序列化失败：{e}"))?;
        s.push_str(&line);
        s.push('\n');
    }
    Ok(s)
}

/// 收尾：写 manifest，**最后**写 READY。
///
/// 顺序是承重的：对方的 watcher 认 READY，看见它就会把整个目录搬走。先写 READY
/// 再写 manifest，等于让对方搬走一个没有清单的目录 —— 而那时文件已经不在我们这边了。
pub fn finalize(dir: &Path, rows: &[ManifestRow]) -> Result<(), String> {
    let body = manifest_body(rows)?;
    std::fs::write(dir.join(MANIFEST), body).map_err(|e| format!("写 manifest 失败：{e}"))?;
    std::fs::write(dir.join(READY), format!("{} 张\n", rows.len()))
        .map_err(|e| format!("写 {READY} 失败：{e}"))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即失败
mod tests {
    use super::*;

    #[test]
    fn sku_dir_falls_back_when_either_half_is_missing() {
        assert_eq!(sku_dir_name("王一博", "55"), "王一博-55");
        // 半个键分出来的目录比没分更难收拾 —— 缺一个就整个退回未分类。
        assert_eq!(sku_dir_name("王一博", ""), UNSORTED);
        assert_eq!(sku_dir_name("", "55"), UNSORTED);
        assert_eq!(sku_dir_name(" ", " "), UNSORTED);
        // 键来自快照列，但仍要过一遍文件名清洗：它要当目录名用。
        assert!(!sku_dir_name("王/一博", "55").contains('/'));
    }

    #[test]
    fn manifest_is_one_json_object_per_line() {
        let row = |id: i64, uid: &str| ManifestRow {
            file: format!("王一博-55/W-{id}-{uid}.jpg"),
            name: format!("W-{id}"),
            uid: uid.into(),
            sku: "55".into(),
            style: "王一博".into(),
            purpose: "i2v".into(),
            r#ref: "正面.jpg".into(),
            prompt: "正文".into(),
            job_id: "投单-260804-1530".into(),
            work_id: id,
            accepted_at: 1_754_300_000,
        };
        let body = manifest_body(&[row(1, "hu72"), row(2, "k3p9")]).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 2);
        for l in lines {
            let v: serde_json::Value = serde_json::from_str(l).expect("每行都得是独立 JSON");
            assert_eq!(v["sku"], "55");
            // camelCase 是契约的一部分。
            assert!(v.get("jobId").is_some() && v.get("workId").is_some(), "{l}");
            assert!(v.get("job_id").is_none(), "不得混用 snake_case：{l}");
        }
        assert!(body.ends_with('\n'), "JSON Lines 每行都以换行收尾");
    }

    /// manifest 的每一行都必须指向**真实存在**的文件。
    ///
    /// 下游读 manifest 而不解析文件名，所以这份清单就是它看见的全部世界：
    /// 少一行 = 那张图白花了钱没人知道；多一行 = 对方去取一个不存在的文件。
    #[test]
    fn every_manifest_row_points_at_a_real_file() {
        let d = tempfile::tempdir().unwrap();
        let rows: Vec<ManifestRow> = [("hu72", "正面"), ("k3p9", "侧面")]
            .iter()
            .enumerate()
            .map(|(i, (uid, part))| {
                let dir = sku_dir_name("王一博", "55");
                let file = format!("{dir}/W-王一博-55-{part}-{uid}.jpg");
                std::fs::create_dir_all(d.path().join(&dir)).unwrap();
                std::fs::write(d.path().join(&file), b"img").unwrap();
                ManifestRow {
                    file,
                    name: format!("W-王一博-55-{part}"),
                    uid: (*uid).into(),
                    sku: "55".into(),
                    style: "王一博".into(),
                    purpose: "i2v".into(),
                    r#ref: format!("{part}.jpg"),
                    prompt: "正文".into(),
                    job_id: "投单-260804-1530".into(),
                    work_id: i as i64 + 1,
                    accepted_at: 0,
                }
            })
            .collect();
        finalize(d.path(), &rows).unwrap();

        let body = std::fs::read_to_string(d.path().join(MANIFEST)).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), rows.len(), "行数 = 本次交付张数");
        for l in lines {
            let v: serde_json::Value = serde_json::from_str(l).unwrap();
            let rel = v["file"].as_str().expect("file 必须是字符串");
            assert!(
                d.path().join(rel).is_file(),
                "manifest 指向了不存在的文件：{rel}"
            );
        }
    }

    // READY 必须**最后**写：对方看见它就会把整个目录搬走。
    #[test]
    fn ready_is_written_after_the_manifest() {
        let d = tempfile::tempdir().unwrap();
        finalize(d.path(), &[]).unwrap();
        let m = std::fs::metadata(d.path().join(MANIFEST)).unwrap();
        let r = std::fs::metadata(d.path().join(READY)).unwrap();
        assert!(
            r.modified().unwrap() >= m.modified().unwrap(),
            "READY.txt 的 mtime 不得早于 manifest"
        );
    }
}
