//! 补料提示词生成（F1）。
//!
//! 缺料/低余量的 SKU → 一段可直接粘贴给 Claude / Codex 的补料 prompt，
//! 内容生产因此闭环：缺什么 → 让 AI 写什么 → 按格式落盘 → 收件箱自动收录。
//!
//! 模板的输出契约必须与 `docs/收件箱收录格式规范.md` 一致——本模块的单测让
//! `inbox::parser` 反过来消化模板里的示例，模板一旦跑偏，测试立刻红。

/// 一个待补料 SKU 的上下文。
#[derive(Debug, Clone)]
pub struct RestockSku {
    pub code: String,
    pub style_name: String,
    pub product_name: String,
    /// 固定话题标签（写进 TXT 的【话题】行）。
    pub topics: Vec<String>,
    pub title_count: i64,
    pub body_count: i64,
}

/// 要补的内容类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestockKind {
    Title,
    Body,
    Both,
}

impl RestockKind {
    pub fn from_str_or_both(s: &str) -> RestockKind {
        match s {
            "title" => RestockKind::Title,
            "body" => RestockKind::Body,
            _ => RestockKind::Both,
        }
    }
}

/// 默认目标条数。
const TITLES_WANTED: i64 = 10;
const BODIES_WANTED: i64 = 5;

/// 生成补料 prompt（纯函数）。多个 SKU 合并成一段，Claude 一次可写多款。
pub fn build_restock_prompt(skus: &[RestockSku], kind: RestockKind) -> String {
    if skus.is_empty() {
        return String::new();
    }
    let want_title = matches!(kind, RestockKind::Title | RestockKind::Both);
    let want_body = matches!(kind, RestockKind::Body | RestockKind::Both);

    let mut s = String::new();
    s.push_str("你是电商内容运营。请为下面每个 SKU 写文案，并**按指定格式落盘为 TXT 文件**。\n\n");

    s.push_str("## 待补款式\n\n");
    for k in skus {
        s.push_str(&format!("### {} · {}\n", k.code, k.style_name));
        if !k.product_name.is_empty() {
            s.push_str(&format!("- 商品名：{}\n", k.product_name));
        }
        if !k.topics.is_empty() {
            s.push_str(&format!("- 固定话题：{}\n", k.topics.join(" ")));
        }
        let mut needs: Vec<String> = Vec::new();
        if want_title {
            needs.push(format!(
                "标题 {} 条（现有 {}）",
                (TITLES_WANTED - k.title_count).max(TITLES_WANTED / 2),
                k.title_count
            ));
        }
        if want_body {
            needs.push(format!(
                "正文 {} 条（现有 {}）",
                (BODIES_WANTED - k.body_count).max(BODIES_WANTED / 2),
                k.body_count
            ));
        }
        s.push_str(&format!("- 需要：{}\n\n", needs.join(" · ")));
    }

    s.push_str("## 落盘格式（严格遵守，否则无法自动收录）\n\n");
    s.push_str("每个 SKU 一个文件夹，文件夹名 = SKU 编码；文件统一 UTF-8。\n");
    s.push_str("文件头用全角括号，头与内容之间空一行。**文件名不带平台名**（全平台共用）。\n\n");

    if want_title {
        s.push_str("**标题**：落盘为 `收件箱/{SKU编码}/标题.txt`，一行一条：\n\n");
        s.push_str("```\n");
        s.push_str(&title_example(&skus[0]));
        s.push_str("```\n\n");
    }
    if want_body {
        s.push_str(
            "**正文**：落盘为 `收件箱/{SKU编码}/正文.txt`，条与条之间用**单独一行** `====` 分隔\
             （正文内部可自由换行）：\n\n",
        );
        s.push_str("```\n");
        s.push_str(&body_example(&skus[0]));
        s.push_str("```\n\n");
    }

    s.push_str("## 要求\n\n");
    s.push_str("- 每条独立可用，不要编号、不要引号包裹。\n");
    s.push_str("- 标题 ≤ 20 字，口语化，避免绝对化用词（最/第一/唯一）。\n");
    s.push_str("- 正文 100–200 字，结尾自然带上固定话题。\n");
    s.push_str("- 不要重复已有文案（同一句话入库时会被自动跳过）。\n");
    s
}

fn title_example(k: &RestockSku) -> String {
    let mut s = format!("【SKU】{}\n【类型】标题\n", k.code);
    if !k.topics.is_empty() {
        s.push_str(&format!(
            "【话题】{}\n",
            k.topics
                .iter()
                .map(|t| format!("#{t}"))
                .collect::<Vec<_>>()
                .join(" ")
        ));
    }
    s.push_str("\n第一条标题写在这里\n第二条标题写在这里\n");
    s
}

fn body_example(k: &RestockSku) -> String {
    let mut s = format!("【SKU】{}\n【类型】正文\n", k.code);
    if !k.topics.is_empty() {
        s.push_str(&format!(
            "【话题】{}\n",
            k.topics
                .iter()
                .map(|t| format!("#{t}"))
                .collect::<Vec<_>>()
                .join(" ")
        ));
    }
    // 分隔符是 `====`（单独一行），与 docs/收件箱收录格式规范.md §2.2 一致。
    s.push_str("\n第一条正文写在这里，100–200 字。\n\n====\n\n第二条正文写在这里。\n");
    s
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即测试失败，是期望行为
mod tests {
    use super::*;
    use crate::publish::inbox::parser;

    fn sku() -> RestockSku {
        RestockSku {
            code: "SF-YD-201".into(),
            style_name: "云朵沙发".into(),
            product_name: "三人位布艺沙发".into(),
            topics: vec!["沙发".into(), "家居".into()],
            title_count: 2,
            body_count: 0,
        }
    }

    /// 反向验证：模板里给 AI 看的示例，必须能被我们自己的 parser 消化。
    /// 模板一旦跑偏（改了头字段、换了分隔符），这条测试立刻红——比人工比对文档可靠。
    #[test]
    fn examples_in_prompt_are_parseable_by_our_own_parser() {
        let k = sku();

        let title_txt = title_example(&k);
        let parsed = parser::parse(&title_txt, None).expect("标题示例应可解析");
        assert_eq!(parsed.sku_code.as_deref(), Some("SF-YD-201"));
        assert_eq!(parsed.titles.len(), 2);
        assert_eq!(parsed.topics, vec!["沙发", "家居"]);

        let body_txt = body_example(&k);
        let parsed = parser::parse(&body_txt, None).expect("正文示例应可解析");
        assert_eq!(parsed.bodies.len(), 2, "--- 分隔的两条正文");
    }

    #[test]
    fn prompt_lists_every_sku_and_respects_kind() {
        let a = sku();
        let b = RestockSku {
            code: "SF-YD-202".into(),
            style_name: "岩板餐桌".into(),
            ..sku()
        };
        let p = build_restock_prompt(&[a, b], RestockKind::Title);
        assert!(
            p.contains("SF-YD-201") && p.contains("SF-YD-202"),
            "多 SKU 合并成一段"
        );
        assert!(p.contains("标题.txt"));
        assert!(!p.contains("正文.txt"), "只补标题时不该提正文");

        let p = build_restock_prompt(&[sku()], RestockKind::Both);
        assert!(p.contains("标题.txt") && p.contains("正文.txt"));
    }

    #[test]
    fn empty_input_yields_empty_prompt() {
        assert!(build_restock_prompt(&[], RestockKind::Both).is_empty());
    }
}
