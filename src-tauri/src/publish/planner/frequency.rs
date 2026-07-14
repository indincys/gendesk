//! 分层发布频率（发布模块执行计划 §4.1/§2.3）。纯函数、日期驱动（无持久游标）。
//!
//! 热款：每日发（hot_daily≥1）。温款：每周 N 天（由 SKU id 派生的固定周内日）。
//! 冷款：轮播池每周轮出 M 个（按周序滑窗），轮到的当周发一次（派生周内日）。

// 频率派生函数先于 generate_sheet 消费者落地。
#![allow(dead_code)]

use chrono::{Datelike, NaiveDate};

/// 分层频率规则（对应 PublishSettings.tier_rules 扁平字段）。
#[derive(Debug, Clone, Copy)]
pub struct FreqRules {
    /// 热款每日发布开关：`>= 1` 即每天一次（× 平台集）。**大于 1 无额外效果**——
    /// 同 SKU 同日多套装是 V2 的事；设置层已把它夹到 0/1，UI 是开关。
    pub hot_daily: i64,
    pub warm_weekly: i64,
    pub cold_weekly_rotate: i64,
}

/// SKU 频率输入。
#[derive(Debug, Clone)]
pub struct SkuFreq {
    pub id: i64,
    /// hot|warm|cold
    pub tier: String,
}

fn id_hash(id: i64) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64 ^ (id as u64);
    h = h.wrapping_mul(0x0000_0100_0000_01B3);
    h ^ (h >> 29)
}

/// 由 id 派生 n 个固定周内日（0=周一…6=周日）。n≥7 → 全周。
fn chosen_weekdays(id: i64, n: i64) -> Vec<u32> {
    if n <= 0 {
        return Vec::new();
    }
    if n >= 7 {
        return (0..7).collect();
    }
    // 从洗牌后的 0..7 取前 n 个（id 派生的确定顺序）。
    let mut days: Vec<u32> = (0..7).collect();
    let mut seed = id_hash(id);
    for i in (1..days.len()).rev() {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let j = (seed >> 33) as usize % (i + 1);
        days.swap(i, j);
    }
    days.truncate(n as usize);
    days.sort_unstable();
    days
}

/// 单个 id 派生的「当周唯一发布日」（冷款用）。
fn single_weekday(id: i64) -> u32 {
    (id_hash(id) % 7) as u32
}

/// 自纪元的周序（用于冷款轮播滑窗）。
fn week_index(d: NaiveDate) -> i64 {
    d.num_days_from_ce() as i64 / 7
}

/// 当日应发的 sku_id 列表。`date`：`YYYY-MM-DD`。非法日期返回空。
pub fn due_skus(date: &str, skus: &[SkuFreq], rules: &FreqRules) -> Vec<i64> {
    let Ok(d) = NaiveDate::parse_from_str(date, "%Y-%m-%d") else {
        return Vec::new();
    };
    let weekday = d.weekday().num_days_from_monday(); // 0=周一
    let wk = week_index(d);

    let mut due = Vec::new();

    // 冷款池（排序），本周滑窗。
    let mut cold: Vec<i64> = skus
        .iter()
        .filter(|s| s.tier == "cold")
        .map(|s| s.id)
        .collect();
    cold.sort_unstable();
    let cold_window: Vec<i64> = if rules.cold_weekly_rotate <= 0 || cold.is_empty() {
        Vec::new()
    } else {
        let len = cold.len();
        let m = (rules.cold_weekly_rotate as usize).min(len);
        // 窗口每周前移 **m 个**（而不是 1 个）：前移 1 会让相邻两周重叠 m-1 个，
        // 全池覆盖周期从 ⌈len/m⌉ 周退化成 len 周。
        let start = ((wk.wrapping_mul(m as i64)).rem_euclid(len as i64)) as usize;
        (0..m).map(|k| cold[(start + k) % len]).collect()
    };

    for s in skus {
        let is_due = match s.tier.as_str() {
            "hot" => rules.hot_daily >= 1,
            "warm" => chosen_weekdays(s.id, rules.warm_weekly).contains(&weekday),
            "cold" => cold_window.contains(&s.id) && single_weekday(s.id) == weekday,
            _ => false,
        };
        if is_due {
            due.push(s.id);
        }
    }
    due
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即测试失败，是期望行为
mod tests {
    use super::*;

    const RULES: FreqRules = FreqRules {
        hot_daily: 1,
        warm_weekly: 3,
        cold_weekly_rotate: 2,
    };

    fn sku(id: i64, tier: &str) -> SkuFreq {
        SkuFreq {
            id,
            tier: tier.into(),
        }
    }

    // 一周七天遍历，统计某 SKU 被排的天数。
    fn due_days_in_week(id: i64, skus: &[SkuFreq], rules: &FreqRules) -> usize {
        // 2026-07-13 是周一。
        (0..7)
            .filter(|off| {
                let d =
                    NaiveDate::from_ymd_opt(2026, 7, 13).unwrap() + chrono::Duration::days(*off);
                let date = d.format("%Y-%m-%d").to_string();
                due_skus(&date, skus, rules).contains(&id)
            })
            .count()
    }

    #[test]
    fn hot_due_every_day() {
        let skus = vec![sku(1, "hot")];
        assert_eq!(due_days_in_week(1, &skus, &RULES), 7);
    }

    #[test]
    fn warm_due_n_days_per_week() {
        let skus = vec![sku(2, "warm")];
        assert_eq!(due_days_in_week(2, &skus, &RULES), 3, "温款每周 3 天");
    }

    #[test]
    fn warm_weekly_zero_never_due() {
        let skus = vec![sku(2, "warm")];
        let rules = FreqRules {
            warm_weekly: 0,
            ..RULES
        };
        assert_eq!(due_days_in_week(2, &skus, &rules), 0);
    }

    /// 与 `week_index` 边界对齐的起始日（每 7 天一个 week_index 块）。
    fn week_aligned_base() -> NaiveDate {
        let mut d = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        while d.num_days_from_ce() % 7 != 0 {
            d += chrono::Duration::days(1);
        }
        d
    }

    /// 第 w 个 week_index 块内被轮到的冷款集合（块内 7 天含每个周内日各一次）。
    fn cold_window_of_week(
        base: NaiveDate,
        w: i64,
        skus: &[SkuFreq],
        rules: &FreqRules,
    ) -> Vec<i64> {
        let mut seen = Vec::new();
        for off in 0..7 {
            let d = base + chrono::Duration::days(w * 7 + off);
            let date = d.format("%Y-%m-%d").to_string();
            for id in due_skus(&date, skus, rules) {
                if !seen.contains(&id) {
                    seen.push(id);
                }
            }
        }
        seen
    }

    // C3：窗口每周前移 M 个 → 覆盖周期 ⌈len/M⌉ 周。5 冷款 × 每周 2 → 3 周内全覆盖。
    // 旧实现每周只前移 1 位，需要 5 周（相邻两周重叠 1 个），全池覆盖被拖慢。
    #[test]
    fn cold_rotation_covers_pool_in_ceil_len_over_m_weeks() {
        let skus: Vec<SkuFreq> = (1..=5).map(|i| sku(i, "cold")).collect();
        let base = week_aligned_base();
        let mut seen = std::collections::HashSet::new();
        for w in 0..3 {
            for id in cold_window_of_week(base, w, &skus, &RULES) {
                seen.insert(id);
            }
        }
        assert_eq!(seen.len(), 5, "5 个冷款、每周 2 个 → 3 周内应全部轮到一遍");
    }

    // 连续两周窗口的重叠：整除时为 0；非整除时首尾衔接最多重叠 m-1 个（宽容版断言）。
    #[test]
    fn adjacent_weeks_barely_overlap() {
        let skus: Vec<SkuFreq> = (1..=5).map(|i| sku(i, "cold")).collect();
        let base = week_aligned_base();
        let m = RULES.cold_weekly_rotate as usize;
        for w in 0..8 {
            let a = cold_window_of_week(base, w, &skus, &RULES);
            let b = cold_window_of_week(base, w + 1, &skus, &RULES);
            let overlap = a.iter().filter(|id| b.contains(id)).count();
            assert!(
                overlap <= m.saturating_sub(1),
                "第 {w} 周与下周重叠 {overlap} 个（上限 {}）",
                m - 1
            );
        }
    }

    #[test]
    fn cold_due_at_most_once_per_week() {
        let skus: Vec<SkuFreq> = (1..=5).map(|i| sku(i, "cold")).collect();
        // 每个冷款一周内至多被排 1 天。
        for id in 1..=5 {
            assert!(due_days_in_week(id, &skus, &RULES) <= 1);
        }
    }

    #[test]
    fn invalid_date_empty() {
        assert!(due_skus("not-a-date", &[sku(1, "hot")], &RULES).is_empty());
    }
}
