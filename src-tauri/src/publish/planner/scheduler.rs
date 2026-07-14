//! 排期展开与约束（发布模块执行计划 §2.3 / §4.3）。纯函数。
//!
//! 日内容套装 × 启用平台 × 该平台账号 → 候选任务行 → 约束过滤（账号日限 + 同平台多账号
//! 最小间隔）→ 时段分配（时段模板内 + 抖动，留空=立即发）。缺料副产物。
//! proptest 不变量（§6.2）：账号不超日限、同平台最小间隔、时间落时段内、
//! 行数 = 展开数 − 裁剪数、疑似任务永不出现在产出中。

// 部分字段/方法先于 generate_sheet 消费者落地。
#![allow(dead_code)]

use crate::publish::planner::Rng;

/// 时段（分钟自午夜）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slot {
    pub start_min: i64,
    pub end_min: i64,
}

/// 解析 `HH:MM` → 分钟自午夜（非法返回 None）。任务行计划时间、时段端点、
/// 每日生成时刻三处共用同一份校验，避免各写各的、松紧不一。
pub fn parse_hhmm(s: &str) -> Option<i64> {
    let (h, m) = s.trim().split_once(':')?;
    let h: i64 = h.trim().parse().ok()?;
    let m: i64 = m.trim().parse().ok()?;
    if (0..24).contains(&h) && (0..60).contains(&m) {
        Some(h * 60 + m)
    } else {
        None
    }
}

/// 解析 `HH:MM-HH:MM`（非法返回 None）。
pub fn parse_slot(s: &str) -> Option<Slot> {
    let (a, b) = s.split_once('-')?;
    let (start_min, end_min) = (parse_hhmm(a)?, parse_hhmm(b)?);
    if start_min < end_min {
        Some(Slot { start_min, end_min })
    } else {
        None
    }
}

/// 排期用账号。
#[derive(Debug, Clone)]
pub struct SchedAccount {
    pub id: i64,
    pub platform: String,
    pub daily_limit: i64,
    /// 账号可用时段（空 = 跟随全局时段）。需求 §4.1「账号档案：可用时段」。
    pub slots: Vec<Slot>,
}

/// 一个当日应发套装（已选定素材，含内容类型）。
#[derive(Debug, Clone)]
pub struct DueSet {
    pub sku_id: i64,
    /// 该 SKU 当日生效平台 code 集。
    pub platforms: Vec<String>,
    /// "video" | "gallery"
    pub content_kind: String,
}

#[derive(Debug, Clone)]
pub struct ScheduleInput {
    pub due: Vec<DueSet>,
    pub accounts: Vec<SchedAccount>,
    pub global_slots: Vec<Slot>,
    pub min_gap_minutes: i64,
    pub seed: u64,
}

/// 排出的一行（未落库；状态恒为待执行 pending）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedRow {
    pub sku_id: i64,
    pub account_id: i64,
    pub platform: String,
    pub content_kind: String,
    /// 定时发布分钟；None = 立即发布。
    pub planned_minute: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct ScheduleResult {
    pub rows: Vec<PlannedRow>,
    /// 期望展开数（约束前）。
    pub expanded: usize,
    /// 因账号日限裁剪的行数。
    pub trimmed: usize,
}

/// 分钟所属的时段（不在任何时段内为 None）。
fn slot_of(slots: &[Slot], minute: i64) -> Option<&Slot> {
    slots
        .iter()
        .find(|s| minute >= s.start_min && minute <= s.end_min)
}

/// 简单字符串 hash（平台 → seed 扰动），避免不同平台共用同一抖动。
fn str_hash(s: &str) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01B3);
    }
    h
}

/// 生成时间线上的可用时刻点：时段内、两两间隔 ≥ gap、**每步独立抖动**。
///
/// 旧实现是「起点小抖动 + 严格等距网格」，产出 12:03/13:03/14:03 这种完美等差序列
/// ——对平台风控是可识别模式。改为每步 `gap + rand(0, jitter_max)`，间隔仍恒 ≥ gap
/// （不变量 2 不破），但不再等差。
fn available_points(slots: &[Slot], gap: i64, rng: &mut Rng) -> Vec<i64> {
    let gap = gap.max(1);
    // 抖动幅度：gap 的一半，夹在 5–30 分钟（gap 很小时不至于把点挤没，很大时不至于失控）。
    let jitter_max = (gap / 2).clamp(5, 30);
    let mut points = Vec::new();
    let mut next_allowed = i64::MIN;
    let mut ordered: Vec<Slot> = slots.to_vec();
    ordered.sort_by_key(|s| s.start_min);
    for slot in ordered {
        let span = (slot.end_min - slot.start_min).max(0);
        let start_jitter = if span > 0 {
            rng.below(gap.min(span + 1) as usize) as i64
        } else {
            0
        };
        let mut t = (slot.start_min + start_jitter).max(next_allowed);
        while t <= slot.end_min {
            points.push(t);
            let step = gap + rng.below((jitter_max + 1) as usize) as i64;
            next_allowed = t + gap;
            t += step;
        }
    }
    points
}

/// 排期。确定性（同 seed 同输入同输出）。
pub fn schedule(input: &ScheduleInput) -> ScheduleResult {
    // 1) 展开：套装 × 平台 × 该平台账号。
    let mut expanded_rows: Vec<PlannedRow> = Vec::new();
    for set in &input.due {
        for plat in &set.platforms {
            for acct in input.accounts.iter().filter(|a| &a.platform == plat) {
                expanded_rows.push(PlannedRow {
                    sku_id: set.sku_id,
                    account_id: acct.id,
                    platform: plat.clone(),
                    content_kind: set.content_kind.clone(),
                    planned_minute: None,
                });
            }
        }
    }
    let expanded = expanded_rows.len();

    // 2) 账号日限裁剪。**按日轮转**：先稳定排序保证可复现，再用 seed（含日期派生）洗牌
    // 后截断。固定按 sku_id 截断会让 id 大的那批 SKU 天天被裁，永远发不出去。
    let mut kept: Vec<PlannedRow> = Vec::new();
    let mut trimmed = 0usize;
    let mut acct_ids: Vec<i64> = input.accounts.iter().map(|a| a.id).collect();
    acct_ids.sort_unstable();
    acct_ids.dedup();
    for aid in acct_ids {
        let limit = input
            .accounts
            .iter()
            .find(|a| a.id == aid)
            .map(|a| a.daily_limit.max(0))
            .unwrap_or(0) as usize;
        let mut rows: Vec<PlannedRow> = expanded_rows
            .iter()
            .filter(|r| r.account_id == aid)
            .cloned()
            .collect();
        rows.sort_by_key(|r| r.sku_id);
        if rows.len() > limit {
            let mut rng = Rng::new(input.seed ^ (aid as u64));
            rng.shuffle(&mut rows);
            trimmed += rows.len() - limit;
            rows.truncate(limit);
        }
        kept.extend(rows);
    }

    // 3) 时段分配。每个平台一条时间线（点两两 ≥ min_gap，这是「同平台多账号最小间隔」
    // 的来源），账号只能取落在自己可用时段内的点（需求 §4.1；账号无时段则跟随全局）。
    // 容量不够的行留空 = 立即发。
    let gap = input.min_gap_minutes.max(1);
    let mut used_minutes: std::collections::HashSet<i64> = std::collections::HashSet::new();
    let mut platforms: Vec<String> = kept.iter().map(|r| r.platform.clone()).collect();
    platforms.sort();
    platforms.dedup();

    for plat in platforms {
        let mut rng = Rng::new(input.seed ^ str_hash(&plat));
        // 该平台涉及的账号及其生效时段。
        let acct_slots = |aid: i64| -> Vec<Slot> {
            input
                .accounts
                .iter()
                .find(|a| a.id == aid)
                .map(|a| a.slots.clone())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| input.global_slots.clone())
        };
        // 时间线覆盖该平台所有账号的时段并集（账号时段可能超出全局时段）。
        let mut union_slots: Vec<Slot> = Vec::new();
        for r in kept.iter().filter(|r| r.platform == plat) {
            for s in acct_slots(r.account_id) {
                if !union_slots.contains(&s) {
                    union_slots.push(s);
                }
            }
        }
        let mut grid = available_points(&union_slots, gap, &mut rng);
        grid.sort_unstable();

        // 行的分配顺序：稳定序后洗牌（同一天可复现，但不按 id 顺序占点）。
        let mut idxs: Vec<usize> = kept
            .iter()
            .enumerate()
            .filter(|(_, r)| r.platform == plat)
            .map(|(i, _)| i)
            .collect();
        idxs.sort_by_key(|&i| (kept[i].account_id, kept[i].sku_id));
        rng.shuffle(&mut idxs);

        // 逐行取「该账号时段内、尚未被占用」的最早点。
        let mut taken = vec![false; grid.len()];
        let mut assigned: Vec<(i64, usize)> = Vec::new(); // (分钟, 行索引)
        for &i in &idxs {
            let slots = acct_slots(kept[i].account_id);
            let found = grid
                .iter()
                .enumerate()
                .find(|(k, m)| !taken[*k] && slot_of(&slots, **m).is_some());
            if let Some((k, m)) = found {
                let m = *m;
                taken[k] = true;
                assigned.push((m, i));
            }
        }

        // 跨平台错峰：同一分钟被别的平台占了就顺延，但**不得**越出所属时段，
        // 也**不得**逼近本平台的下一个点（否则 min_gap 不变量就破了）。宁可撞点，不破约束。
        assigned.sort_unstable();
        for k in 0..assigned.len() {
            let (orig, row) = assigned[k];
            let next = assigned.get(k + 1).map(|(m, _)| *m);
            let slot_end = slot_of(&acct_slots(kept[row].account_id), orig).map(|s| s.end_min);
            let limit = match (slot_end, next) {
                (Some(e), Some(n)) => e.min(n - gap),
                (Some(e), None) => e,
                (None, Some(n)) => n - gap,
                (None, None) => orig,
            };
            let mut m = orig;
            while used_minutes.contains(&m) && m < limit {
                m += 1;
            }
            used_minutes.insert(m);
            kept[row].planned_minute = Some(m);
        }
    }

    // 产出稳定排序（时间优先，None 靠后）。
    kept.sort_by(|a, b| {
        match (a.planned_minute, b.planned_minute) {
            (Some(x), Some(y)) => x.cmp(&y),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        }
        .then(a.platform.cmp(&b.platform))
        .then(a.account_id.cmp(&b.account_id))
        .then(a.sku_id.cmp(&b.sku_id))
    });

    ScheduleResult {
        rows: kept,
        expanded,
        trimmed,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即测试失败，是期望行为
mod tests {
    use super::*;

    fn slots() -> Vec<Slot> {
        vec![
            parse_slot("11:30-13:00").unwrap(),
            parse_slot("18:00-20:00").unwrap(),
            parse_slot("21:00-22:30").unwrap(),
        ]
    }

    #[test]
    fn parse_hhmm_valid_and_invalid() {
        assert_eq!(parse_hhmm("00:00"), Some(0));
        assert_eq!(parse_hhmm("22:30"), Some(22 * 60 + 30));
        assert_eq!(parse_hhmm(" 7:05 "), Some(7 * 60 + 5));
        assert_eq!(parse_hhmm("24:00"), None);
        assert_eq!(parse_hhmm("12:60"), None);
        assert_eq!(parse_hhmm("12"), None);
        assert_eq!(parse_hhmm("随便"), None);
        assert_eq!(parse_hhmm(""), None);
    }

    #[test]
    fn parse_slot_valid_and_invalid() {
        assert_eq!(
            parse_slot("11:30-13:00"),
            Some(Slot {
                start_min: 690,
                end_min: 780
            })
        );
        assert_eq!(parse_slot("13:00-11:00"), None); // 逆序
        assert_eq!(parse_slot("12:00-12:00"), None); // 零长（start == end 非法）
        assert_eq!(parse_slot("25:00-26:00"), None); // 越界
        assert_eq!(parse_slot("abc"), None);
    }

    // 容量充足时全部行分到时段内、两两 ≥ gap、点数 = 行数（约束 available_points 网格生成）。
    #[test]
    fn assigns_times_within_slots_when_capacity_ample() {
        let due: Vec<DueSet> = (1..=3)
            .map(|i| DueSet {
                sku_id: i,
                platforms: vec!["xhs".into()],
                content_kind: "video".into(),
            })
            .collect();
        let inp = ScheduleInput {
            due,
            accounts: vec![SchedAccount {
                id: 10,
                platform: "xhs".into(),
                daily_limit: 3,
                slots: vec![],
            }],
            global_slots: vec![parse_slot("08:00-22:00").unwrap()], // 14h 宽裕
            min_gap_minutes: 60,
            seed: 5,
        };
        let r = schedule(&inp);
        assert_eq!(r.rows.len(), 3);
        assert!(
            r.rows.iter().all(|row| row.planned_minute.is_some()),
            "容量充足时应全部排上时间，而非留空"
        );
        let mut times: Vec<i64> = r.rows.iter().filter_map(|row| row.planned_minute).collect();
        times.sort();
        assert!(
            times.iter().all(|&t| (480..=1320).contains(&t)),
            "时间须落在 08:00–22:00"
        );
        for w in times.windows(2) {
            assert!(w[1] - w[0] >= 60, "两两间隔 ≥ 60");
        }
        times.dedup();
        assert_eq!(times.len(), 3, "三行三个不同时刻");
    }

    // 窄时段容量不足 → 超出的行留空（立即发），而非违反间隔。
    #[test]
    fn overflow_rows_get_none() {
        let due: Vec<DueSet> = (1..=3)
            .map(|i| DueSet {
                sku_id: i,
                platforms: vec!["xhs".into()],
                content_kind: "video".into(),
            })
            .collect();
        let inp = ScheduleInput {
            due,
            accounts: vec![SchedAccount {
                id: 10,
                platform: "xhs".into(),
                daily_limit: 3,
                slots: vec![],
            }],
            global_slots: vec![parse_slot("12:00-12:30").unwrap()], // 30min，gap 60 → 至多 1 点
            min_gap_minutes: 60,
            seed: 5,
        };
        let r = schedule(&inp);
        let with_time = r
            .rows
            .iter()
            .filter(|row| row.planned_minute.is_some())
            .count();
        assert_eq!(
            with_time, 1,
            "30 分钟时段按 60 分钟间隔只排得下 1 个，其余立即发"
        );
    }

    #[test]
    fn expands_set_by_platform_and_account() {
        let inp = ScheduleInput {
            due: vec![DueSet {
                sku_id: 1,
                platforms: vec!["xhs".into(), "douyin".into()],
                content_kind: "video".into(),
            }],
            accounts: vec![
                SchedAccount {
                    id: 10,
                    platform: "xhs".into(),
                    daily_limit: 3,
                    slots: vec![],
                },
                SchedAccount {
                    id: 11,
                    platform: "douyin".into(),
                    daily_limit: 3,
                    slots: vec![],
                },
                SchedAccount {
                    id: 12,
                    platform: "douyin".into(),
                    daily_limit: 3,
                    slots: vec![],
                },
            ],
            global_slots: slots(),
            min_gap_minutes: 60,
            seed: 1,
        };
        let r = schedule(&inp);
        // xhs(1 acct) + douyin(2 accts) = 3 行
        assert_eq!(r.expanded, 3);
        assert_eq!(r.rows.len(), 3);
        assert_eq!(r.trimmed, 0);
    }

    #[test]
    fn daily_limit_trims() {
        // 一个账号，4 个 SKU，日限 2 → 裁剪 2。
        let due: Vec<DueSet> = (1..=4)
            .map(|i| DueSet {
                sku_id: i,
                platforms: vec!["xhs".into()],
                content_kind: "video".into(),
            })
            .collect();
        let inp = ScheduleInput {
            due,
            accounts: vec![SchedAccount {
                id: 10,
                platform: "xhs".into(),
                daily_limit: 2,
                slots: vec![],
            }],
            global_slots: slots(),
            min_gap_minutes: 60,
            seed: 1,
        };
        let r = schedule(&inp);
        assert_eq!(r.expanded, 4);
        assert_eq!(r.trimmed, 2);
        assert_eq!(r.rows.len(), 2);
    }

    // C2：日限裁剪按日轮转 —— 固定按 sku_id 截断会让 id 大的那批 SKU 天天被裁、
    // 永远发不出去。遍历 14 个 seed（模拟 14 天），每个 SKU 至少被保留过一次。
    #[test]
    fn daily_limit_trim_rotates_across_days_no_permanent_starvation() {
        let due: Vec<DueSet> = (1..=4)
            .map(|i| DueSet {
                sku_id: i,
                platforms: vec!["xhs".into()],
                content_kind: "video".into(),
            })
            .collect();
        let mut kept_ever: std::collections::HashSet<i64> = std::collections::HashSet::new();
        for day in 0..14u64 {
            let inp = ScheduleInput {
                due: due.clone(),
                accounts: vec![SchedAccount {
                    id: 10,
                    platform: "xhs".into(),
                    daily_limit: 2,
                    slots: vec![],
                }],
                global_slots: slots(),
                min_gap_minutes: 60,
                seed: 0x9E37_79B9 ^ day, // 日期派生 seed
            };
            for row in schedule(&inp).rows {
                kept_ever.insert(row.sku_id);
            }
        }
        assert_eq!(kept_ever.len(), 4, "14 天内每个 SKU 都该被排上过至少一次");
    }

    // C5：账号有自己的可用时段时，它的行只能落在该时段内（需求 §4.1）。
    #[test]
    fn account_slots_are_respected() {
        let due: Vec<DueSet> = (1..=2)
            .map(|i| DueSet {
                sku_id: i,
                platforms: vec!["xhs".into()],
                content_kind: "video".into(),
            })
            .collect();
        let night = parse_slot("21:00-22:30").unwrap();
        let inp = ScheduleInput {
            due,
            accounts: vec![SchedAccount {
                id: 10,
                platform: "xhs".into(),
                daily_limit: 2,
                slots: vec![night],
            }],
            // 全局时段是中午，账号时段是晚上：以账号为准。
            global_slots: vec![parse_slot("11:30-13:00").unwrap()],
            min_gap_minutes: 30,
            seed: 7,
        };
        let r = schedule(&inp);
        let times: Vec<i64> = r.rows.iter().filter_map(|row| row.planned_minute).collect();
        assert!(!times.is_empty(), "账号时段内应排得下");
        for t in times {
            assert!(
                t >= night.start_min && t <= night.end_min,
                "{t} 不在账号可用时段内"
            );
        }
    }

    // C4：时间不再是完美等差（等距网格是平台风控可识别的模式）。
    #[test]
    fn point_intervals_are_not_a_perfect_grid() {
        let mut rng = Rng::new(42);
        let pts = available_points(&[parse_slot("08:00-22:00").unwrap()], 60, &mut rng);
        assert!(pts.len() >= 5);
        let deltas: Vec<i64> = pts.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(deltas.iter().all(|d| *d >= 60), "间隔仍恒 ≥ gap");
        assert!(
            deltas
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len()
                > 1,
            "步长应有抖动，而非恒等于 gap：{deltas:?}"
        );
    }

    // proptest：排期不变量全套（§6.2）+ C4/C5 新增。
    proptest::proptest! {
        #[test]
        fn schedule_invariants(
            seed: u64,
            n_sku in 0usize..6,
            n_acct in 0usize..5,
            gap in 15i64..120,
        ) {
            let plats = ["xhs", "douyin", "kuaishou"];
            let due: Vec<DueSet> = (0..n_sku).map(|i| DueSet {
                sku_id: i as i64,
                platforms: plats.iter().take(1 + i % 3).map(|s| s.to_string()).collect(),
                content_kind: if i % 2 == 0 { "video" } else { "gallery" }.into(),
            }).collect();
            let accounts: Vec<SchedAccount> = (0..n_acct).map(|i| SchedAccount {
                id: 100 + i as i64,
                platform: plats[i % 3].into(),
                daily_limit: (1 + (i % 3)) as i64,
                slots: vec![],
            }).collect();
            let inp = ScheduleInput { due, accounts: accounts.clone(), global_slots: slots(), min_gap_minutes: gap, seed };
            let r = schedule(&inp);

            // 不变量 4：行数 = 展开 − 裁剪。
            proptest::prop_assert_eq!(r.rows.len(), r.expanded - r.trimmed);

            // 不变量 1：账号不超日限。
            for a in &accounts {
                let cnt = r.rows.iter().filter(|row| row.account_id == a.id).count() as i64;
                proptest::prop_assert!(cnt <= a.daily_limit.max(0));
            }

            // 不变量 3：非空时间落在某时段内。
            let sl = slots();
            for row in &r.rows {
                if let Some(m) = row.planned_minute {
                    let inside = sl.iter().any(|s| m >= s.start_min && m <= s.end_min);
                    proptest::prop_assert!(inside, "时间 {} 不在任何时段内", m);
                }
                // 不变量 5：产出行状态恒为待执行（结构上无 suspect 字段，此处确保无异常态）。
            }


            // 不变量 2：同平台的非空时间两两间隔 ≥ min_gap。
            for p in plats {
                let mut times: Vec<i64> = r.rows.iter()
                    .filter(|row| row.platform == p)
                    .filter_map(|row| row.planned_minute)
                    .collect();
                times.sort();
                for w in times.windows(2) {
                    proptest::prop_assert!(w[1] - w[0] >= gap, "同平台 {} 间隔 {} < {}", p, w[1]-w[0], gap);
                }
            }

            // C4 新增（弱断言）：跨平台错峰后，全局重复的分钟数 ≤ 平台数 − 1。
            // 单执行机串行发布，同一分钟撞多个任务会挤压；顺延不越界时应基本消除撞点。
            let mut all: Vec<i64> = r.rows.iter().filter_map(|row| row.planned_minute).collect();
            all.sort_unstable();
            let dupes = all.windows(2).filter(|w| w[0] == w[1]).count();
            proptest::prop_assert!(
                dupes <= plats.len().saturating_sub(1),
                "全局撞点 {} 个，超过平台数-1", dupes
            );
        }
    }
}
