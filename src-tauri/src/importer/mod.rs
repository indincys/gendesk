//! 提示词 txt 导入（执行计划 1.6 / 需求 6.4）。
//!
//! 解析「分组 / 前缀 / 场景 / 标签 / 小标题 / 正文」字段 + UTF-8/GBK 编码探测；
//! 两段式：`parse`（纯函数，不落库）→ 命令层 `commit`（落库 + 号池发放）。
//!
//! 格式约定（宽容解析，两种写法并存）：
//! - 分组头：`分组: 名称`（半/全角冒号）**或** `分组【名称】`（括号内联）→ 开启新分组；
//!   一个文件含多个 `分组` 头即按分组自动拆分。
//! - 其它头部行 `前缀:`/`场景:`/`标签:` 设置当前分组元信息；
//! - 独占一行的 `【小标题】` → 作为紧随其后那条提示词的小标题；
//! - 正文行前导序号（`1.`/`2、`/`3）` 等）自动剥离；
//! - 其余每条非空行 = 一条提示词（一行一提示词，空行忽略）；
//! - 缺分组 → 归入默认分组「未分组导入」；缺前缀 → 由 commit 阶段自动分配。

/// 单条解析出的提示词（正文 + 可选小标题）。
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedPrompt {
    /// 来自 `【小标题】` 行；无则 None。
    pub title: Option<String>,
    pub text: String,
}

/// 单个解析出的分组。
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedGroup {
    pub name: String,
    pub prefix: Option<String>,
    pub scene: String,
    pub tags: Vec<String>,
    pub prompts: Vec<ParsedPrompt>,
}

/// 解析诊断（E37：行号级报错/提示，非致命）。
#[derive(Debug, Clone, PartialEq)]
pub struct ParseWarning {
    /// 1-based 行号。
    pub line: usize,
    pub message: String,
}

/// 解析结果。
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedImport {
    /// 探测到的编码名（如 "UTF-8" / "GBK"）。
    pub encoding: String,
    pub groups: Vec<ParsedGroup>,
    /// 非致命诊断（缺分组标记、悬空小标题等），含行号。
    pub warnings: Vec<ParseWarning>,
}

impl ParsedImport {
    pub fn total_prompts(&self) -> usize {
        self.groups.iter().map(|g| g.prompts.len()).sum()
    }
}

const DEFAULT_GROUP: &str = "未分组导入";

/// 探测编码并解码为字符串。
pub fn decode(bytes: &[u8]) -> (String, String) {
    let mut detector = chardetng::EncodingDetector::new();
    detector.feed(bytes, true);
    let enc = detector.guess(None, true);
    let (cow, _, _) = enc.decode(bytes);
    (enc.name().to_string(), cow.into_owned())
}

/// 解析字节流为结构化导入结果（不落库、不 panic）。
pub fn parse(bytes: &[u8]) -> ParsedImport {
    let (encoding, text) = decode(bytes);
    let (groups, warnings) = parse_text(&text);
    ParsedImport {
        encoding,
        groups,
        warnings,
    }
}

fn parse_text(text: &str) -> (Vec<ParsedGroup>, Vec<ParseWarning>) {
    let mut groups: Vec<ParsedGroup> = Vec::new();
    let mut cur: Option<ParsedGroup> = None;
    let mut warnings: Vec<ParseWarning> = Vec::new();
    // 待挂靠的小标题：遇到 `【小标题】` 行后暂存，附加到下一条正文。含行号供悬空诊断。
    let mut pending_title: Option<(String, usize)> = None;
    // 是否已见过任一「分组」标记（E37：正文出现在分组标记前时告警一次）。
    let mut seen_group_header = false;
    let mut warned_missing_group = false;

    // 确保存在「当前分组」；否则建默认分组。
    fn ensure(cur: &mut Option<ParsedGroup>) -> &mut ParsedGroup {
        cur.get_or_insert_with(|| ParsedGroup {
            name: DEFAULT_GROUP.to_string(),
            prefix: None,
            scene: String::new(),
            tags: Vec::new(),
            prompts: Vec::new(),
        })
    }

    // 解析模型：每条非空、非头部/非小标题行 = 一条提示词（一行一提示词）。
    // 空行仅作视觉分隔，被忽略。行号 1-based。
    for (idx, raw) in text.lines().enumerate() {
        let line = idx + 1;
        let trimmed = raw.trim_end_matches('\r').trim();
        if trimmed.is_empty() {
            continue;
        }

        // 分组头：`分组: 名称` 或 `分组【名称】`（后者优先于「小标题」判定）。
        if let Some(name) = parse_group_header(trimmed) {
            if let Some((t, tline)) = pending_title.take() {
                warnings.push(ParseWarning {
                    line: tline,
                    message: format!("小标题「{t}」后没有正文，已忽略。"),
                });
            }
            if let Some(g) = cur.take() {
                groups.push(g);
            }
            cur = Some(ParsedGroup {
                name,
                prefix: None,
                scene: String::new(),
                tags: Vec::new(),
                prompts: Vec::new(),
            });
            seen_group_header = true;
            continue;
        }

        if let Some((key, value)) = parse_header(trimmed) {
            match key {
                Header::Prefix => ensure(&mut cur).prefix = Some(value.to_uppercase()),
                Header::Scene => ensure(&mut cur).scene = value.to_string(),
                Header::Tags => ensure(&mut cur).tags = split_tags(value),
            }
            continue;
        }

        // 独占一行的 `【小标题】` → 暂存给下一条正文。
        if let Some(title) = parse_title_line(trimmed) {
            pending_title = Some((title, line));
            continue;
        }

        // 正文行：若此前从未见过「分组」标记，告警一次（含行号）。
        if !seen_group_header && !warned_missing_group {
            warnings.push(ParseWarning {
                line,
                message: format!(
                    "此行正文出现在任何「分组」标记之前，已归入默认分组「{DEFAULT_GROUP}」。\
                     可在文件开头加一行「分组: 名称」。"
                ),
            });
            warned_missing_group = true;
        }
        ensure(&mut cur).prompts.push(ParsedPrompt {
            title: pending_title.take().map(|(t, _)| t),
            text: strip_leading_number(trimmed).to_string(),
        });
    }

    // 文件结尾仍有未挂靠的小标题。
    if let Some((t, tline)) = pending_title.take() {
        warnings.push(ParseWarning {
            line: tline,
            message: format!("小标题「{t}」后没有正文，已忽略。"),
        });
    }

    if let Some(g) = cur.take() {
        groups.push(g);
    }
    // 丢弃完全为空（无提示词）的分组。
    groups.retain(|g| !g.prompts.is_empty());
    (groups, warnings)
}

/// 识别分组头：`分组: 名称` / `分组：名称` / `分组【名称】`（含 `组`/`group` 同义）。
fn parse_group_header(line: &str) -> Option<String> {
    for kw in ["分组", "组", "group", "Group"] {
        if let Some(rest) = line.strip_prefix(kw) {
            let rest = rest.trim_start();
            // 内联括号形式：分组【名称】
            if let Some(inner) = bracket_inner(rest) {
                let name = inner.trim();
                if !name.is_empty() {
                    return Some(name.to_string());
                }
            }
            // 冒号形式：分组: 名称
            if let Some(after) = rest.strip_prefix(':').or_else(|| rest.strip_prefix('：')) {
                let name = after.trim();
                if !name.is_empty() {
                    return Some(name.to_string());
                }
            }
        }
    }
    None
}

/// 取 `【...】` 中第一个括号块的内部文本（要求以 `【` 开头）。
fn bracket_inner(s: &str) -> Option<&str> {
    let rest = s.strip_prefix('【')?;
    let end = rest.find('】')?;
    Some(&rest[..end])
}

/// 若整行是单个 `【...】` 括号块，返回括号内文本（作为小标题）。
fn parse_title_line(line: &str) -> Option<String> {
    let inner = line.strip_prefix('【')?.strip_suffix('】')?;
    // 内部若还含闭括号，说明不是单一括号块（可能是正文），不当作小标题。
    if inner.contains('】') {
        return None;
    }
    let t = inner.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// 剥离正文前导序号：`1.` / `2、` / `3）` / `4．` 等（要求序号后紧跟分隔符）。
fn strip_leading_number(s: &str) -> &str {
    let digits_end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    if digits_end == 0 {
        return s; // 无前导数字
    }
    let after = &s[digits_end..];
    match after.chars().next() {
        Some(c) if matches!(c, '.' | '．' | '、' | '。' | ')' | '）' | ':' | '：') => {
            after[c.len_utf8()..].trim_start()
        }
        _ => s,
    }
}

enum Header {
    Prefix,
    Scene,
    Tags,
}

/// 识别元信息头部行 `键: 值`（半/全角冒号）。分组头另由 `parse_group_header` 处理。
fn parse_header(line: &str) -> Option<(Header, &str)> {
    let idx = line.find([':', '：'])?;
    let key = line[..idx].trim();
    // 冒号可能是全角（3 字节），用 char 边界安全切分。
    let value = line[idx..]
        .char_indices()
        .nth(1)
        .map(|(off, _)| &line[idx + off..])
        .unwrap_or("")
        .trim();
    let header = match key {
        "前缀" | "prefix" | "Prefix" => Header::Prefix,
        "场景" | "scene" | "Scene" => Header::Scene,
        "标签" | "tag" | "tags" | "Tags" => Header::Tags,
        _ => return None,
    };
    Some((header, value))
}

/// 标签分隔：`, ，、; ；` 与空白。
fn split_tags(value: &str) -> Vec<String> {
    value
        .split(|c: char| matches!(c, ',' | '，' | '、' | ';' | '；') || c.is_whitespace())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即失败
mod tests {
    use super::*;

    /// 测试便捷：把 ParsedPrompt 拍平成 (title, text) 便于断言。
    fn flat(g: &ParsedGroup) -> Vec<(Option<&str>, &str)> {
        g.prompts
            .iter()
            .map(|p| (p.title.as_deref(), p.text.as_str()))
            .collect()
    }
    fn texts(g: &ParsedGroup) -> Vec<&str> {
        g.prompts.iter().map(|p| p.text.as_str()).collect()
    }

    #[test]
    fn parses_full_utf8_document() {
        let doc = "\
分组: 电商主图
前缀: dz
场景: 商品
标签: 白底, 3C、主图

第一条提示词正文。
第二条提示词正文。
";
        let out = parse(doc.as_bytes());
        assert_eq!(out.groups.len(), 1);
        let g = &out.groups[0];
        assert_eq!(g.name, "电商主图");
        assert_eq!(g.prefix.as_deref(), Some("DZ")); // 前缀大写归一
        assert_eq!(g.scene, "商品");
        assert_eq!(g.tags, vec!["白底", "3C", "主图"]);
        assert_eq!(texts(g), vec!["第一条提示词正文。", "第二条提示词正文。"]);
        // 无小标题行时 title 为 None。
        assert!(g.prompts.iter().all(|p| p.title.is_none()));
    }

    #[test]
    fn missing_group_falls_back_to_default() {
        let out = parse("只有一条正文，没有分组头。".as_bytes());
        assert_eq!(out.groups.len(), 1);
        assert_eq!(out.groups[0].name, DEFAULT_GROUP);
        assert_eq!(out.groups[0].prefix, None);
        assert_eq!(out.groups[0].prompts.len(), 1);
    }

    #[test]
    fn missing_tags_and_empty_lines_ok() {
        let doc = "分组: A\n\n\n正文1\n\n\n正文2\n\n";
        let out = parse(doc.as_bytes());
        assert_eq!(out.groups[0].tags.len(), 0);
        assert_eq!(texts(&out.groups[0]), vec!["正文1", "正文2"]);
    }

    #[test]
    fn each_nonempty_line_is_one_prompt() {
        let doc = "分组: A\n第一行\n第二行\n第三行\n\n下一条";
        let out = parse(doc.as_bytes());
        assert_eq!(
            texts(&out.groups[0]),
            vec!["第一行", "第二行", "第三行", "下一条"]
        );
    }

    #[test]
    fn very_long_prompt_over_500_chars() {
        let long = "光".repeat(600);
        let doc = format!("分组: A\n{long}");
        let out = parse(doc.as_bytes());
        assert_eq!(out.groups[0].prompts[0].text.chars().count(), 600);
    }

    #[test]
    fn multiple_groups() {
        let doc = "分组: A\n前缀: AA\na1\n\n分组: B\n前缀: BB\nb1\nb2扩展\n\nb3";
        let out = parse(doc.as_bytes());
        assert_eq!(out.groups.len(), 2);
        assert_eq!(out.groups[0].name, "A");
        assert_eq!(texts(&out.groups[0]), vec!["a1"]);
        assert_eq!(out.groups[1].name, "B");
        assert_eq!(texts(&out.groups[1]), vec!["b1", "b2扩展", "b3"]);
    }

    #[test]
    fn decodes_gbk() {
        // "分组: 商品\n正文一" 的 GBK 字节
        let (bytes, _enc, _had_errors) = encoding_rs::GBK.encode("分组: 商品\n正文一");
        let out = parse(&bytes);
        assert_eq!(out.groups[0].name, "商品");
        assert_eq!(texts(&out.groups[0]), vec!["正文一"]);
    }

    #[test]
    fn fullwidth_colon_supported() {
        let out = parse("分组：全角\n正文".as_bytes());
        assert_eq!(out.groups[0].name, "全角");
    }

    #[test]
    fn bracket_group_header_and_titles() {
        // 用户实际格式：分组【名】 + 每条上方独占一行的【小标题】 + 带前导序号的正文。
        let doc = "\
分组【丁禹兮】

【雷雨拆快递】
1. 把图中这只卡套放进随手拍照片里。

【楼道骑车】
2. 把绿色卡套连同配件放进照片里。
";
        let out = parse(doc.as_bytes());
        assert_eq!(out.groups.len(), 1);
        let g = &out.groups[0];
        assert_eq!(g.name, "丁禹兮");
        assert_eq!(
            flat(g),
            vec![
                (Some("雷雨拆快递"), "把图中这只卡套放进随手拍照片里。"),
                (Some("楼道骑车"), "把绿色卡套连同配件放进照片里。"),
            ]
        );
    }

    #[test]
    fn multiple_bracket_groups_split() {
        let doc = "\
分组【组一】
【标题A】
1. 甲正文
分组【组二】
【标题B】
1、乙正文
【标题C】
2）丙正文
";
        let out = parse(doc.as_bytes());
        assert_eq!(out.groups.len(), 2);
        assert_eq!(out.groups[0].name, "组一");
        assert_eq!(flat(&out.groups[0]), vec![(Some("标题A"), "甲正文")]);
        assert_eq!(out.groups[1].name, "组二");
        assert_eq!(
            flat(&out.groups[1]),
            vec![(Some("标题B"), "乙正文"), (Some("标题C"), "丙正文")]
        );
    }

    #[test]
    fn title_without_number_and_prompt_without_title() {
        let doc = "分组【G】\n【只有标题】\n没有序号的正文\n没有标题也没序号的正文";
        let out = parse(doc.as_bytes());
        assert_eq!(
            flat(&out.groups[0]),
            vec![
                (Some("只有标题"), "没有序号的正文"),
                (None, "没有标题也没序号的正文"),
            ]
        );
    }

    #[test]
    fn embedded_brackets_in_body_not_treated_as_title() {
        // 正文中含括号但非整行括号块 → 不误判为小标题。
        let doc = "分组【G】\n把【绿色】卡套放进照片";
        let out = parse(doc.as_bytes());
        assert_eq!(flat(&out.groups[0]), vec![(None, "把【绿色】卡套放进照片")]);
    }

    // E37：正文出现在任何「分组」标记前 → 告警含首个正文行号。
    #[test]
    fn warns_content_before_group_with_line() {
        // 第 1~2 行为空/说明，第 3 行是缺分组标记的正文。
        let doc = "\n\n没有分组头的正文\n分组: A\n正文A";
        let out = parse(doc.as_bytes());
        assert_eq!(out.warnings.len(), 1);
        assert_eq!(out.warnings[0].line, 3, "告警指向第 3 行");
        assert!(out.warnings[0].message.contains("分组"));
        // 正文仍被容错归入默认组 + A 组。
        assert_eq!(out.total_prompts(), 2);
    }

    // E37：小标题后无正文（文件结尾悬空）→ 告警含小标题行号。
    #[test]
    fn warns_dangling_title_at_eof() {
        let doc = "分组: A\n正文1\n【结尾悬空标题】";
        let out = parse(doc.as_bytes());
        assert_eq!(out.warnings.len(), 1);
        assert_eq!(out.warnings[0].line, 3);
        assert!(out.warnings[0].message.contains("结尾悬空标题"));
    }

    // 正常文档无告警。
    #[test]
    fn clean_document_has_no_warnings() {
        let out = parse("分组: A\n前缀: AA\n【标题】\n正文".as_bytes());
        assert!(out.warnings.is_empty());
    }

    #[test]
    fn arbitrary_bytes_do_not_panic() {
        // 属性式冒烟：随机字节不 panic（round-trip 安全）。
        for seed in 0u32..64 {
            let bytes: Vec<u8> = (0..128)
                .map(|i| ((seed.wrapping_mul(31) + i) % 256) as u8)
                .collect();
            let _ = parse(&bytes);
        }
    }
}
