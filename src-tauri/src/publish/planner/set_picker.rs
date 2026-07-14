//! 套装选取（发布模块执行计划 §2.2）。纯函数：查重过滤 → 最少使用 → 同分随机。
//!
//! 内容类型由素材包类型决定：视频包 → 只需标题；图集包 → 标题 + 正文。
//! 标题/正文优先匹配目标平台标签，回退「通用」。固定 seed 可复现。

// 部分字段/方法先于 generate_sheet 消费者落地。
#![allow(dead_code)]

use crate::publish::planner::Rng;

/// 素材包候选。`last_pub`：该包各平台最近发布时间（platform code → Unix 秒）。
#[derive(Debug, Clone)]
pub struct PackCand {
    pub id: i64,
    /// "video" | "gallery"
    pub kind: String,
    /// 存储生命周期：new|active|retired。
    pub lifecycle: String,
    pub last_pub: Vec<(String, i64)>,
}

/// 文本候选（标题或正文）。
#[derive(Debug, Clone)]
pub struct TextCand {
    pub id: i64,
    /// 平台标签 code 或 "general"。
    pub platform: String,
    pub use_count: i64,
}

/// 选取输入。
#[derive(Debug, Clone)]
pub struct PickInput {
    pub packs: Vec<PackCand>,
    pub titles: Vec<TextCand>,
    pub bodies: Vec<TextCand>,
    /// 目标平台 code 集（该 SKU 当日生效平台）。
    pub target_platforms: Vec<String>,
    pub dedup_days: i64,
    pub now: i64,
    pub seed: u64,
}

/// 选取结果：一套内容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetPick {
    pub pack_id: i64,
    pub title_id: i64,
    /// 图集包才有正文。
    pub body_id: Option<i64>,
    pub content_kind: String,
}

/// 选取失败原因（进缺料清单）。变体同前缀 `No` 是语义所需，豁免命名 lint。
#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickError {
    NoPack,
    NoTitle,
    NoBody,
}

impl PickError {
    pub fn label(&self) -> &'static str {
        match self {
            PickError::NoPack => "无可用素材包",
            PickError::NoTitle => "无可用标题",
            PickError::NoBody => "无可用正文（图集需正文）",
        }
    }

    /// 机器可读原因码（存 shortage_json；中文由前端 `shortageLabel()` 单点映射）。
    pub fn code(&self) -> &'static str {
        match self {
            PickError::NoPack => "no_pack",
            PickError::NoTitle => "no_title",
            PickError::NoBody => "no_body",
        }
    }
}

/// 素材包是否在查重窗口内「已用尽」：全部目标平台都有近发布。
fn pack_exhausted(p: &PackCand, targets: &[String], dedup_days: i64, now: i64) -> bool {
    if targets.is_empty() {
        return false;
    }
    let window = dedup_days.max(0) * 86_400;
    targets.iter().all(|plat| {
        p.last_pub
            .iter()
            .find(|(pl, _)| pl == plat)
            .is_some_and(|(_, t)| t + window > now)
    })
}

/// 素材包「最近使用时间」：所有平台里的最大发布时间（无记录 = 0，最久未用）。
fn pack_last_used(p: &PackCand) -> i64 {
    p.last_pub.iter().map(|(_, t)| *t).max().unwrap_or(0)
}

/// 从候选里按「最少使用 → 同分随机」选一条文本；优先平台匹配，回退通用。
fn pick_text(cands: &[TextCand], targets: &[String], rng: &mut Rng) -> Option<i64> {
    if cands.is_empty() {
        return None;
    }
    // 优先：平台标签命中任一目标平台的条目；否则回退全部（含 general）。
    let matched: Vec<&TextCand> = cands
        .iter()
        .filter(|c| targets.iter().any(|t| t == &c.platform))
        .collect();
    let pool: Vec<&TextCand> = if matched.is_empty() {
        cands.iter().collect()
    } else {
        matched
    };
    // 最少使用优先。
    let min_use = pool.iter().map(|c| c.use_count).min()?;
    let mut tied: Vec<&TextCand> = pool
        .into_iter()
        .filter(|c| c.use_count == min_use)
        .collect();
    tied.sort_by_key(|c| c.id); // 稳定基准
    let idx = rng.below(tied.len());
    Some(tied[idx].id)
}

/// 选取一套内容。确定性（同 seed 同输入同输出）。
pub fn pick(input: &PickInput) -> Result<SetPick, PickError> {
    let mut rng = Rng::new(input.seed);

    // 1) 可用素材包：非退役、非新入库（未完善不排期）、未用尽。
    let mut usable: Vec<&PackCand> = input
        .packs
        .iter()
        .filter(|p| p.lifecycle == "active")
        .filter(|p| !pack_exhausted(p, &input.target_platforms, input.dedup_days, input.now))
        .collect();
    if usable.is_empty() {
        return Err(PickError::NoPack);
    }
    // 最少使用（最久未用）优先 → 同分随机。
    let min_used = usable.iter().map(|p| pack_last_used(p)).min().unwrap_or(0);
    usable.retain(|p| pack_last_used(p) == min_used);
    usable.sort_by_key(|p| p.id);
    let pack = usable[rng.below(usable.len())];

    // 2) 标题必选。
    let title_id =
        pick_text(&input.titles, &input.target_platforms, &mut rng).ok_or(PickError::NoTitle)?;

    // 3) 图集包需正文。
    let body_id = if pack.kind == "gallery" {
        Some(pick_text(&input.bodies, &input.target_platforms, &mut rng).ok_or(PickError::NoBody)?)
    } else {
        None
    };

    Ok(SetPick {
        pack_id: pack.id,
        title_id,
        body_id,
        content_kind: pack.kind.clone(),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即测试失败，是期望行为
mod tests {
    use super::*;

    fn pack(id: i64, kind: &str, last: &[(&str, i64)]) -> PackCand {
        PackCand {
            id,
            kind: kind.into(),
            lifecycle: "active".into(),
            last_pub: last.iter().map(|(p, t)| (p.to_string(), *t)).collect(),
        }
    }
    fn text(id: i64, plat: &str, use_count: i64) -> TextCand {
        TextCand {
            id,
            platform: plat.into(),
            use_count,
        }
    }

    fn base(packs: Vec<PackCand>, titles: Vec<TextCand>, bodies: Vec<TextCand>) -> PickInput {
        PickInput {
            packs,
            titles,
            bodies,
            target_platforms: vec!["xhs".into()],
            dedup_days: 30,
            now: 1000 * 86_400,
            seed: 1,
        }
    }

    #[test]
    fn video_pack_needs_no_body() {
        let inp = base(
            vec![pack(1, "video", &[])],
            vec![text(10, "general", 0)],
            vec![],
        );
        let r = pick(&inp).unwrap();
        assert_eq!(r.pack_id, 1);
        assert_eq!(r.title_id, 10);
        assert_eq!(r.body_id, None);
        assert_eq!(r.content_kind, "video");
    }

    #[test]
    fn gallery_pack_requires_body() {
        let inp = base(
            vec![pack(1, "gallery", &[])],
            vec![text(10, "general", 0)],
            vec![],
        );
        assert_eq!(pick(&inp), Err(PickError::NoBody));

        let inp2 = base(
            vec![pack(1, "gallery", &[])],
            vec![text(10, "general", 0)],
            vec![text(20, "general", 0)],
        );
        let r = pick(&inp2).unwrap();
        assert_eq!(r.body_id, Some(20));
    }

    #[test]
    fn exhausted_pack_filtered() {
        let now = 1000 * 86_400;
        // 该包在 xhs 5 天前发过（30 天窗口内）→ 用尽（只有一个目标平台）。
        let inp = base(
            vec![pack(1, "video", &[("xhs", now - 5 * 86_400)])],
            vec![text(10, "general", 0)],
            vec![],
        );
        assert_eq!(pick(&inp), Err(PickError::NoPack));
    }

    #[test]
    fn platform_match_preferred_over_general() {
        let inp = base(
            vec![pack(1, "video", &[])],
            vec![text(10, "general", 0), text(11, "xhs", 5)],
            vec![],
        );
        // xhs 匹配优先，即便 use_count 更高。
        let r = pick(&inp).unwrap();
        assert_eq!(r.title_id, 11);
    }

    #[test]
    fn least_used_pack_preferred() {
        let now = 1000 * 86_400;
        let inp = base(
            vec![
                pack(1, "video", &[("douyin", now - 40 * 86_400)]), // 用过（窗口外，可用）但较近
                pack(2, "video", &[]),                              // 从未用
            ],
            vec![text(10, "general", 0)],
            vec![],
        );
        let r = pick(&inp).unwrap();
        assert_eq!(r.pack_id, 2, "从未用的包优先");
    }

    #[test]
    fn deterministic_for_seed() {
        let mk = || {
            base(
                vec![
                    pack(1, "video", &[]),
                    pack(2, "video", &[]),
                    pack(3, "video", &[]),
                ],
                vec![text(10, "general", 0), text(11, "general", 0)],
                vec![],
            )
        };
        let a = pick(&mk()).unwrap();
        let b = pick(&mk()).unwrap();
        assert_eq!(a, b, "同 seed 同输入结果一致");
    }

    // proptest：任意输入不 panic；有可用包+标题时视频包必成功。
    proptest::proptest! {
        #[test]
        fn pick_never_panics(seed: u64, n_packs in 0usize..6, n_titles in 0usize..6) {
            let packs = (0..n_packs).map(|i| pack(i as i64, "video", &[])).collect();
            let titles = (0..n_titles).map(|i| text(100 + i as i64, "general", i as i64)).collect();
            let mut inp = base(packs, titles, vec![]);
            inp.seed = seed;
            let _ = pick(&inp);
        }
    }
}
