//! 收件箱 TXT 解析（收件箱收录格式规范 §2）。纯函数、任意字节输入不 panic。
//!
//! 三类：标题（一行一条）/ 正文（`====` 分隔）/ 图文套装（【标题】+【正文】成套，`====` 分隔）。
//! 头部字段以全角括号【】标记；【话题】取前 5；SKU 三冗余识别（头 > 文件名前缀 > 文件夹名）。

use crate::publish::paths::is_valid_sku_code;

/// 多套之间 / 多条正文之间的分隔行。
const SEP: &str = "====";
/// 【话题】最多采纳个数。
const MAX_TOPICS: usize = 5;

/// TXT 类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxtKind {
    Title,
    Body,
    Combo,
}

impl TxtKind {
    /// 由【类型】头或文件名类型前缀解析。
    pub fn from_zh(s: &str) -> Option<TxtKind> {
        match s.trim() {
            "标题" => Some(TxtKind::Title),
            "正文" => Some(TxtKind::Body),
            "图文" => Some(TxtKind::Combo),
            _ => None,
        }
    }

    /// 存储用短码（inbox_items.kind）。
    pub fn code(self) -> &'static str {
        match self {
            TxtKind::Title => "title",
            TxtKind::Body => "body",
            TxtKind::Combo => "combo",
        }
    }
}

/// 解析结果。titles/bodies 对图文成对（同序）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedTxt {
    pub sku_code: Option<String>,
    /// 平台原文（中文名或空；标签规范化由 platform::text_platform_tag 处理）。
    pub platform: Option<String>,
    pub kind: TxtKind,
    pub topics: Vec<String>,
    pub titles: Vec<String>,
    pub bodies: Vec<String>,
}

/// 解析失败原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// 无法确定类型（无【类型】头且无文件名提示且无法从结构推断）。
    UnknownKind,
    /// 有类型但无任何有效条目。
    Empty,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::UnknownKind => write!(f, "无法确定文件类型（缺【类型】头与文件名提示）"),
            ParseError::Empty => write!(f, "文件无有效标题/正文内容"),
        }
    }
}

/// 提取一行的头部字段：`【KEY】VALUE` → `(KEY, VALUE)`（去首尾空白）。
fn header_field(line: &str) -> Option<(&str, &str)> {
    let t = line.trim();
    let rest = t.strip_prefix('【')?;
    let (key, after) = rest.split_once('】')?;
    Some((key.trim(), after.trim()))
}

/// 顶部头部字段键（在 body 起始前出现）。【标题】【正文】是内容标记，不在此列。
fn is_top_header_key(key: &str) -> bool {
    matches!(key, "SKU" | "平台" | "类型" | "话题")
}

/// 解析【话题】值：`#标签1 #标签2` → 去 `#`、去空、取前 5。
fn parse_topics(value: &str) -> Vec<String> {
    value
        .split_whitespace()
        .map(|t| t.trim_start_matches('#').trim().to_string())
        .filter(|t| !t.is_empty())
        .take(MAX_TOPICS)
        .collect()
}

/// 解析 TXT。`kind_hint` 来自文件名类型前缀，头部【类型】优先于它。
pub fn parse(content: &str, kind_hint: Option<TxtKind>) -> Result<ParsedTxt, ParseError> {
    let lines: Vec<&str> = content.lines().collect();

    // 1) 消费顶部头部字段（SKU/平台/类型/话题）与空行，直到第一行 body 内容。
    let mut sku_code: Option<String> = None;
    let mut platform: Option<String> = None;
    let mut kind_header: Option<TxtKind> = None;
    let mut topics: Vec<String> = Vec::new();
    let mut idx = 0;
    while idx < lines.len() {
        let line = lines[idx];
        if line.trim().is_empty() {
            idx += 1;
            continue;
        }
        match header_field(line) {
            Some((key, value)) if is_top_header_key(key) => {
                match key {
                    "SKU" => {
                        let v = value.trim();
                        if !v.is_empty() {
                            sku_code = Some(v.to_string());
                        }
                    }
                    "平台" => {
                        let v = value.trim();
                        if !v.is_empty() {
                            platform = Some(v.to_string());
                        }
                    }
                    "类型" => kind_header = TxtKind::from_zh(value),
                    "话题" => topics = parse_topics(value),
                    _ => {}
                }
                idx += 1;
            }
            // 非顶部头部字段（含【标题】/【正文】或普通正文）→ body 开始。
            _ => break,
        }
    }

    let body_lines = &lines[idx..];

    // 2) 确定类型：头 > 文件名提示 > 结构推断。
    let kind = kind_header
        .or(kind_hint)
        .or_else(|| infer_kind(body_lines))
        .ok_or(ParseError::UnknownKind)?;

    // 3) 按类型抽取条目。
    let (titles, bodies) = match kind {
        TxtKind::Title => {
            let titles: Vec<String> = body_lines
                .iter()
                .map(|l| l.trim())
                .filter(|l| !l.is_empty())
                .map(str::to_string)
                .collect();
            (titles, Vec::new())
        }
        TxtKind::Body => {
            let bodies = split_blocks(body_lines)
                .into_iter()
                .filter(|b| !b.is_empty())
                .collect();
            (Vec::new(), bodies)
        }
        TxtKind::Combo => {
            let mut titles = Vec::new();
            let mut bodies = Vec::new();
            for block in split_blocks_lines(body_lines) {
                if let Some((t, b)) = parse_combo_block(&block) {
                    titles.push(t);
                    bodies.push(b);
                }
            }
            (titles, bodies)
        }
    };

    if titles.is_empty() && bodies.is_empty() {
        return Err(ParseError::Empty);
    }

    Ok(ParsedTxt {
        sku_code,
        platform,
        kind,
        topics,
        titles,
        bodies,
    })
}

/// 无【类型】头也无文件名提示时的结构推断：出现【标题】/【正文】标记 → 图文；
/// 出现 `====` → 正文；否则 → 标题。
fn infer_kind(body_lines: &[&str]) -> Option<TxtKind> {
    if body_lines.iter().any(|l| {
        let t = l.trim();
        t.starts_with("【标题】") || t.starts_with("【正文】")
    }) {
        return Some(TxtKind::Combo);
    }
    if body_lines.iter().any(|l| l.trim() == SEP) {
        return Some(TxtKind::Body);
    }
    if body_lines.iter().any(|l| !l.trim().is_empty()) {
        return Some(TxtKind::Title);
    }
    None
}

/// 按 `====` 分块并把每块内容 trim 成字符串。
fn split_blocks(body_lines: &[&str]) -> Vec<String> {
    split_blocks_lines(body_lines)
        .into_iter()
        .map(|block| block.join("\n").trim().to_string())
        .collect()
}

/// 按 `====` 分块，保留每块的原始行（图文块二次解析用）。
fn split_blocks_lines<'a>(body_lines: &[&'a str]) -> Vec<Vec<&'a str>> {
    let mut blocks = Vec::new();
    let mut cur: Vec<&str> = Vec::new();
    for &line in body_lines {
        if line.trim() == SEP {
            blocks.push(std::mem::take(&mut cur));
        } else {
            cur.push(line);
        }
    }
    blocks.push(cur);
    // 丢弃完全空的块。
    blocks
        .into_iter()
        .filter(|b| b.iter().any(|l| !l.trim().is_empty()))
        .collect()
}

/// 解析图文块：`【标题】…` + `【正文】` 后接正文。缺任一 → None（该套无效）。
fn parse_combo_block(block: &[&str]) -> Option<(String, String)> {
    let mut title: Option<String> = None;
    let mut body_lines: Vec<&str> = Vec::new();
    let mut in_body = false;
    for &line in block {
        if in_body {
            body_lines.push(line);
            continue;
        }
        match header_field(line) {
            Some(("标题", v)) => title = Some(v.trim().to_string()),
            Some(("正文", v)) => {
                in_body = true;
                if !v.trim().is_empty() {
                    body_lines.push(v);
                }
            }
            _ => {
                // 【标题】之前的杂散行忽略；若已有标题但正文未标记，视为正文前导忽略。
            }
        }
    }
    let title = title?;
    let body = body_lines.join("\n").trim().to_string();
    if title.is_empty() || body.is_empty() {
        return None;
    }
    Some((title, body))
}

/// 从文件名类型前缀推断类型：`标题_…` / `正文_…` / `图文_…`。
pub fn kind_from_filename(filename: &str) -> Option<TxtKind> {
    let stem = filename.rsplit('/').next().unwrap_or(filename);
    let first = stem.split(['_', '.']).next().unwrap_or("");
    TxtKind::from_zh(first)
}

/// SKU 三冗余识别：头【SKU】> 文件名前缀 > 文件夹名（前置事实 §3.6）。
/// 纯提取候选编码；是否为已知 SKU 由 ingest 查库判定。
pub fn resolve_sku(
    header_sku: Option<&str>,
    filename: &str,
    folder: Option<&str>,
) -> Option<String> {
    if let Some(h) = header_sku.map(str::trim).filter(|s| !s.is_empty()) {
        return Some(h.to_string());
    }
    // 文件名前缀：取首段，排除类型关键字，需像 SKU code。
    let stem = filename.rsplit('/').next().unwrap_or(filename);
    let first = stem.split(['_', '.']).next().unwrap_or("");
    if TxtKind::from_zh(first).is_none() && is_valid_sku_code(first) {
        return Some(first.to_string());
    }
    folder
        .map(str::trim)
        .filter(|f| is_valid_sku_code(f))
        .map(str::to_string)
}

/// 反序列化为规范格式（round-trip 测试用）。
#[cfg(test)]
pub fn serialize(p: &ParsedTxt) -> String {
    let mut out = String::new();
    if let Some(sku) = &p.sku_code {
        out.push_str(&format!("【SKU】{sku}\n"));
    }
    if let Some(pf) = &p.platform {
        out.push_str(&format!("【平台】{pf}\n"));
    }
    let kind_zh = match p.kind {
        TxtKind::Title => "标题",
        TxtKind::Body => "正文",
        TxtKind::Combo => "图文",
    };
    out.push_str(&format!("【类型】{kind_zh}\n"));
    if !p.topics.is_empty() {
        let tags = p
            .topics
            .iter()
            .map(|t| format!("#{t}"))
            .collect::<Vec<_>>()
            .join(" ");
        out.push_str(&format!("【话题】{tags}\n"));
    }
    out.push('\n');
    match p.kind {
        TxtKind::Title => {
            out.push_str(&p.titles.join("\n"));
        }
        TxtKind::Body => {
            out.push_str(&p.bodies.join(&format!("\n\n{SEP}\n\n")));
        }
        TxtKind::Combo => {
            let sets: Vec<String> = p
                .titles
                .iter()
                .zip(p.bodies.iter())
                .map(|(t, b)| format!("【标题】{t}\n【正文】\n{b}"))
                .collect();
            out.push_str(&sets.join(&format!("\n\n{SEP}\n\n")));
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即测试失败，是期望行为
mod tests {
    use super::*;

    const TITLE_SAMPLE: &str = "【SKU】SF-YD-201\n【平台】小红书\n【类型】标题\n\n\
        小户型也能拥有的云朵感沙发\n坐下去的瞬间就不想起来了\n\n新家软装的第一件大件，选它\n";

    const BODY_SAMPLE: &str = "【SKU】SF-YD-201\n【平台】小红书\n【类型】正文\n\n\
        第一条正文……\n可以多行。\n\n====\n\n第二条正文……\n";

    const COMBO_SAMPLE: &str = "【SKU】SF-YD-201\n【平台】小红书\n【类型】图文\n\n\
        【标题】小户型也能拥有的云朵感沙发\n【正文】\n正文内容……\n可以多行。\n\n====\n\n\
        【标题】第二套的标题\n【正文】\n第二套的正文……\n";

    #[test]
    fn parses_title_sample() {
        let p = parse(TITLE_SAMPLE, None).unwrap();
        assert_eq!(p.kind, TxtKind::Title);
        assert_eq!(p.sku_code.as_deref(), Some("SF-YD-201"));
        assert_eq!(p.platform.as_deref(), Some("小红书"));
        assert_eq!(p.titles.len(), 3);
        assert_eq!(p.titles[2], "新家软装的第一件大件，选它");
        assert!(p.bodies.is_empty());
    }

    #[test]
    fn parses_body_sample() {
        let p = parse(BODY_SAMPLE, None).unwrap();
        assert_eq!(p.kind, TxtKind::Body);
        assert_eq!(p.bodies.len(), 2);
        assert_eq!(p.bodies[0], "第一条正文……\n可以多行。");
        assert_eq!(p.bodies[1], "第二条正文……");
    }

    #[test]
    fn parses_combo_sample_splits_pairs() {
        let p = parse(COMBO_SAMPLE, None).unwrap();
        assert_eq!(p.kind, TxtKind::Combo);
        assert_eq!(p.titles, vec!["小户型也能拥有的云朵感沙发", "第二套的标题"]);
        assert_eq!(p.bodies, vec!["正文内容……\n可以多行。", "第二套的正文……"]);
    }

    #[test]
    fn topic_line_takes_first_five() {
        let src = "【SKU】X\n【类型】标题\n【话题】#a #b #c #d #e #f #g\n\n标题一\n";
        let p = parse(src, None).unwrap();
        assert_eq!(p.topics, vec!["a", "b", "c", "d", "e"]);
    }

    #[test]
    fn kind_hint_used_when_no_header() {
        let src = "【SKU】X\n\n只有一行\n";
        let p = parse(src, Some(TxtKind::Title)).unwrap();
        assert_eq!(p.kind, TxtKind::Title);
        assert_eq!(p.titles, vec!["只有一行"]);
    }

    #[test]
    fn infer_kind_from_structure() {
        // 有 【标题】 → combo
        assert_eq!(
            parse("【标题】T\n【正文】\nB\n", None).unwrap().kind,
            TxtKind::Combo
        );
        // 有 ==== → body
        assert_eq!(parse("a\n====\nb\n", None).unwrap().kind, TxtKind::Body);
        // 纯行 → title
        assert_eq!(parse("just a line\n", None).unwrap().kind, TxtKind::Title);
    }

    #[test]
    fn empty_and_unknown_errors() {
        // 无内容
        assert_eq!(parse("\n\n", Some(TxtKind::Title)), Err(ParseError::Empty));
        // 无类型头/提示 + 全空 → 结构无法推断 → UnknownKind
        assert_eq!(parse("   \n", None), Err(ParseError::UnknownKind));
        // combo 缺正文 → 该套无效 → Empty
        assert_eq!(
            parse("【类型】图文\n\n【标题】只有标题\n", None),
            Err(ParseError::Empty)
        );
    }

    #[test]
    fn resolve_sku_priority() {
        // 头优先
        assert_eq!(
            resolve_sku(Some("SF-YD-201"), "标题_小红书.txt", Some("OTHER")),
            Some("SF-YD-201".to_string())
        );
        // 无头 → 文件名前缀（类型关键字被排除，回退文件夹）
        assert_eq!(
            resolve_sku(None, "标题_小红书.txt", Some("SF-YD-201")),
            Some("SF-YD-201".to_string())
        );
        // 文件名带 SKU 前缀
        assert_eq!(
            resolve_sku(None, "SF-YD-9_标题.txt", None),
            Some("SF-YD-9".to_string())
        );
        // 都无 → None
        assert_eq!(resolve_sku(None, "标题_小红书.txt", None), None);
        // 空头回退到文件夹（文件名前缀是类型关键字被排除）
        assert_eq!(
            resolve_sku(Some("  "), "标题_x.txt", Some("AB-1")),
            Some("AB-1".to_string())
        );
    }

    #[test]
    fn kind_from_filename_prefix() {
        assert_eq!(kind_from_filename("标题_小红书.txt"), Some(TxtKind::Title));
        assert_eq!(kind_from_filename("a/正文_通用.txt"), Some(TxtKind::Body));
        assert_eq!(kind_from_filename("图文_抖音.txt"), Some(TxtKind::Combo));
        assert_eq!(kind_from_filename("random.txt"), None);
    }

    #[test]
    fn roundtrip_all_samples() {
        for src in [TITLE_SAMPLE, BODY_SAMPLE, COMBO_SAMPLE] {
            let p1 = parse(src, None).unwrap();
            let re = serialize(&p1);
            let p2 = parse(&re, None).unwrap();
            assert_eq!(p1, p2, "round-trip 不一致:\n{re}");
        }
    }

    // proptest：任意字节输入不 panic（发布模块执行计划 §6.2）。
    proptest::proptest! {
        #[test]
        fn parse_never_panics(bytes: Vec<u8>) {
            let s = String::from_utf8_lossy(&bytes);
            let _ = parse(&s, None);
            let _ = parse(&s, Some(TxtKind::Title));
            let _ = parse(&s, Some(TxtKind::Body));
            let _ = parse(&s, Some(TxtKind::Combo));
        }

        // 规范结构 round-trip：任意标题集合解析后可再解析一致。
        #[test]
        fn title_roundtrip(titles in proptest::collection::vec("[^\n【】]{1,20}", 1..8)) {
            let trimmed: Vec<String> = titles.iter().map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty()).collect();
            proptest::prop_assume!(!trimmed.is_empty());
            let p = ParsedTxt {
                sku_code: Some("SF-1".into()), platform: Some("小红书".into()),
                kind: TxtKind::Title, topics: vec![], titles: trimmed.clone(), bodies: vec![],
            };
            let re = serialize(&p);
            let back = parse(&re, None).unwrap();
            proptest::prop_assert_eq!(back.titles, trimmed);
        }
    }
}
