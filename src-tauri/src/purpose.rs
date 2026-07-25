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

/// 「图生视频」的组名/场景/标签关键词。命中即**预猜**，不是判定。
///
/// 由来：用户的 txt 从不写 `标签: 图生视频`，但组名几乎总带自我说明
/// （`鹿晗-B-Roll素材分镜图`、`GD4 首帧`）。让人每次进提示词库补标是白干的活。
///
/// 全部小写、已去分隔符后比对，故 `B-Roll`/`B Roll`/`b_roll`/`BROLL` 一网打尽。
const I2V_KEYWORDS: &[&str] = &[
    "broll",
    "图生视频",
    "图转视频",
    "分镜",
    "首帧",
    "视频素材",
    "运镜",
    "i2v",
    "v2v",
];

/// 归一化：转小写 + 去掉分隔符（连字符/下划线/空白/各类中西文标点）。
///
/// 只去分隔符不去别的：`B-Roll` 与 `B Roll` 必须等价，但不能把 `分镜` 里的字也吃掉。
fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_whitespace() && !matches!(c, '-' | '_' | '—' | '－' | '·' | '.' | '/'))
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// 从分组名/场景/自由标签推断用途（**默认预勾选值，不是门禁**）。
///
/// 只看这三处元信息，**故意不扫正文**：图生视频的首帧图提示词里出现「首帧」纯属偶然，
/// 而一条正文误命中就会把整组默认标成视频用途，人还得回去取消——预猜错的代价必须
/// 低于不猜。组名/场景/标签是人为该组起的名字，是唯一「作者已经表过态」的地方。
pub fn infer_purposes(group_name: &str, scene: &str, tags: &[String]) -> Vec<String> {
    let mut haystack = normalize(group_name);
    haystack.push_str(&normalize(scene));
    for t in tags {
        haystack.push_str(&normalize(t));
    }
    if I2V_KEYWORDS.iter().any(|k| haystack.contains(&normalize(k))) {
        vec![PURPOSE_I2V.to_string()]
    } else {
        Vec::new()
    }
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

    // 关键词预猜的核心用例：用户真实组名带 B-Roll / 分镜 / 首帧，各种分隔符写法都要命中。
    #[test]
    fn infers_i2v_from_real_group_names() {
        for name in [
            "鹿晗-B-Roll素材分镜图",
            "G-Dragon B Roll",
            "gd4_broll",
            "BROLL",
            "侯明昊首帧",
            "第三批分镜",
            "图生视频-通用",
        ] {
            assert_eq!(
                infer_purposes(name, "", &[]),
                vec![PURPOSE_I2V.to_string()],
                "组名「{name}」应预猜为图生视频"
            );
        }
    }

    // 预猜错的代价必须低于不猜：普通图片组不得被默认标成视频用途。
    #[test]
    fn does_not_infer_for_ordinary_groups() {
        for name in ["电商主图", "白底商品", "人物场景", "详情页长图", "DZ 主图"] {
            assert!(
                infer_purposes(name, "", &[]).is_empty(),
                "普通组名「{name}」不应被预猜为视频用途"
            );
        }
    }

    // 场景与自由标签也参与预猜（有人把说明写在「场景:」而不是组名里）。
    #[test]
    fn infers_from_scene_and_tags() {
        assert_eq!(
            infer_purposes("第三批", "B-Roll 素材", &[]),
            vec![PURPOSE_I2V.to_string()]
        );
        assert_eq!(
            infer_purposes("第三批", "", &["分镜".to_string()]),
            vec![PURPOSE_I2V.to_string()]
        );
    }

    // **故意不扫正文**：正文里偶然出现「首帧」不该把整组标成视频用途。
    // 这条测试守住「只看三处元信息」这个决定——把正文加进 haystack 会让它失败。
    #[test]
    fn ignores_body_text_by_construction() {
        // infer_purposes 的签名里根本没有正文参数；组名干净时必须为空，
        // 即便调用方手里握着一堆含「首帧」的正文。
        assert!(infer_purposes("电商主图", "商品", &[]).is_empty());
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
