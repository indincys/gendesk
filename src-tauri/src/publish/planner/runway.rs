//! 资产跑道（F3）：按分层频率 + 查重窗口推演「还能撑几天」。
//!
//! 静态阈值（素材 < 2 就预警）只能说明「现在少」，说不清「什么时候断」——
//! 一个每天发五个平台的热款和一个每周发一次的冷款，同样剩 3 个包，紧迫程度差一个数量级。
//! 这里把余量换算成倒计时。纯函数，无 IO。

use crate::publish::planner::frequency::{due_skus, FreqRules, SkuFreq};

/// 推演上限（超过这个天数就认为「够用」，不必再算）。
pub const HORIZON_DAYS: i64 = 60;

/// 一个素材包在跑道推演里的状态。
#[derive(Debug, Clone)]
pub struct RunwayPack {
    /// 各平台最近发布时间（Unix 秒）。
    pub last_pub: Vec<(String, i64)>,
}

#[derive(Debug, Clone)]
pub struct RunwayInput {
    pub sku_id: i64,
    /// hot|warm|cold
    pub tier: String,
    /// 该 SKU 生效平台。
    pub platforms: Vec<String>,
    /// 可用（active，未退役）素材包。
    pub packs: Vec<RunwayPack>,
    pub title_count: i64,
    pub body_count: i64,
    /// 该 SKU 是否需要正文（有图集包）。
    pub needs_body: bool,
    pub dedup_days: i64,
    pub rules: FreqRules,
    /// 起算日（`YYYY-MM-DD`）与其 Unix 秒。
    pub start_date: String,
    pub now: i64,
}

/// 推演结果：三池各自还能撑几天。`None` = 超出推演上限（够用）或压根不排期。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunwayView {
    pub material_days: Option<i64>,
    pub title_days: Option<i64>,
    pub body_days: Option<i64>,
}

/// 未来 N 天里，该 SKU 的应发日（相对起算日的偏移天数）。
fn due_offsets(input: &RunwayInput) -> Vec<i64> {
    let Ok(start) = chrono::NaiveDate::parse_from_str(&input.start_date, "%Y-%m-%d") else {
        return Vec::new();
    };
    let freq = [SkuFreq {
        id: input.sku_id,
        tier: input.tier.clone(),
    }];
    (0..HORIZON_DAYS)
        .filter(|off| {
            let d = start + chrono::Duration::days(*off);
            let date = d.format("%Y-%m-%d").to_string();
            due_skus(&date, &freq, &input.rules).contains(&input.sku_id)
        })
        .collect()
}

/// 推演跑道。
///
/// 素材：逐个应发日模拟——当天为每个目标平台挑一个「已出查重窗口」的包用掉；
/// 挑不出来的那天就是断料日。标题/正文不受查重窗口约束，只按「一次发布用一条」轮换，
/// 池子里有 N 条就够用 N 次（用完会重复用最少使用的那条，不算断料——
/// 但重复用同一批文案本身有风险，所以仍按 N 次给出倒计时）。
pub fn runway(input: &RunwayInput) -> RunwayView {
    let offsets = due_offsets(input);
    if offsets.is_empty() || input.platforms.is_empty() {
        // 不排期（冷款没轮到、无平台）→ 不给倒计时，避免虚假紧迫感。
        return RunwayView {
            material_days: None,
            title_days: None,
            body_days: None,
        };
    }
    let window = input.dedup_days.max(0) * 86_400;

    // 素材：模拟消耗。packs[i] 在各平台上的「下次可用时刻」。
    let mut free_at: Vec<Vec<(String, i64)>> =
        input.packs.iter().map(|p| p.last_pub.clone()).collect();
    let mut material_days: Option<i64> = None;

    'days: for off in &offsets {
        let day_ts = input.now + off * 86_400;
        for plat in &input.platforms {
            // 找一个该平台已出窗的包。
            let pick = free_at.iter().position(|last| {
                // 该平台从未发过（None），或已出查重窗口 → 可用。
                match last.iter().find(|(p, _)| p == plat) {
                    None => true,
                    Some((_, t)) => t + window <= day_ts,
                }
            });
            match pick {
                Some(i) => {
                    // 用掉：该包在该平台的最近发布刷新为今天。
                    let entry = &mut free_at[i];
                    match entry.iter_mut().find(|(p, _)| p == plat) {
                        Some(e) => e.1 = day_ts,
                        None => entry.push((plat.clone(), day_ts)),
                    }
                }
                None => {
                    material_days = Some(*off);
                    break 'days;
                }
            }
        }
    }

    // 标题/正文：每次发布（每个应发日 × 每个平台）用一条。
    let per_day = input.platforms.len() as i64;
    let uses_by_day = |count: i64| -> Option<i64> {
        if per_day <= 0 {
            return None;
        }
        let mut used = 0i64;
        for off in &offsets {
            used += per_day;
            if used > count {
                return Some(*off);
            }
        }
        None
    };

    RunwayView {
        material_days,
        title_days: uses_by_day(input.title_count),
        body_days: if input.needs_body {
            uses_by_day(input.body_count)
        } else {
            None
        },
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即测试失败，是期望行为
mod tests {
    use super::*;

    const RULES: FreqRules = FreqRules {
        hot_daily: 1,
        warm_weekly: 3,
        cold_weekly_rotate: 5,
    };

    fn input(tier: &str, packs: usize, titles: i64) -> RunwayInput {
        RunwayInput {
            sku_id: 1,
            tier: tier.into(),
            platforms: vec!["xhs".into()],
            packs: (0..packs)
                .map(|_| RunwayPack { last_pub: vec![] })
                .collect(),
            title_count: titles,
            body_count: 0,
            needs_body: false,
            dedup_days: 30,
            rules: RULES,
            start_date: "2026-07-15".into(),
            now: 1_800_000_000,
        }
    }

    // 热款每天发：3 个包、30 天查重窗口 → 第 4 天就没有出窗的包可用了。
    #[test]
    fn hot_sku_burns_material_fast() {
        let r = runway(&input("hot", 3, 100));
        assert_eq!(
            r.material_days,
            Some(3),
            "第 0/1/2 天各用掉一个包，第 3 天断"
        );
    }

    // 温款每周 3 天：同样 3 个包能撑到下一轮，倒计时明显更长。
    #[test]
    fn warm_sku_lasts_longer_than_hot_with_same_packs() {
        let hot = runway(&input("hot", 3, 100)).material_days.unwrap();
        // None = 超出 60 天推演上限，也算「更长」。
        if let Some(d) = runway(&input("warm", 3, 100)).material_days {
            assert!(d > hot, "温款 {d} 天应长于热款 {hot} 天");
        }
    }

    // 冷款：轮播池没轮到就不排期 → 不给倒计时（避免虚假紧迫感）。
    #[test]
    fn cold_sku_not_in_rotation_has_no_countdown() {
        let mut i = input("cold", 1, 1);
        i.rules = FreqRules {
            cold_weekly_rotate: 0,
            ..RULES
        };
        let r = runway(&i);
        assert_eq!(r.material_days, None);
        assert_eq!(r.title_days, None);
    }

    // 标题池：热款每天 1 个平台 → 3 条标题撑 3 天。
    #[test]
    fn title_runway_counts_uses_per_day() {
        let r = runway(&input("hot", 100, 3));
        assert_eq!(r.title_days, Some(3));
    }

    // 多平台加速消耗：同样 4 条标题，2 个平台只能撑 2 天。
    #[test]
    fn more_platforms_burn_text_faster() {
        let mut i = input("hot", 100, 4);
        i.platforms = vec!["xhs".into(), "douyin".into()];
        assert_eq!(runway(&i).title_days, Some(2));
    }

    // 素材充足时不给倒计时（超出推演上限）。
    #[test]
    fn ample_material_has_no_deadline() {
        let r = runway(&input("hot", 100, 1000));
        assert_eq!(r.material_days, None);
    }

    // 图集 SKU 才算正文跑道。
    #[test]
    fn body_runway_only_when_needed() {
        let mut i = input("hot", 100, 1000);
        i.needs_body = false;
        assert_eq!(runway(&i).body_days, None);
        i.needs_body = true;
        i.body_count = 2;
        assert_eq!(runway(&i).body_days, Some(2));
    }
}
