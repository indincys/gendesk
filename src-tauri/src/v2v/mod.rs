//! 图生视频（image-to-video）流水线。
//!
//! 「验收通过的图 → 改写成即梦提示词 → 提交 → 轮询 → 出片 → 人工验收 → 交付」，
//! 全程状态在库内（`v2v_clips`，v0.15.0）。Claude Code / Codex 侧的 skill 只做一件事：
//! 读交接工单、把生图提示词改写成图生视频提示词、写回去。
//!
//! 子模块各管一段：`handoff`（交接目录工单往返）· `runner`（提交/轮询/落盘）·
//! `dreamina`（CLI 封装）· `autofill`（常驻非 VIP 队列）· `watcher`（监听改写结果）·
//! `activity`（执行日志）· `queue_trend`（排队位次 → 排队速度）· `events`。
//!
//! ## 这里只剩两个纯函数，它们是「改写提示怎么写」的全部
//!
//! v0.13.0 那套一次性导出包（manifest/ledger/PackItem/write_pack）已随 v0.22.0 删除：
//! 真相在库里，包被移走就失忆的那份台账早在 v0.15.0 就取消了，而写包那条路径此后
//! 一直没有任何调用方。剩下的 [`common_affixes`] / [`variable_part`] 仍在用 ——
//! `handoff::materialize` 靠它们把组内逐字相同的产品保真模板从改写提示里剥掉。

pub mod activity;
pub mod autofill;
pub mod dreamina;
pub mod events;
pub mod handoff;
pub mod queue_trend;
pub mod runner;
pub mod watcher;

/// 组内公共前后缀长度（按 **char** 计，不是字节——按字节切会把中文切碎）。
///
/// 同一组的提示词共享该产品的「保真模板」：实测 batch 15 四个组各有 147/165/384/305 字
/// 逐字相同的产品保真尾巴（「哪个环穿哪个孔、谁挂在谁之上一律不得移动」），对图生视频
/// 是纯噪音甚至有害——图已经是首帧，产品已经画对了，再喂 300 字配件穿接关系只会把改写带偏。
///
/// 少于 2 条时返回 `(0, 0)`：单条的「公共前后缀」就是它自己，剥完什么都不剩。
pub fn common_affixes(texts: &[String]) -> (usize, usize) {
    if texts.len() < 2 {
        return (0, 0);
    }
    let chars: Vec<Vec<char>> = texts.iter().map(|t| t.chars().collect()).collect();
    let Some(min_len) = chars.iter().map(|c| c.len()).min() else {
        return (0, 0);
    };
    if min_len == 0 {
        return (0, 0);
    }

    let mut prefix = 0usize;
    while prefix < min_len {
        let c = chars[0][prefix];
        if chars.iter().any(|t| t[prefix] != c) {
            break;
        }
        prefix += 1;
    }

    let mut suffix = 0usize;
    while suffix < min_len {
        let c = chars[0][chars[0].len() - 1 - suffix];
        if chars.iter().any(|t| t[t.len() - 1 - suffix] != c) {
            break;
        }
        suffix += 1;
    }

    // 前后缀可能重叠（组内两条几乎相同时，两轮都会一路扫到 min_len）。
    // 保前缀、压后缀，保证 prefix + suffix <= min_len，切片区间不会反转。
    if prefix + suffix > min_len {
        suffix = min_len - prefix;
    }
    (prefix, suffix)
}

/// 按给定前后缀长度取可变部分。剥完为空则回退全文——宁可带噪音，不可无内容。
pub fn variable_part(text: &str, prefix: usize, suffix: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if prefix + suffix >= chars.len() {
        return text.trim().to_string();
    }
    let out: String = chars[prefix..chars.len() - suffix].iter().collect();
    let trimmed = out.trim();
    if trimmed.is_empty() {
        text.trim().to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即失败
mod tests {
    use super::*;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    // 核心用例：组内共享的产品保真模板应被整段剥掉，只留下场景/动势。
    #[test]
    fn strips_shared_template_head_and_tail() {
        let texts = v(&[
            "把参考图中的产品完整原样保留，将背景替换为屋顶花园的木地台。产品必须像素级完整保留：绿色外框、白色内衬，配件一个不多一个不少。",
            "把参考图中的产品完整原样保留，将背景替换为宠物餐吧的原木餐桌。产品必须像素级完整保留：绿色外框、白色内衬，配件一个不多一个不少。",
        ]);
        let (p, s) = common_affixes(&texts);
        assert!(p > 0 && s > 0, "前后缀都应识别到");
        let a = variable_part(&texts[0], p, s);
        let b = variable_part(&texts[1], p, s);
        assert!(a.contains("屋顶花园"), "场景差异须保留: {a}");
        assert!(b.contains("宠物餐吧"), "场景差异须保留: {b}");
        assert!(
            !a.contains("像素级完整保留"),
            "共享的产品保真尾巴须被剥掉: {a}"
        );
        assert!(!a.contains("把参考图中的产品"), "共享的开头须被剥掉: {a}");
    }

    // 单条时不能剥离：它跟自己的「公共前后缀」就是全文，剥完一个字不剩。
    // 组内只通过 1 张的情况在真实数据里就有（batch 15 有组只通过 3 张，将来会有 1 张的）。
    #[test]
    fn single_item_is_never_stripped() {
        let texts = v(&["只有这一条提示词。"]);
        assert_eq!(common_affixes(&texts), (0, 0));
        assert_eq!(variable_part(&texts[0], 0, 0), "只有这一条提示词。");
    }

    // 完全相同的两条：前后缀各自都会扫满全长，必须夹紧避免区间反转（否则 panic）。
    #[test]
    fn identical_texts_do_not_panic_and_fall_back_to_full_text() {
        let texts = v(&["一模一样的文本", "一模一样的文本"]);
        let (p, s) = common_affixes(&texts);
        assert!(p + s <= texts[0].chars().count(), "前后缀之和不得超过全长");
        // 剥完为空 → 回退全文，绝不给下游一个空提示词。
        assert_eq!(variable_part(&texts[0], p, s), "一模一样的文本");
    }

    // 按 char 而非 byte 切：中文一字三字节，按字节切会切出半个字（非法 UTF-8 直接 panic）。
    #[test]
    fn slices_by_char_not_byte() {
        let texts = v(&["前缀甲乙丙后缀", "前缀丁戊己后缀"]);
        let (p, s) = common_affixes(&texts);
        assert_eq!((p, s), (2, 2), "应按字符数计前后缀");
        assert_eq!(variable_part(&texts[0], p, s), "甲乙丙");
        assert_eq!(variable_part(&texts[1], p, s), "丁戊己");
    }

    // 毫无共同点的两条：不剥离，各自全文通过。
    #[test]
    fn no_common_affix_keeps_full_text() {
        let texts = v(&["完全不同的内容甲", "另起炉灶的内容乙"]);
        assert_eq!(common_affixes(&texts), (0, 0));
    }

    // 长短悬殊：短的那条是长的那条的真前缀时，min_len 夹紧后区间仍合法。
    #[test]
    fn length_mismatch_stays_within_bounds() {
        let texts = v(&["共同开头", "共同开头再加一大段别的内容"]);
        let (p, s) = common_affixes(&texts);
        assert!(p + s <= 4, "不得超过较短那条的长度");
        for t in &texts {
            let _ = variable_part(t, p, s); // 不 panic 即可
        }
    }

    // 空文本混入不应导致越界。
    #[test]
    fn empty_text_is_tolerated() {
        let texts = v(&["", "有内容"]);
        assert_eq!(common_affixes(&texts), (0, 0));
        assert_eq!(variable_part("", 0, 0), "");
    }
}
