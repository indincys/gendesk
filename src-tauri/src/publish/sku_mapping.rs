//! SKU 映射表解析（发布模块 · 批量建档/改档）。
//!
//! 一份表同时承担「批量建 SKU」与「补文件夹别名/话题」：编码不存在 → 新建，存在 → 就地更新。
//! **空单元格一律不动**（既不清空已有值，也不覆盖收件箱采纳的话题）——只有写了内容的格子才生效。
//!
//! 输入容忍：`.xlsx`（首个工作表）与 `.csv/.tsv/.txt`（UTF-8 / GBK 自动探测，
//! 带引号字段按 RFC4180 解析，允许字段内换行）。表头行可选：识别到列名就按列名对应
//! （顺序随意、多余列忽略），识别不到则回退到位置约定 `编码, 别名, 话题`（兼容旧格式）。

use std::path::Path;

use calamine::{Data, Reader, Xlsx};

use crate::error::{AppError, AppResult};
use crate::importer;
use crate::publish::paths;
use crate::publish::platform::Platform;

/// 模板/识别共用的列顺序。
pub const TEMPLATE_HEADER: [&str; 9] = [
    "SKU编码",
    "款式名",
    "品名",
    "文件夹别名",
    "话题",
    "分层",
    "平台",
    "状态",
    "备注",
];

/// 一列的语义。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Code,
    StyleName,
    ProductName,
    Alias,
    Topics,
    Tier,
    Platforms,
    Status,
    Note,
}

/// 表内一行（空字段 = `None` = 不动）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MappingRow {
    /// 源文件行号（1-based，供报告定位）。
    pub line: usize,
    pub code: String,
    pub style_name: Option<String>,
    pub product_name: Option<String>,
    pub alias: Option<String>,
    pub topics: Option<Vec<String>>,
    pub tier: Option<String>,
    /// `None`=不动；`Some(None)`=跟随全局矩阵；`Some(Some(..))`=平台覆盖。
    pub platforms: Option<Option<Vec<String>>>,
    pub status: Option<String>,
    pub note: Option<String>,
}

/// 解析结果。
#[derive(Debug, Clone, Default)]
pub struct ParsedMapping {
    /// 探测到的编码名（xlsx 为 `XLSX`）。
    pub encoding: String,
    pub had_header: bool,
    pub rows: Vec<MappingRow>,
    /// 无法导入的行（缺编码 / 编码非法 / 文件内编码重复）。
    pub errors: Vec<String>,
}

/// 列名归一：去空白与常见修饰符后小写（`SKU 编码` / `sku_code` / `【编码】` 同解）。
fn norm(s: &str) -> String {
    s.chars()
        .filter(|c| {
            !c.is_whitespace()
                && !matches!(
                    c,
                    '_' | '-'
                        | '('
                        | ')'
                        | '（'
                        | '）'
                        | '「'
                        | '」'
                        | '【'
                        | '】'
                        | '*'
                        | ':'
                        | '：'
                )
        })
        .flat_map(char::to_lowercase)
        .collect()
}

/// 表头单元格 → 列语义。
pub fn field_of(header: &str) -> Option<Field> {
    match norm(header).as_str() {
        "编码" | "编号" | "sku" | "sku编码" | "skucode" | "code" | "款号" | "货号" => {
            Some(Field::Code)
        }
        "款式名" | "款式" | "款式名称" | "名称" | "style" | "stylename" => {
            Some(Field::StyleName)
        }
        "品名" | "产品名" | "商品名" | "产品名称" | "商品名称" | "product" | "productname" => {
            Some(Field::ProductName)
        }
        "别名" | "文件夹别名" | "文件夹" | "文件夹名" | "目录" | "目录名" | "alias" | "folder"
        | "folderalias" => Some(Field::Alias),
        "话题" | "标签" | "话题标签" | "topic" | "topics" | "tags" => Some(Field::Topics),
        "分层" | "热度" | "层级" | "tier" => Some(Field::Tier),
        "平台" | "发布平台" | "平台覆盖" | "platform" | "platforms" => {
            Some(Field::Platforms)
        }
        "状态" | "status" => Some(Field::Status),
        "备注" | "说明" | "note" | "remark" | "memo" => Some(Field::Note),
        _ => None,
    }
}

/// 分层：`热款/温款/冷款`（含单字与英文）→ 库内枚举。
fn parse_tier(s: &str) -> Option<String> {
    match norm(s).as_str() {
        "热" | "热款" | "hot" => Some("hot".into()),
        "温" | "温款" | "warm" => Some("warm".into()),
        "冷" | "冷款" | "cold" => Some("cold".into()),
        _ => None,
    }
}

/// 状态：`在售/停发` → `active/paused`。
fn parse_status(s: &str) -> Option<String> {
    match norm(s).as_str() {
        "在售" | "启用" | "正常" | "active" | "on" => Some("active".into()),
        "停发" | "停用" | "暂停" | "下架" | "paused" | "off" => Some("paused".into()),
        _ => None,
    }
}

/// 话题：空白/顿号/逗号/分号分隔，去 `#`、去重、最多 5 个。
pub fn parse_topics(s: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for tok in s.split(|c: char| {
        c.is_whitespace() || matches!(c, '、' | ',' | '，' | ';' | '；' | '/' | '|' | '｜')
    }) {
        let t = tok.trim().trim_start_matches('#').trim();
        if !t.is_empty() && !out.iter().any(|x| x == t) {
            out.push(t.to_string());
        }
        if out.len() >= 5 {
            break;
        }
    }
    out
}

/// 平台列：`Some(None)`=显式跟随全局；`Some(Some(codes))`=覆盖；未识别的名字进 `unknown`。
fn parse_platforms(s: &str) -> (Option<Option<Vec<String>>>, Vec<String>) {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return (None, Vec::new());
    }
    match norm(trimmed).as_str() {
        "全局" | "跟随全局" | "默认" | "全部" | "全平台" | "all" => {
            return (Some(None), Vec::new());
        }
        _ => {}
    }
    let mut codes: Vec<String> = Vec::new();
    let mut unknown: Vec<String> = Vec::new();
    for tok in trimmed.split(|c: char| {
        c.is_whitespace() || matches!(c, '、' | ',' | '，' | ';' | '；' | '/' | '|' | '｜')
    }) {
        let t = tok.trim();
        if t.is_empty() {
            continue;
        }
        match Platform::from_zh(t).or_else(|| Platform::from_code(&t.to_lowercase())) {
            Some(p) => {
                let c = p.code().to_string();
                if !codes.contains(&c) {
                    codes.push(c);
                }
            }
            None => unknown.push(t.to_string()),
        }
    }
    if codes.is_empty() {
        (None, unknown)
    } else {
        (Some(Some(codes)), unknown)
    }
}

/// 分隔符探测：优先 Tab，其次半角逗号，最后全角逗号。
fn detect_delimiter(text: &str) -> char {
    let probe = text
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or_default();
    if probe.contains('\t') {
        '\t'
    } else if probe.contains(',') {
        ','
    } else if probe.contains('，') {
        '，'
    } else {
        ','
    }
}

/// RFC4180 风格切分：支持 `"..."` 包裹、`""` 转义、字段内换行；返回 (记录起始行号, 各列)。
fn parse_delimited(text: &str, delim: char) -> Table {
    let mut out: Table = Vec::new();
    let mut cur: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut line = 1usize;
    let mut rec_line = 1usize;
    let mut chars = text.chars().peekable();

    while let Some(c) = chars.next() {
        if in_quotes {
            match c {
                '"' => {
                    if chars.peek() == Some(&'"') {
                        chars.next();
                        field.push('"');
                    } else {
                        in_quotes = false;
                    }
                }
                '\n' => {
                    line += 1;
                    field.push('\n');
                }
                _ => field.push(c),
            }
            continue;
        }
        match c {
            '"' if field.trim().is_empty() => {
                field.clear();
                in_quotes = true;
            }
            '\r' => {}
            '\n' => {
                cur.push(std::mem::take(&mut field));
                out.push((rec_line, std::mem::take(&mut cur)));
                line += 1;
                rec_line = line;
            }
            c if c == delim => cur.push(std::mem::take(&mut field)),
            _ => field.push(c),
        }
    }
    if !field.is_empty() || !cur.is_empty() {
        cur.push(field);
        out.push((rec_line, cur));
    }
    out
}

/// 行表：(源文件行号, 各列文本)。
type Table = Vec<(usize, Vec<String>)>;

/// xlsx 首个工作表 → 行表（行号 = 表内 1-based 行）。
fn read_xlsx(path: &Path) -> AppResult<Table> {
    let mut wb: Xlsx<_> = calamine::open_workbook(path)
        .map_err(|e| AppError::InvalidInput(format!("读取 xlsx 失败：{e}")))?;
    let name = wb
        .sheet_names()
        .first()
        .cloned()
        .ok_or_else(|| AppError::InvalidInput("xlsx 中没有工作表".into()))?;
    let range = wb
        .worksheet_range(&name)
        .map_err(|e| AppError::InvalidInput(format!("读取工作表「{name}」失败：{e}")))?;
    Ok(range
        .rows()
        .enumerate()
        .map(|(i, row)| {
            let cells = row
                .iter()
                .map(|d| match d {
                    Data::Empty => String::new(),
                    other => other.to_string().trim().to_string(),
                })
                .collect();
            (i + 1, cells)
        })
        .collect())
}

/// 读文件 → (编码名, 行表)。按扩展名分流 xlsx / 分隔符文本。
fn read_file(path: &Path) -> AppResult<(String, Table)> {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if ext == "xlsx" || ext == "xlsm" {
        return Ok(("XLSX".into(), read_xlsx(path)?));
    }
    let bytes = std::fs::read(path)?;
    let (encoding, text) = importer::decode(&bytes);
    let text = text.trim_start_matches('\u{feff}').to_string();
    let delim = detect_delimiter(&text);
    Ok((encoding, parse_delimited(&text, delim)))
}

/// 首行是否表头：首列即编码列，或识别出 ≥2 个已知列名。
fn header_columns(cells: &[String]) -> Option<Vec<Option<Field>>> {
    let fields: Vec<Option<Field>> = cells.iter().map(|c| field_of(c.as_str())).collect();
    let known = fields.iter().flatten().count();
    let first_is_code = fields.first().copied().flatten() == Some(Field::Code);
    let has_code = fields.iter().flatten().any(|f| *f == Field::Code);
    if (first_is_code || known >= 2) && has_code {
        Some(fields)
    } else {
        None
    }
}

/// 无表头时的位置约定（兼容旧的三列格式）。
const POSITIONAL: [Field; 3] = [Field::Code, Field::Alias, Field::Topics];

fn cell(cells: &[String], idx: Option<usize>) -> &str {
    idx.and_then(|i| cells.get(i))
        .map(|s| s.trim())
        .unwrap_or("")
}

/// 行表 → 结构化行 + 行级错误。
fn parse_table(rows: Table) -> ParsedMapping {
    let mut out = ParsedMapping::default();
    let mut iter = rows.into_iter().peekable();

    // 跳过前导空行，再判表头。
    let mut columns: Vec<Option<Field>> = POSITIONAL.iter().map(|f| Some(*f)).collect();
    while let Some((_, cells)) = iter.peek() {
        if cells.iter().all(|c| c.trim().is_empty()) {
            iter.next();
            continue;
        }
        if let Some(cols) = header_columns(cells) {
            columns = cols;
            out.had_header = true;
            iter.next();
        }
        break;
    }
    let idx_of = |f: Field| columns.iter().position(|c| *c == Some(f));
    let (i_code, i_style, i_product, i_alias, i_topics, i_tier, i_platforms, i_status, i_note) = (
        idx_of(Field::Code),
        idx_of(Field::StyleName),
        idx_of(Field::ProductName),
        idx_of(Field::Alias),
        idx_of(Field::Topics),
        idx_of(Field::Tier),
        idx_of(Field::Platforms),
        idx_of(Field::Status),
        idx_of(Field::Note),
    );

    let mut seen_codes: Vec<String> = Vec::new();
    for (line, cells) in iter {
        if cells.iter().all(|c| c.trim().is_empty()) {
            continue;
        }
        if cells.first().map(|c| c.trim_start().starts_with("//")) == Some(true) {
            continue;
        }
        let code = cell(&cells, i_code).to_string();
        if code.is_empty() {
            out.errors.push(format!("第 {line} 行：缺 SKU 编码"));
            continue;
        }
        if !paths::is_valid_sku_code(&code) {
            out.errors.push(format!(
                "第 {line} 行：编码「{code}」非法（只能是字母、数字与 - _ .，且无空格）"
            ));
            continue;
        }
        if seen_codes.iter().any(|c| c == &code) {
            out.errors.push(format!(
                "第 {line} 行：文件内编码重复「{code}」，已跳过本行"
            ));
            continue;
        }
        seen_codes.push(code.clone());

        let opt = |s: &str| {
            let t = s.trim();
            (!t.is_empty()).then(|| t.to_string())
        };
        let tier_cell = cell(&cells, i_tier);
        let tier = opt(tier_cell).and_then(|s| {
            let t = parse_tier(&s);
            if t.is_none() {
                out.errors
                    .push(format!("第 {line} 行：分层「{s}」无法识别，已忽略该格"));
            }
            t
        });
        let status_cell = cell(&cells, i_status);
        let status = opt(status_cell).and_then(|s| {
            let st = parse_status(&s);
            if st.is_none() {
                out.errors
                    .push(format!("第 {line} 行：状态「{s}」无法识别，已忽略该格"));
            }
            st
        });
        let topics = opt(cell(&cells, i_topics))
            .map(|s| parse_topics(&s))
            .filter(|t| !t.is_empty());
        let (platforms, unknown) = parse_platforms(cell(&cells, i_platforms));
        if !unknown.is_empty() {
            out.errors.push(format!(
                "第 {line} 行：平台「{}」无法识别，已忽略",
                unknown.join(" ")
            ));
        }

        out.rows.push(MappingRow {
            line,
            code,
            style_name: opt(cell(&cells, i_style)),
            product_name: opt(cell(&cells, i_product)),
            alias: opt(cell(&cells, i_alias)),
            topics,
            tier,
            platforms,
            status,
            note: opt(cell(&cells, i_note)),
        });
    }
    out
}

/// 解析映射表文件（不落库）。
pub fn parse_mapping_file(path: &Path) -> AppResult<ParsedMapping> {
    let (encoding, rows) = read_file(path)?;
    let mut parsed = parse_table(rows);
    parsed.encoding = encoding;
    Ok(parsed)
}

/// 模板 CSV（带 UTF-8 BOM，Excel 双击即正确显示中文）。
pub fn template_csv() -> String {
    let mut s = String::from("\u{feff}");
    s.push_str(&TEMPLATE_HEADER.join(","));
    s.push('\n');
    s.push_str(
        "NFC-W-01,敖瑞鹏01,真丝衬衫,A-敖瑞鹏-01,沙发 家居,热款,,在售,示例行（可整行删除）\n",
    );
    s.push_str("NFC-W-02,敖瑞鹏02,,B-敖瑞鹏-02,,,小红书 抖音,,留空的格子不会覆盖库里已有的值\n");
    s
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即测试失败，是期望行为
mod tests {
    use super::*;

    fn rows_of(text: &str) -> ParsedMapping {
        let delim = detect_delimiter(text);
        parse_table(parse_delimited(text, delim))
    }

    #[test]
    fn positional_three_columns_without_header() {
        let p = rows_of("NFC-W-01,A-敖瑞鹏-01,#沙发 #家居\nNFC-W-02,B-01,客厅\n");
        assert!(!p.had_header);
        assert_eq!(p.rows.len(), 2);
        assert_eq!(p.rows[0].code, "NFC-W-01");
        assert_eq!(p.rows[0].alias.as_deref(), Some("A-敖瑞鹏-01"));
        assert_eq!(
            p.rows[0].topics.as_deref(),
            Some(["沙发".to_string(), "家居".to_string()].as_slice())
        );
        assert_eq!(p.rows[0].line, 1);
        assert_eq!(p.rows[1].line, 2);
    }

    #[test]
    fn tab_separated_is_detected() {
        let p = rows_of("SKU编码\t文件夹别名\t话题\nNFC-W-01\tA-敖瑞鹏-01\t沙发\n");
        assert!(p.had_header);
        assert_eq!(p.rows[0].alias.as_deref(), Some("A-敖瑞鹏-01"));
    }

    #[test]
    fn header_maps_columns_in_any_order() {
        let text = "备注,话题,SKU 编码,文件夹别名,分层\n注释,沙发 家居,NFC-W-01,A-敖瑞鹏-01,热款\n";
        let p = rows_of(text);
        assert!(p.had_header);
        let r = &p.rows[0];
        assert_eq!(r.code, "NFC-W-01");
        assert_eq!(r.alias.as_deref(), Some("A-敖瑞鹏-01"));
        assert_eq!(r.tier.as_deref(), Some("hot"));
        assert_eq!(r.note.as_deref(), Some("注释"));
        assert_eq!(r.line, 2);
        assert!(p.errors.is_empty());
    }

    #[test]
    fn empty_cells_stay_none_so_import_never_overwrites() {
        let p = rows_of("SKU编码,款式名,文件夹别名,话题\nNFC-W-01,,,\n");
        let r = &p.rows[0];
        assert_eq!(r.code, "NFC-W-01");
        assert!(r.style_name.is_none());
        assert!(r.alias.is_none());
        assert!(r.topics.is_none());
        assert!(r.platforms.is_none());
    }

    #[test]
    fn quoted_field_keeps_comma_and_newline() {
        let p = rows_of("SKU编码,备注\nNFC-W-01,\"逗号, 与\n换行\"\n");
        assert_eq!(p.rows.len(), 1);
        assert_eq!(p.rows[0].note.as_deref(), Some("逗号, 与\n换行"));
    }

    #[test]
    fn quoted_field_unescapes_doubled_quotes() {
        let p = rows_of("SKU编码,备注\nNFC-W-01,\"他说\"\"好\"\"\"\n");
        assert_eq!(p.rows[0].note.as_deref(), Some("他说\"好\""));
    }

    #[test]
    fn invalid_and_duplicate_codes_become_errors() {
        let p = rows_of("SKU编码,文件夹别名\nNFC W 01,A-01\nNFC-W-02,B-01\nNFC-W-02,C-01\n,D-01\n");
        assert_eq!(p.rows.len(), 1, "只有 NFC-W-02 首次出现这一行可导入");
        assert_eq!(p.errors.len(), 3);
        assert!(p.errors[0].contains("非法"));
        assert!(p.errors[1].contains("重复"));
        assert!(p.errors[2].contains("缺 SKU 编码"));
    }

    #[test]
    fn platforms_zh_codes_and_global() {
        assert_eq!(
            parse_platforms("小红书、抖音"),
            (
                Some(Some(vec!["xhs".to_string(), "douyin".to_string()])),
                vec![]
            )
        );
        assert_eq!(parse_platforms("跟随全局"), (Some(None), vec![]));
        assert_eq!(parse_platforms(""), (None, vec![]));
        let (p, unknown) = parse_platforms("小红书 微博");
        assert_eq!(p, Some(Some(vec!["xhs".to_string()])));
        assert_eq!(unknown, vec!["微博".to_string()]);
    }

    #[test]
    fn tier_and_status_accept_zh_and_en() {
        assert_eq!(parse_tier("热款").as_deref(), Some("hot"));
        assert_eq!(parse_tier("冷").as_deref(), Some("cold"));
        assert_eq!(parse_tier("WARM").as_deref(), Some("warm"));
        assert!(parse_tier("爆款").is_none());
        assert_eq!(parse_status("停发").as_deref(), Some("paused"));
        assert_eq!(parse_status("在售").as_deref(), Some("active"));
    }

    #[test]
    fn topics_strip_hash_dedupe_cap_five() {
        assert_eq!(parse_topics("#沙发 #家居、沙发"), ["沙发", "家居"]);
        assert_eq!(parse_topics("a b c d e f g").len(), 5);
        assert!(parse_topics("  ").is_empty());
    }

    #[test]
    fn gbk_bytes_decode_to_chinese() {
        let (bytes, _, _) = encoding_rs::GBK.encode("SKU编码,文件夹别名\nNFC-W-01,A-敖瑞鹏-01\n");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("m.csv");
        std::fs::write(&path, &bytes[..]).unwrap();
        let p = parse_mapping_file(&path).unwrap();
        assert_eq!(p.rows[0].alias.as_deref(), Some("A-敖瑞鹏-01"));
    }

    #[test]
    fn template_has_bom_and_all_columns() {
        let t = template_csv();
        assert!(t.starts_with('\u{feff}'));
        let p = rows_of(t.trim_start_matches('\u{feff}'));
        assert!(p.had_header);
        assert_eq!(p.rows.len(), 2, "两行示例");
        assert_eq!(p.rows[0].tier.as_deref(), Some("hot"));
        assert_eq!(
            p.rows[1].platforms,
            Some(Some(vec!["xhs".into(), "douyin".into()]))
        );
    }
}
