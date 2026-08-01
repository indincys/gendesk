//! 商品级文案 txt 契约解析，命令导入与 watcher 共用。

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedCopy {
    pub product_code: Option<String>,
    pub kind: String,
    pub titles: Vec<String>,
    pub bodies: Vec<String>,
    pub topics: Vec<String>,
}

fn header_value<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.trim().strip_prefix(key).map(str::trim)
}

fn split_blocks(lines: &[&str]) -> Vec<String> {
    lines
        .split(|line| line.trim() == "====")
        .map(|block| block.join("\n").trim().to_string())
        .filter(|block| !block.is_empty())
        .collect()
}

fn parse_combo(lines: &[&str]) -> (Vec<String>, Vec<String>) {
    let mut titles = Vec::new();
    let mut bodies = Vec::new();
    for block in lines.split(|line| line.trim() == "====") {
        let mut title: Option<String> = None;
        let mut body = Vec::new();
        let mut in_body = false;
        for line in block {
            if let Some(value) = header_value(line, "【标题】") {
                if !value.is_empty() {
                    title = Some(value.to_string());
                }
                in_body = false;
            } else if let Some(value) = header_value(line, "【正文】") {
                in_body = true;
                if !value.is_empty() {
                    body.push(value);
                }
            } else if in_body {
                body.push(line);
            }
        }
        if let Some(value) = title {
            titles.push(value);
        }
        let value = body.join("\n").trim().to_string();
        if !value.is_empty() {
            bodies.push(value);
        }
    }
    (titles, bodies)
}

pub fn parse(source: &str, filename: &str) -> Result<ParsedCopy, String> {
    let normalized = source.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let lines: Vec<&str> = normalized.lines().collect();
    let mut product_code = None;
    let mut kind = None;
    let mut topics = Vec::new();
    let mut body_start = 0;
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if let Some(value) = header_value(trimmed, "【商品】") {
            if !value.is_empty() {
                product_code = Some(value.to_ascii_uppercase());
            }
        } else if let Some(value) = header_value(trimmed, "【类型】") {
            kind = match value {
                "标题" => Some("title"),
                "正文" => Some("body"),
                "图文" => Some("combo"),
                _ => None,
            };
        } else if let Some(value) = header_value(trimmed, "【话题】") {
            topics = value
                .split_whitespace()
                .map(|tag| format!("#{}", tag.trim_start_matches('#')))
                .filter(|tag| tag.len() > 1)
                .collect();
        } else if trimmed.is_empty() {
            body_start = index + 1;
            break;
        } else if !trimmed.starts_with('【') {
            body_start = index;
            break;
        }
        body_start = index + 1;
    }
    let filename_kind = if filename.contains("标题") {
        Some("title")
    } else if filename.contains("正文") {
        Some("body")
    } else if filename.contains("图文") {
        Some("combo")
    } else {
        None
    };
    let kind = kind
        .or(filename_kind)
        .ok_or_else(|| "缺少或无法识别【类型】".to_string())?;
    let content = &lines[body_start.min(lines.len())..];
    let (titles, bodies) = match kind {
        "title" => (
            content
                .iter()
                .map(|line| line.trim())
                .filter(|line| !line.is_empty())
                .map(str::to_string)
                .collect(),
            Vec::new(),
        ),
        "body" => (Vec::new(), split_blocks(content)),
        "combo" => parse_combo(content),
        _ => (Vec::new(), Vec::new()),
    };
    if titles.is_empty() && bodies.is_empty() {
        return Err("文件里没有可收录的标题或正文".into());
    }
    Ok(ParsedCopy {
        product_code,
        kind: kind.to_string(),
        titles,
        bodies,
        topics,
    })
}

/// FNV-1a 64 位，稳定、无随机种子，足够用于本地内容去重键。
pub fn content_hash(bytes: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // 测试断言失败即测试失败
mod tests {
    use super::*;

    #[test]
    fn body_separator_and_product_header_are_parsed() {
        let parsed = parse(
            "【商品】A\n【类型】正文\n\n第一段\n第二行\n====\n第二段",
            "正文.txt",
        )
        .unwrap();
        assert_eq!(parsed.product_code.as_deref(), Some("A"));
        assert_eq!(parsed.bodies, vec!["第一段\n第二行", "第二段"]);
    }

    #[test]
    fn combo_is_split_into_two_pools() {
        let parsed = parse(
            "【商品】A\n【类型】图文\n\n【标题】标题一\n【正文】\n正文一\n====\n【标题】标题二\n【正文】正文二",
            "图文.txt",
        )
        .unwrap();
        assert_eq!(parsed.titles, vec!["标题一", "标题二"]);
        assert_eq!(parsed.bodies, vec!["正文一", "正文二"]);
    }

    #[test]
    fn hash_is_content_based_and_stable() {
        assert_eq!(content_hash(b"same"), content_hash(b"same"));
        assert_ne!(content_hash(b"same"), content_hash(b"other"));
    }
}
