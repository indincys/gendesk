//! 提示词 txt 导入（执行计划 1.6 / 需求 6.4）。
//!
//! 解析「分组 / 前缀 / 场景 / 标签 / 正文」字段 + UTF-8/GBK 编码探测；
//! 两段式：`parse`（纯函数，不落库）→ 命令层 `commit`（落库 + 号池发放）。
//!
//! 格式约定（宽容解析）：
//! - 头部行 `分组:`/`前缀:`/`场景:`/`标签:`（半/全角冒号均可）设置当前分组元信息；
//! - 其余每条非空行 = 一条提示词（一行一提示词，空行忽略）；
//! - 缺分组 → 归入默认分组「未分组导入」；缺前缀 → 由 commit 阶段自动分配。

/// 单个解析出的分组。
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedGroup {
    pub name: String,
    pub prefix: Option<String>,
    pub scene: String,
    pub tags: Vec<String>,
    pub prompts: Vec<String>,
}

/// 解析结果。
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedImport {
    /// 探测到的编码名（如 "UTF-8" / "GBK"）。
    pub encoding: String,
    pub groups: Vec<ParsedGroup>,
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
    let groups = parse_text(&text);
    ParsedImport { encoding, groups }
}

fn parse_text(text: &str) -> Vec<ParsedGroup> {
    let mut groups: Vec<ParsedGroup> = Vec::new();
    let mut cur: Option<ParsedGroup> = None;

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

    // 解析模型：每条非空、非头部行 = 一条提示词（一行一提示词，无歧义）。
    // 空行仅作视觉分隔，被忽略。
    for raw in text.lines() {
        let trimmed = raw.trim_end_matches('\r').trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Some((key, value)) = parse_header(trimmed) {
            match key {
                Header::Group => {
                    // 新「分组:」头开启新分组：先收束上一个。
                    if let Some(g) = cur.take() {
                        groups.push(g);
                    }
                    cur = Some(ParsedGroup {
                        name: value.to_string(),
                        prefix: None,
                        scene: String::new(),
                        tags: Vec::new(),
                        prompts: Vec::new(),
                    });
                }
                Header::Prefix => ensure(&mut cur).prefix = Some(value.to_uppercase()),
                Header::Scene => ensure(&mut cur).scene = value.to_string(),
                Header::Tags => ensure(&mut cur).tags = split_tags(value),
            }
            continue;
        }

        ensure(&mut cur).prompts.push(trimmed.to_string());
    }

    if let Some(g) = cur.take() {
        groups.push(g);
    }
    // 丢弃完全为空（无提示词）的分组。
    groups.retain(|g| !g.prompts.is_empty());
    groups
}

enum Header {
    Group,
    Prefix,
    Scene,
    Tags,
}

/// 识别头部行 `键: 值`（半/全角冒号）。
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
        "分组" | "组" | "group" | "Group" => Header::Group,
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
        assert_eq!(g.prompts, vec!["第一条提示词正文。", "第二条提示词正文。"]);
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
        assert_eq!(out.groups[0].prompts, vec!["正文1", "正文2"]);
    }

    #[test]
    fn each_nonempty_line_is_one_prompt() {
        let doc = "分组: A\n第一行\n第二行\n第三行\n\n下一条";
        let out = parse(doc.as_bytes());
        assert_eq!(
            out.groups[0].prompts,
            vec!["第一行", "第二行", "第三行", "下一条"]
        );
    }

    #[test]
    fn very_long_prompt_over_500_chars() {
        let long = "光".repeat(600);
        let doc = format!("分组: A\n{long}");
        let out = parse(doc.as_bytes());
        assert_eq!(out.groups[0].prompts[0].chars().count(), 600);
    }

    #[test]
    fn multiple_groups() {
        let doc = "分组: A\n前缀: AA\na1\n\n分组: B\n前缀: BB\nb1\nb2扩展\n\nb3";
        let out = parse(doc.as_bytes());
        assert_eq!(out.groups.len(), 2);
        assert_eq!(out.groups[0].name, "A");
        assert_eq!(out.groups[0].prompts, vec!["a1"]);
        assert_eq!(out.groups[1].name, "B");
        assert_eq!(out.groups[1].prompts, vec!["b1", "b2扩展", "b3"]);
    }

    #[test]
    fn decodes_gbk() {
        // "分组: 商品\n正文一" 的 GBK 字节
        let (bytes, _enc, _had_errors) = encoding_rs::GBK.encode("分组: 商品\n正文一");
        let out = parse(&bytes);
        assert_eq!(out.groups[0].name, "商品");
        assert_eq!(out.groups[0].prompts, vec!["正文一"]);
    }

    #[test]
    fn fullwidth_colon_supported() {
        let out = parse("分组：全角\n正文".as_bytes());
        assert_eq!(out.groups[0].name, "全角");
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
