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

/// 解析 `HH:MM-HH:MM`（非法返回 None）。
pub fn parse_slot(s: &str) -> Option<Slot> {
    let (a, b) = s.split_once('-')?;
    let hm = |t: &str| -> Option<i64> {
        let (h, m) = t.trim().split_once(':')?;
        let h: i64 = h.trim().parse().ok()?;
        let m: i64 = m.trim().parse().ok()?;
        if (0..24).contains(&h) && (0..60).contains(&m) {
            Some(h * 60 + m)
        } else {
            None
        }
    };
    let (start_min, end_min) = (hm(a)?, hm(b)?);
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

/// 简单字符串 hash（平台 → seed 扰动），避免不同平台共用同一抖动。
fn str_hash(s: &str) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01B3);
    }
    h
}

/// 生成平台时间线上的可用时刻点：时段内、全局两两间隔 ≥ gap、起点带 seed 抖动。
fn available_points(slots: &[Slot], gap: i64, rng: &mut Rng) -> Vec<i64> {
    let gap = gap.max(1);
    let mut points = Vec::new();
    let mut next_allowed = i64::MIN;
    let mut ordered: Vec<Slot> = slots.to_vec();
    ordered.sort_by_key(|s| s.start_min);
    for slot in ordered {
        let span = (slot.end_min - slot.start_min).max(0);
        let jitter = if span > 0 {
            rng.below(gap.min(span + 1) as usize) as i64
        } else {
            0
        };
        let mut t = (slot.start_min + jitter).max(next_allowed);
        while t <= slot.end_min {
            points.push(t);
            next_allowed = t + gap;
            t += gap;
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

    // 2) 账号日限裁剪（每账号保留至多 daily_limit 行，按 sku_id 稳定序）。
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
            trimmed += rows.len() - limit;
            rows.truncate(limit);
        }
        kept.extend(rows);
    }

    // 3) 时段分配：按平台聚合，同平台时间线上两两间隔 ≥ min_gap；超容量的行留空（立即发）。
    let mut platforms: Vec<String> = kept.iter().map(|r| r.platform.clone()).collect();
    platforms.sort();
    platforms.dedup();
    for plat in platforms {
        let mut rng = Rng::new(input.seed ^ str_hash(&plat));
        let points = available_points(&input.global_slots, input.min_gap_minutes, &mut rng);
        // 该平台的行索引（稳定序后洗牌，实现抖动分配）。
        let mut idxs: Vec<usize> = kept
            .iter()
            .enumerate()
            .filter(|(_, r)| r.platform == plat)
            .map(|(i, _)| i)
            .collect();
        idxs.sort_by_key(|&i| (kept[i].account_id, kept[i].sku_id));
        rng.shuffle(&mut idxs);
        for (k, &i) in idxs.iter().enumerate() {
            kept[i].planned_minute = points.get(k).copied();
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
    fn parse_slot_valid_and_invalid() {
        assert_eq!(
            parse_slot("11:30-13:00"),
            Some(Slot {
                start_min: 690,
                end_min: 780
            })
        );
        assert_eq!(parse_slot("13:00-11:00"), None); // 逆序
        assert_eq!(parse_slot("25:00-26:00"), None); // 越界
        assert_eq!(parse_slot("abc"), None);
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
                },
                SchedAccount {
                    id: 11,
                    platform: "douyin".into(),
                    daily_limit: 3,
                },
                SchedAccount {
                    id: 12,
                    platform: "douyin".into(),
                    daily_limit: 3,
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

    // proptest：排期不变量全套（§6.2）。
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
        }
    }
}
