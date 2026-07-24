//! 用途（管线）单点定义。
//!
//! 「用途」标在**提示词组**上，不在图上、也不在批次上：一张图的用途由它的提示词决定，
//! 提示词的用途由那份 txt 决定，而一份 txt = 一个组。批次会混组（batch 7 混了几十个组），
//! 所以批次不是用途单元。
//!
//! 存储复用既有的 `tags`/`tag_bindings`（entity_type='prompt_group'），零 schema 改动；
//! 但**取值受控**——UI 只给选择器不给输入框。理由与 `publish::platform` 的五平台单点相同：
//! 一旦允许手打，「图生视频 / 图转视频 / v2v」三种拼法必然同时存在，下游按字符串筛选就漏。
//!
//! 用途是**筛选默认值，不是门禁**：作品库照旧允许手选任意作品导出。堵死了就得改代码。

use serde::Serialize;
use specta::Type;

/// 图生视频：该组的图会送去即梦（Dreamina）做图生视频素材。
pub const PURPOSE_I2V: &str = "图生视频";

/// 一个用途选项（前端选择器渲染源）。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PurposeView {
    /// 落库的标签名（tags.name）。
    pub tag: String,
    /// 一句话说明，选择器里做副标题。
    pub hint: String,
}

/// 全部受控用途。新增用途 = 在此追加一行，前端不需要改。
pub fn all() -> Vec<PurposeView> {
    vec![PurposeView {
        tag: PURPOSE_I2V.to_string(),
        hint: "该组的验收图会导出为图生视频包，送即梦生成视频".to_string(),
    }]
}

/// 是否为受控用途标签（区别于导入 txt 里自由写的普通标签）。
pub fn is_purpose(tag: &str) -> bool {
    all().iter().any(|p| p.tag == tag)
}

/// 合并：保留既有的自由标签，只替换用途标签。
///
/// 用途选择器不该顺手抹掉用户在 txt 里写的 `标签: 白底,3C`——那是两套互不相干的东西
/// 恰好共用一张表。
pub fn merge_purposes(existing: &[String], purposes: &[String]) -> Vec<String> {
    existing
        .iter()
        .filter(|t| !is_purpose(t))
        .cloned()
        .chain(purposes.iter().cloned())
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // 测试断言失败即失败
mod tests {
    use super::*;

    // 用途标签是前端选择器与后端筛选的共同契约：两侧都从 all() 取，
    // 改名只需改这里一处。这条测试守住「常量与列表不脱节」。
    #[test]
    fn purpose_constant_is_listed() {
        assert!(is_purpose(PURPOSE_I2V), "常量用途必须出现在 all() 中");
        assert!(!is_purpose("随手写的标签"), "自由标签不应被认作受控用途");
    }

    // 打用途不得抹掉 txt 导入进来的自由标签：两套东西恰好共用一张 tags 表。
    #[test]
    fn merge_keeps_free_tags_and_replaces_purposes() {
        let existing = vec![
            "白底".to_string(),
            "3C".to_string(),
            PURPOSE_I2V.to_string(),
        ];
        // 取消用途：自由标签必须原样留下。
        let merged = merge_purposes(&existing, &[]);
        assert_eq!(merged, vec!["白底".to_string(), "3C".to_string()]);
        // 重新打上：不产生重复。
        let merged = merge_purposes(&existing, &[PURPOSE_I2V.to_string()]);
        assert_eq!(
            merged,
            vec![
                "白底".to_string(),
                "3C".to_string(),
                PURPOSE_I2V.to_string()
            ],
            "用途只应出现一次，自由标签顺序不变"
        );
    }

    // 组上一个标签都没有时也要能打（全库 tags 表长期为空，这是最常见的起点）。
    #[test]
    fn merge_from_empty_existing() {
        assert_eq!(
            merge_purposes(&[], &[PURPOSE_I2V.to_string()]),
            vec![PURPOSE_I2V.to_string()]
        );
    }

    // 用途标签会成为目录名的一部分（v2v 包名），且要参与 SQL 精确匹配，
    // 不允许前后空白——否则「图生视频 」与「图生视频」在库里是两个标签。
    #[test]
    fn purpose_tags_are_trimmed_and_nonempty() {
        for p in all() {
            assert_eq!(p.tag.trim(), p.tag, "用途标签不得带前后空白");
            assert!(!p.tag.is_empty(), "用途标签不得为空");
            assert!(
                !p.hint.is_empty(),
                "用途需要一句说明，否则选择器只有光秃秃的名字"
            );
        }
    }
}
