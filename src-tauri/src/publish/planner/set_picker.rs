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
    /// 所选包在**查重窗口内**已发过的目标平台。调用方必须把这些平台从当日展开中剔除
    /// ——否则「同素材包同平台 30 天」这条硬约束就被突破了（需求 §2.4）。
    /// 正常（有完全出窗的包可选）时为空。
    pub conflicted_platforms: Vec<String>,
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

/// 该包在查重窗口内**已发过**的目标平台（这些平台今天不能再用这个包）。
fn conflicted_platforms(
    p: &PackCand,
    targets: &[String],
    dedup_days: i64,
    now: i64,
) -> Vec<String> {
    let window = dedup_days.max(0) * 86_400;
    targets
        .iter()
        .filter(|plat| {
            p.last_pub
                .iter()
                .find(|(pl, _)| &pl == plat)
                .is_some_and(|(_, t)| t + window > now)
        })
        .cloned()
        .collect()
}

/// 素材包是否「已用尽」：全部目标平台都在查重窗口内。
fn pack_exhausted(p: &PackCand, targets: &[String], dedup_days: i64, now: i64) -> bool {
    if targets.is_empty() {
        return false;
    }
    conflicted_platforms(p, targets, dedup_days, now).len() == targets.len()
}

/// 素材包「最近使用时间」：所有平台里的最大发布时间（无记录 = 0，最久未用）。
fn pack_last_used(p: &PackCand) -> i64 {
    p.last_pub.iter().map(|(_, t)| *t).max().unwrap_or(0)
}

/// 从候选里按「最少使用 → 同分随机」选一条文本。
///
/// 三级：命中目标平台标签 → 「通用」标签 → 全部。第二级是关键：
/// 回退到「全部」会把抖音标签的标题发到小红书（需求 §3.3 要求回退**通用**）；
/// 第三级只是保底不缺料（宁可用错平台的文案，也不让整个 SKU 因此排不出来）。
fn pick_text(cands: &[TextCand], targets: &[String], rng: &mut Rng) -> Option<i64> {
    if cands.is_empty() {
        return None;
    }
    let matched: Vec<&TextCand> = cands
        .iter()
        .filter(|c| targets.iter().any(|t| t == &c.platform))
        .collect();
    let general: Vec<&TextCand> = cands
        .iter()
        .filter(|c| c.platform == crate::publish::platform::GENERAL_TAG)
        .collect();
    let pool: Vec<&TextCand> = if !matched.is_empty() {
        matched
    } else if !general.is_empty() {
        general
    } else {
        cands.iter().collect()
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
///
/// 选包三级偏好：
/// 1. **所有目标平台都出窗**的包（理想）；
/// 2. 没有 1 时，退而取仍有平台可用的包，并把窗口内的平台放进
///    [`SetPick::conflicted_platforms`]——调用方据此剔除这些平台，窗口约束在任何
///    路径上都不被突破；
/// 3. 全部用尽 → [`PickError::NoPack`]。
pub fn pick(input: &PickInput) -> Result<SetPick, PickError> {
    let mut rng = Rng::new(input.seed);

    // 1) 可用素材包：仅 active（new 待人工过目、retired 已退役），且未完全用尽。
    let usable: Vec<&PackCand> = input
        .packs
        .iter()
        .filter(|p| p.lifecycle == "active")
        .filter(|p| !pack_exhausted(p, &input.target_platforms, input.dedup_days, input.now))
        .collect();
    if usable.is_empty() {
        return Err(PickError::NoPack);
    }

    // 首选完全无冲突的包；没有才接受「部分平台冲突」的包。
    let clean: Vec<&PackCand> = usable
        .iter()
        .copied()
        .filter(|p| {
            conflicted_platforms(p, &input.target_platforms, input.dedup_days, input.now).is_empty()
        })
        .collect();
    let mut pool = if clean.is_empty() { usable } else { clean };

    // 最少使用（最久未用）优先 → 同分随机。
    let min_used = pool.iter().map(|p| pack_last_used(p)).min().unwrap_or(0);
    pool.retain(|p| pack_last_used(p) == min_used);
    pool.sort_by_key(|p| p.id);
    let pack = pool[rng.below(pool.len())];
    let conflicted =
        conflicted_platforms(pack, &input.target_platforms, input.dedup_days, input.now);

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
        conflicted_platforms: conflicted,
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

    // C1：只有部分平台出窗的包被选中时，冲突平台必须被报出来（供 generate_sheet 剔除）。
    // 旧行为：包只要有一个平台没发过就整包可选，然后展开到**全部**平台——包括 5 天前
    // 刚发过的那个，直接违反「同素材包同平台 30 天」。
    #[test]
    fn partially_used_pack_reports_conflicted_platforms() {
        let now = 1000 * 86_400;
        let mut inp = base(
            // xhs 5 天前发过（窗口内），douyin 从未发过。
            vec![pack(1, "video", &[("xhs", now - 5 * 86_400)])],
            vec![text(10, "general", 0)],
            vec![],
        );
        inp.target_platforms = vec!["xhs".into(), "douyin".into()];
        let r = pick(&inp).unwrap();
        assert_eq!(r.pack_id, 1);
        assert_eq!(
            r.conflicted_platforms,
            vec!["xhs".to_string()],
            "xhs 在窗口内，今天不能再用这个包发 xhs"
        );
    }

    // 有完全出窗的包时优先选它，且不报冲突。
    #[test]
    fn clean_pack_preferred_over_partially_used() {
        let now = 1000 * 86_400;
        let mut inp = base(
            vec![
                pack(1, "video", &[("xhs", now - 5 * 86_400)]), // 部分冲突
                pack(2, "video", &[]),                          // 完全干净
            ],
            vec![text(10, "general", 0)],
            vec![],
        );
        inp.target_platforms = vec!["xhs".into(), "douyin".into()];
        let r = pick(&inp).unwrap();
        assert_eq!(r.pack_id, 2);
        assert!(r.conflicted_platforms.is_empty());
    }

    // C1：平台无命中时回退「通用」，而不是全部（否则抖音标题会发到小红书）。
    #[test]
    fn text_falls_back_to_general_not_to_all() {
        let inp = base(
            vec![pack(1, "video", &[])],
            vec![text(10, "douyin", 0), text(11, "general", 9)],
            vec![],
        );
        // 目标平台 xhs：抖音标签的条目不该被选中，即便它用得更少。
        let r = pick(&inp).unwrap();
        assert_eq!(r.title_id, 11, "应回退到通用标签，而非抖音标签");
    }

    // 连通用都没有 → 保底选全部（宁可用错平台的文案，也不让 SKU 排不出来）。
    #[test]
    fn text_last_resort_uses_any_when_no_general() {
        let inp = base(
            vec![pack(1, "video", &[])],
            vec![text(10, "douyin", 0)],
            vec![],
        );
        assert_eq!(pick(&inp).unwrap().title_id, 10);
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

        // C1 硬不变量：把 conflicted_platforms 剔除后，**实际会展开的每个平台**上，
        // 所选包都已出查重窗口。这条不变量守的是需求 §2.4 的「同素材包同平台 30 天」。
        #[test]
        fn picked_pack_is_out_of_window_on_every_expanded_platform(
            seed: u64,
            n_packs in 1usize..5,
            days in 0i64..60,
            dedup in 1i64..60,
        ) {
            let now = 1000 * 86_400;
            let plats = ["xhs", "douyin", "kuaishou"];
            // 每个包在若干平台上有一次「days 天前」的发布记录。
            let packs: Vec<PackCand> = (0..n_packs)
                .map(|i| {
                    let last: Vec<(&str, i64)> = plats
                        .iter()
                        .enumerate()
                        .filter(|(j, _)| (i + j) % 2 == 0)
                        .map(|(_, p)| (*p, now - days * 86_400))
                        .collect();
                    pack(i as i64, "video", &last)
                })
                .collect();
            let mut inp = base(packs.clone(), vec![text(10, "general", 0)], vec![]);
            inp.target_platforms = plats.iter().map(|s| s.to_string()).collect();
            inp.dedup_days = dedup;
            inp.now = now;
            inp.seed = seed;

            if let Ok(r) = pick(&inp) {
                let picked = packs.iter().find(|p| p.id == r.pack_id).expect("选中的包必在候选里");
                let window = dedup * 86_400;
                for plat in inp.target_platforms.iter().filter(|p| !r.conflicted_platforms.contains(p)) {
                    if let Some((_, t)) = picked.last_pub.iter().find(|(pl, _)| pl == plat) {
                        proptest::prop_assert!(
                            t + window <= now,
                            "平台 {} 仍在查重窗口内却被展开（last={} window={}）", plat, t, window
                        );
                    }
                }
                // 至少还剩一个平台可发，否则该包应判为用尽（NoPack）。
                proptest::prop_assert!(
                    r.conflicted_platforms.len() < inp.target_platforms.len(),
                    "完全用尽的包不该被选中"
                );
            }
        }
    }
}
