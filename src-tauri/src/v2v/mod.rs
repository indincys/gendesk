//! 图生视频（image-to-video）导出包。
//!
//! 把「验收通过的图 + 它的生图提示词」成对导出为一个自包含目录，供 Claude Code 侧的
//! skill 读取、改写成即梦（Dreamina）提示词、再逐条提交 `dreamina image2video`。
//!
//! ## 为什么是「包」而不是「一堆文件」
//!
//! 图多了以后真正的痛点不是导出，是「哪些已经做过、哪些改写完了、哪些提交了还没取回」。
//! 所以包里自带 `manifest.jsonl`（只读契约，本模块写一次）与留给 skill 追加的
//! `ledger.jsonl`（append-only，同 id 最后一条即当前态，断点续跑 = 折叠 ledger）。
//! JSONL 而非 JSON 数组：可 grep、可 `head -n` 分片，skill 不必把整包读进上下文。
//!
//! ## 一一对应的锚点是 work id，不是文件名
//!
//! 输出文件名形如 `参考图名_260724_BR140010_1.JPG`，其中编号已去连字符——`BR140010`
//! 无法反推是 `BR14-0010` 还是 `BR1-40010`，**文件名本来就是不可逆的**。历史批次更早于
//! 抽卡序号（E17 D2）落地，连结构都不一致。故包内主键取 `accepted_works.id`（`W{id}`），
//! 中文原名只作为 `displayName` 留在 manifest 里给人看。
//!
//! ## 一包一组
//!
//! 组是用途的天然单元（一份 txt = 一个组），也是成片的单元：同组的分镜图最后要剪在
//! 一起，运镜语言与时长必须统一，跨组混一个包改写风格会飘。

pub mod dreamina;
pub mod events;
pub mod handoff;
pub mod runner;

use serde::Serialize;

use crate::error::{AppError, AppResult};
use crate::publish::paths::ascii_slug;

/// 导出渠道标识（`work_exports.channel`）。单点定义：写台账与「隐藏已导出」筛选共用，
/// 否则一侧写 `i2v`、另一侧查 `v2v`，筛选永远筛不掉任何东西且毫无报错。
pub const CHANNEL_I2V: &str = "i2v";

/// 包内条目（manifest.jsonl 一行一条）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackItem {
    /// 包内主键，`W{work_id}`。ASCII、稳定、可作文件名。
    pub id: String,
    pub work_id: i64,
    pub prompt_code: String,
    pub group_name: String,
    pub ref_name: String,
    pub batch_id: Option<i64>,
    pub accepted_at: i64,
    /// 包内相对路径：原图，喂即梦。
    pub image: String,
    /// 包内相对路径：缩略图，喂 LLM 看。384×512 约 260 token，比原图省一个量级。
    pub thumb: String,
    /// 导出前的中文原文件名，仅供人对照。
    pub display_name: String,
    /// 生图提示词全文（快照，来自 accepted_works.prompt_text）。
    pub source_prompt: String,
    /// 剥掉组内公共前后缀后的可变部分——场景/构图/动势，改写视频提示词的真正素材。
    pub variable_part: String,
    /// 剥掉的公共前缀字数（char）。为 0 表示未剥离。
    pub stripped_prefix_chars: usize,
    /// 剥掉的公共后缀字数（char）。
    pub stripped_suffix_chars: usize,
}

/// 一次导出的结果摘要（回前端 toast）。
#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct PackSummary {
    /// 包目录绝对路径。
    pub pack_dir: String,
    /// 包目录名（= work_exports.pack_id）。
    pub pack_id: String,
    pub exported: i64,
    /// 源文件缺失而跳过的条目数。
    pub skipped: i64,
    pub stripped_prefix_chars: usize,
    pub stripped_suffix_chars: usize,
}

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

/// 包目录名：`{YYMMDD}-{组前缀}[-{组名 ASCII 化}]`，例 `260724-gd4-g-dragon-b-roll`。
///
/// 组前缀（BR14/GD4）在库里唯一，单独就足以定位组；ASCII 化的组名只在还剩得下
/// 可读字符时追加（纯中文组名会被折叠成占位符 `x`，那就不追加，免得满目录 `-x`）。
pub fn pack_dir_name(date_yymmdd: &str, group_prefix: &str, group_name: &str) -> String {
    let base = format!("{}-{}", date_yymmdd, ascii_slug(group_prefix));
    let slug = ascii_slug(group_name);
    let slug = slug.trim_matches(|c| c == '-' || c == '_');
    // ascii_slug 对纯中文名回退为 "x"；那不是信息，别往目录名上挂。
    let informative =
        slug.len() >= 2 && slug != "x" && slug.chars().any(|c| c.is_ascii_alphanumeric());
    if informative {
        format!("{base}-{slug}")
    } else {
        base
    }
}

/// 目标目录已存在时追加序号，避免覆盖上一次导出的同组包（同一天导出两次）。
pub fn dedupe_dir(parent: &std::path::Path, base: &str) -> String {
    if !parent.join(base).exists() {
        return base.to_string();
    }
    let mut n = 2;
    loop {
        let candidate = format!("{base}-{n}");
        if !parent.join(&candidate).exists() {
            return candidate;
        }
        n += 1;
    }
}

/// 写包体。**READY.txt 最后写**——skill 只认带 READY.txt 的包，
/// 半成品包（拷到一半、磁盘满）不会被当成可执行的输入。
pub fn write_pack(
    pack_dir: &std::path::Path,
    items: &[PackItem],
    group_name: &str,
) -> AppResult<()> {
    std::fs::create_dir_all(pack_dir.join("images")).map_err(|e| AppError::Io(e.to_string()))?;
    std::fs::create_dir_all(pack_dir.join("thumbs")).map_err(|e| AppError::Io(e.to_string()))?;
    // 空目录先建好，skill 直接往里写，也让人一眼看懂包的结构。
    std::fs::create_dir_all(pack_dir.join("prompts")).map_err(|e| AppError::Io(e.to_string()))?;
    std::fs::create_dir_all(pack_dir.join("videos")).map_err(|e| AppError::Io(e.to_string()))?;

    let mut manifest = String::new();
    for item in items {
        let line = serde_json::to_string(item)
            .map_err(|e| AppError::Internal(format!("manifest 序列化失败: {e}")))?;
        manifest.push_str(&line);
        manifest.push('\n');
    }
    std::fs::write(pack_dir.join("manifest.jsonl"), manifest)
        .map_err(|e| AppError::Io(e.to_string()))?;

    // ledger 由 skill 追加；先建空文件，省得 skill 判断存在性。
    std::fs::write(pack_dir.join("ledger.jsonl"), "").map_err(|e| AppError::Io(e.to_string()))?;

    std::fs::write(pack_dir.join("执行说明.md"), readme(items, group_name))
        .map_err(|e| AppError::Io(e.to_string()))?;

    std::fs::write(
        pack_dir.join("READY.txt"),
        format!("{} 条，导出完成\n", items.len()),
    )
    .map_err(|e| AppError::Io(e.to_string()))?;
    Ok(())
}

fn readme(items: &[PackItem], group_name: &str) -> String {
    let (p, s) = items
        .first()
        .map(|i| (i.stripped_prefix_chars, i.stripped_suffix_chars))
        .unwrap_or((0, 0));
    format!(
        "# 图生视频包 · {group_name}\n\n\
         共 {n} 条。一条 = 一张验收通过的图 + 它的生图提示词。\n\n\
         ## 目录\n\n\
         - `manifest.jsonl` 只读契约，一行一条\n\
         - `ledger.jsonl` 追加式状态，同 id 最后一条即当前态\n\
         - `images/` 原图，喂即梦 `--image`\n\
         - `thumbs/` 缩略图，给模型看图用（比原图省一个量级的 token）\n\
         - `prompts/` 改写后的即梦提示词，一条一个 `.txt`（待生成）\n\
         - `videos/` 取回的成片（待生成）\n\n\
         ## manifest 字段\n\n\
         - `sourcePrompt` 生图提示词全文\n\
         - `variablePart` 剥掉组内公共前后缀后的可变部分（本包剥前 {p} 字、后 {s} 字），\
         即场景/构图/动势——改写视频提示词的真正素材\n\
         - 剥离只是提示，不是契约；拿不准就读 `sourcePrompt` 全文\n\n\
         ## 下一步\n\n\
         1. 逐条读 `thumbs/` + `variablePart`，写 `prompts/{{id}}.txt`\n\
         2. `dreamina image2video --image=<绝对路径> --prompt=\"$(cat prompts/{{id}}.txt)\"`\n\
         3. `dreamina query_result --submit_id=<id> --download_dir=videos/`\n\n\
         提示词一律从文件读入，不要内联拼进 shell——改写结果里必然有引号和换行。\n",
        group_name = group_name,
        n = items.len(),
        p = p,
        s = s,
    )
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

    // 包目录名：ASCII、含日期与组前缀；纯中文组名不追加无信息的占位符。
    #[test]
    fn pack_dir_name_is_ascii_and_identifiable() {
        assert_eq!(
            pack_dir_name("260724", "GD4", "G-Dragon-B-Roll素材分镜图"),
            "260724-gd4-g-dragon-b-roll"
        );
        // 纯中文组名 → ascii_slug 回退 "x"，不追加。
        assert_eq!(pack_dir_name("260724", "BR14", "侯明昊"), "260724-br14");
        let name = pack_dir_name("260724", "BR13", "鹿晗-B-Roll素材分镜图");
        assert!(name.starts_with("260724-br13"), "须以日期+前缀开头: {name}");
        assert!(
            name.is_ascii(),
            "包目录名必须全 ASCII（要进 shell 参数）: {name}"
        );
    }

    // 同一天同一组导出两次不得互相覆盖。
    #[test]
    fn dedupe_dir_avoids_overwriting_previous_export() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(dedupe_dir(tmp.path(), "260724-gd4"), "260724-gd4");
        std::fs::create_dir_all(tmp.path().join("260724-gd4")).unwrap();
        assert_eq!(dedupe_dir(tmp.path(), "260724-gd4"), "260724-gd4-2");
        std::fs::create_dir_all(tmp.path().join("260724-gd4-2")).unwrap();
        assert_eq!(dedupe_dir(tmp.path(), "260724-gd4"), "260724-gd4-3");
    }

    // READY.txt 必须最后写：skill 只认带 READY.txt 的包，
    // 若它先于 manifest 落盘，半成品包会被当成可执行输入。
    #[test]
    fn write_pack_emits_manifest_and_ready() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("pack");
        let items = vec![PackItem {
            id: "W1".into(),
            work_id: 1,
            prompt_code: "GD4-0001".into(),
            group_name: "测试组".into(),
            ref_name: "参考图".into(),
            batch_id: Some(15),
            accepted_at: 0,
            image: "images/W1.JPG".into(),
            thumb: "thumbs/W1.jpg".into(),
            display_name: "参考图_260724_GD40001_1.JPG".into(),
            source_prompt: "全文".into(),
            variable_part: "可变".into(),
            stripped_prefix_chars: 3,
            stripped_suffix_chars: 4,
        }];
        write_pack(&dir, &items, "测试组").unwrap();

        let manifest = std::fs::read_to_string(dir.join("manifest.jsonl")).unwrap();
        assert_eq!(manifest.lines().count(), 1, "一行一条");
        let parsed: serde_json::Value =
            serde_json::from_str(manifest.lines().next().unwrap()).unwrap();
        assert_eq!(parsed["id"], "W1");
        assert_eq!(parsed["workId"], 1, "载荷须 camelCase");
        assert_eq!(parsed["sourcePrompt"], "全文");

        assert!(dir.join("READY.txt").is_file(), "READY.txt 须存在");
        assert!(dir.join("ledger.jsonl").is_file(), "ledger 须预建");
        assert!(dir.join("images").is_dir() && dir.join("thumbs").is_dir());
        assert!(dir.join("prompts").is_dir() && dir.join("videos").is_dir());

        // READY 的 mtime 不早于 manifest：保证写序（同秒也可接受，故用 >=）。
        let ready = std::fs::metadata(dir.join("READY.txt"))
            .unwrap()
            .modified()
            .unwrap();
        let man = std::fs::metadata(dir.join("manifest.jsonl"))
            .unwrap()
            .modified()
            .unwrap();
        assert!(ready >= man, "READY.txt 必须最后写");
    }
}
