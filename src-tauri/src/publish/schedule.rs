//! 跨商品全局排期纯函数。

use std::collections::{HashMap, HashSet};

use chrono::{Duration, NaiveDate, NaiveDateTime, NaiveTime};

#[derive(Debug, Clone)]
pub struct SchedulePost {
    pub post_id: i64,
    pub seq: usize,
    pub date: NaiveDate,
    pub anchors: Vec<String>,
    pub jitter_min: i64,
    pub min_gap_min: i64,
    pub platforms: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledTask {
    pub post_id: i64,
    pub platform: String,
    pub scheduled_at: NaiveDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixedSlot {
    pub platform: String,
    pub scheduled_at: NaiveDateTime,
    pub min_gap_min: i64,
}

#[derive(Debug, Clone)]
struct Variable {
    post_id: i64,
    platform: String,
    min_gap_min: i64,
    anchor_text: String,
    choices: Vec<NaiveDateTime>,
}

pub fn parse_hhmm(value: &str) -> Option<NaiveTime> {
    NaiveTime::parse_from_str(value.trim(), "%H:%M").ok()
}

fn candidates(
    anchor: NaiveDateTime,
    start: NaiveDateTime,
    end: NaiveDateTime,
) -> Vec<NaiveDateTime> {
    let mut out = Vec::new();
    let max = (end - start).num_minutes().max(0);
    for delta in 0..=max {
        for signed in if delta == 0 {
            vec![0]
        } else {
            vec![delta, -delta]
        } {
            let at = anchor + Duration::minutes(signed);
            if at >= start && at <= end && !out.contains(&at) {
                out.push(at);
            }
        }
    }
    out
}

#[cfg(test)]
pub fn schedule_all(
    posts: &[SchedulePost],
    now: NaiveDateTime,
) -> Result<Vec<ScheduledTask>, String> {
    schedule_all_with_fixed(posts, now, &[])
}

pub fn schedule_all_with_fixed(
    posts: &[SchedulePost],
    now: NaiveDateTime,
    fixed: &[FixedSlot],
) -> Result<Vec<ScheduledTask>, String> {
    let lower = now + Duration::hours(2);
    let upper = now + Duration::days(14);
    let mut by_platform: HashMap<String, Vec<(NaiveDateTime, i64)>> = HashMap::new();
    for slot in fixed {
        by_platform
            .entry(slot.platform.clone())
            .or_default()
            .push((slot.scheduled_at, slot.min_gap_min));
    }
    let mut variables = Vec::new();
    for post in posts {
        if post.anchors.is_empty() {
            return Err(format!("篇 {} 没有配置发布时间锚点", post.post_id));
        }
        let anchor_text = &post.anchors[post.seq % post.anchors.len()];
        let time = parse_hhmm(anchor_text).ok_or_else(|| format!("非法锚点：{anchor_text}"))?;
        let anchor = post.date.and_time(time);
        let start = (anchor - Duration::minutes(post.jitter_min)).max(lower);
        let end = (anchor + Duration::minutes(post.jitter_min)).min(upper);
        if start > end {
            return Err(format!("锚点 {anchor_text} 不在导出时刻 +2h 到 +14d 内"));
        }
        let choices = candidates(anchor, start, end);
        for platform in &post.platforms {
            variables.push(Variable {
                post_id: post.post_id,
                platform: platform.clone(),
                min_gap_min: post.min_gap_min,
                anchor_text: anchor_text.clone(),
                choices: choices.clone(),
            });
        }
    }

    let mut assigned = vec![None; variables.len()];
    let mut by_post: HashMap<i64, HashSet<NaiveDateTime>> = HashMap::new();
    if !solve(&variables, &mut assigned, &mut by_platform, &mut by_post) {
        let constrained = variables
            .iter()
            .min_by_key(|variable| variable.choices.len())
            .ok_or_else(|| "排期输入为空".to_string())?;
        return Err(format!(
            "排期拥挤：{} 附近无法为 {} 安排满足 {} 分钟间隔的全局解",
            constrained.anchor_text, constrained.platform, constrained.min_gap_min
        ));
    }
    Ok(variables
        .into_iter()
        .zip(assigned)
        .filter_map(|(variable, scheduled_at)| {
            scheduled_at.map(|scheduled_at| ScheduledTask {
                post_id: variable.post_id,
                platform: variable.platform,
                scheduled_at,
            })
        })
        .collect())
}

fn choice_is_valid(
    variable: &Variable,
    candidate: NaiveDateTime,
    by_platform: &HashMap<String, Vec<(NaiveDateTime, i64)>>,
    by_post: &HashMap<i64, HashSet<NaiveDateTime>>,
) -> bool {
    if by_post
        .get(&variable.post_id)
        .is_some_and(|times| times.contains(&candidate))
    {
        return false;
    }
    by_platform
        .get(&variable.platform)
        .into_iter()
        .flatten()
        .all(|(other, other_gap)| {
            let gap = (candidate - *other).num_minutes().unsigned_abs() as i64;
            gap >= variable.min_gap_min.max(*other_gap)
        })
}

fn solve(
    variables: &[Variable],
    assigned: &mut [Option<NaiveDateTime>],
    by_platform: &mut HashMap<String, Vec<(NaiveDateTime, i64)>>,
    by_post: &mut HashMap<i64, HashSet<NaiveDateTime>>,
) -> bool {
    if assigned.iter().all(Option::is_some) {
        return true;
    }
    // MRV：每轮先解当前合法候选最少的变量。仍无解才真正报告拥挤，避免
    // first-fit 把宽窗口抢占后误判窄窗口无解。
    let Some((variable_index, choices)) = variables
        .iter()
        .enumerate()
        .filter(|(index, _)| assigned[*index].is_none())
        .map(|(index, variable)| {
            let choices = variable
                .choices
                .iter()
                .copied()
                .filter(|candidate| choice_is_valid(variable, *candidate, by_platform, by_post))
                .collect::<Vec<_>>();
            (index, choices)
        })
        .min_by_key(|(index, choices)| (choices.len(), *index))
    else {
        return true;
    };
    if choices.is_empty() {
        return false;
    }
    let variable = &variables[variable_index];
    for candidate in choices {
        assigned[variable_index] = Some(candidate);
        by_platform
            .entry(variable.platform.clone())
            .or_default()
            .push((candidate, variable.min_gap_min));
        by_post
            .entry(variable.post_id)
            .or_default()
            .insert(candidate);
        if solve(variables, assigned, by_platform, by_post) {
            return true;
        }
        assigned[variable_index] = None;
        if let Some(prior) = by_platform.get_mut(&variable.platform) {
            prior.pop();
        }
        if let Some(times) = by_post.get_mut(&variable.post_id) {
            times.remove(&candidate);
        }
    }
    false
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // 测试断言失败即测试失败
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn dt(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap()
    }

    proptest! {
        #[test]
        fn five_schedule_invariants(post_count in 1usize..6, jitter in 3i64..20, gap in 1i64..4) {
            let now = dt("2026-08-01 00:00");
            let date = NaiveDate::from_ymd_opt(2026, 8, 2).unwrap();
            let posts: Vec<_> = (0..post_count).map(|i| SchedulePost {
                post_id: i as i64 + 1,
                seq: i,
                date,
                anchors: vec!["09:00".into(), "12:00".into(), "15:00".into(), "18:00".into(), "21:00".into()],
                jitter_min: jitter,
                min_gap_min: gap,
                platforms: vec!["douyin".into(), "xhs".into(), "shipinhao".into(), "kuaishou".into()],
            }).collect();
            let out = schedule_all(&posts, now).unwrap();
            prop_assert_eq!(out.len(), post_count * 4);
            for task in &out {
                let post = posts.iter().find(|p| p.post_id == task.post_id).unwrap();
                let anchor = post.date.and_time(parse_hhmm(&post.anchors[post.seq % post.anchors.len()]).unwrap());
                prop_assert!((task.scheduled_at - anchor).num_minutes().unsigned_abs() <= jitter as u64);
                prop_assert!(task.scheduled_at >= now + Duration::hours(2));
                prop_assert!(task.scheduled_at <= now + Duration::days(14));
            }
            for post in &posts {
                let times: HashSet<_> = out.iter().filter(|t| t.post_id == post.post_id).map(|t| t.scheduled_at).collect();
                prop_assert_eq!(times.len(), 4);
            }
            for platform in ["douyin", "xhs", "shipinhao", "kuaishou"] {
                let tasks: Vec<_> = out.iter().filter(|t| t.platform == platform).collect();
                for a in 0..tasks.len() {
                    for b in (a + 1)..tasks.len() {
                        prop_assert!((tasks[a].scheduled_at - tasks[b].scheduled_at).num_minutes().unsigned_abs() >= gap as u64);
                    }
                }
            }
        }
    }

    #[test]
    fn reports_crowding_instead_of_compressing() {
        let date = NaiveDate::from_ymd_opt(2026, 8, 2).unwrap();
        let posts = vec![1, 2]
            .into_iter()
            .map(|id| SchedulePost {
                post_id: id,
                seq: 0,
                date,
                anchors: vec!["09:00".into()],
                jitter_min: 0,
                min_gap_min: 3,
                platforms: vec!["douyin".into()],
            })
            .collect::<Vec<_>>();
        let err = schedule_all(&posts, dt("2026-08-01 00:00")).unwrap_err();
        assert!(err.contains("排期拥挤"));
        assert!(err.contains("09:00"));
    }

    #[test]
    fn backtracks_when_a_wide_window_would_block_a_fixed_anchor() {
        let date = NaiveDate::from_ymd_opt(2026, 8, 2).unwrap();
        let posts = vec![
            SchedulePost {
                post_id: 1,
                seq: 0,
                date,
                anchors: vec!["09:00".into()],
                jitter_min: 3,
                min_gap_min: 3,
                platforms: vec!["douyin".into()],
            },
            SchedulePost {
                post_id: 2,
                seq: 0,
                date,
                anchors: vec!["09:00".into()],
                jitter_min: 0,
                min_gap_min: 3,
                platforms: vec!["douyin".into()],
            },
        ];
        let out = schedule_all(&posts, dt("2026-08-01 00:00")).unwrap();
        assert_eq!(out[1].scheduled_at, dt("2026-08-02 09:00"));
        assert_eq!(out[0].scheduled_at, dt("2026-08-02 09:03"));
    }

    #[test]
    fn fixed_slot_on_adjacent_date_blocks_cross_midnight_collision() {
        let posts = vec![SchedulePost {
            post_id: 1,
            seq: 0,
            date: NaiveDate::from_ymd_opt(2026, 8, 2).unwrap(),
            anchors: vec!["00:00".into()],
            jitter_min: 3,
            min_gap_min: 3,
            platforms: vec!["xhs".into()],
        }];
        let fixed = vec![FixedSlot {
            platform: "xhs".into(),
            scheduled_at: dt("2026-08-01 23:59"),
            min_gap_min: 3,
        }];
        let out = schedule_all_with_fixed(&posts, dt("2026-08-01 20:00"), &fixed).unwrap();
        assert_eq!(out[0].scheduled_at, dt("2026-08-02 00:02"));
    }

    #[test]
    fn fixed_tasks_from_confirmed_sheets_reserve_their_platform_minutes() {
        let date = NaiveDate::from_ymd_opt(2026, 8, 2).unwrap();
        let posts = vec![SchedulePost {
            post_id: 2,
            seq: 0,
            date,
            anchors: vec!["09:00".into()],
            jitter_min: 3,
            min_gap_min: 3,
            platforms: vec!["douyin".into()],
        }];
        let fixed = vec![FixedSlot {
            platform: "douyin".into(),
            scheduled_at: dt("2026-08-02 09:00"),
            min_gap_min: 3,
        }];
        let out = schedule_all_with_fixed(&posts, dt("2026-08-01 00:00"), &fixed).unwrap();
        assert_eq!(out[0].scheduled_at, dt("2026-08-02 09:03"));
    }
}
