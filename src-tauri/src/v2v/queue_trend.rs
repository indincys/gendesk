//! 排队位次 → 排队速度。**纯函数**，不碰库也不碰 CLI。
//!
//! ## 这里算的是导数，不是计数
//!
//! 「第 4485 位」单独看没有信息量：它可能十分钟后变成 4100（这条队一小时消化两千多位，
//! 今晚就能出片），也可能十分钟后还是 4480（明天早上都轮不到）。要排生产队列，
//! 人真正要的那个数是 **位/小时**，而它只能从两个时间点上的位次相减得到。
//!
//! ## 为什么可以跨 clip 汇总
//!
//! `queue_idx` 是**全局队列**里的位次（同一份回体里还有 `queue_length: 574522`），
//! 所以每一条在跑的条目都在测同一条队。把不同 clip 的斜率放进同一个小时桶里取中位数，
//! 得到的就是那个小时里非 VIP 通道的真实速度，而不是某一单的运气。
//!
//! ## 三条防止「算出一个漂亮但错的数」的规则
//!
//! 1. **位次回升的那一段直接丢掉**。实测里它出现在重排/重试之后 —— 那不是队列变慢，
//!    而是这一单换了个位置。把它当成负速度会污染整个小时桶。
//! 2. **一条 clip 在一个桶里只投一票**（先按 clip 算完段内速度，再跨 clip 取中位数）。
//!    否则采样密的那条 clip 会自己决定这个桶的值。
//! 3. **取中位数不取平均**。刚提交那几条会经历一次「排进去」的跳变，均值扛不住它。

/// 一个采样点（与 `db::repo::v2v::QueueSample` 同形，但这一层不依赖 sqlx）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sample {
    pub clip_id: i64,
    pub at: i64,
    pub queue_idx: i64,
}

/// 一个小时桶里的排队速度。
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct HourRate {
    /// 桶的起点（unix 秒，整点对齐）。
    pub hour_start: i64,
    /// 这个小时里队列消化了多少位（位/小时，正数 = 在前进）。
    pub positions_per_hour: f64,
    /// 参与投票的 clip 条数。1 条时这个数字信心很低，界面据此决定要不要画实线。
    pub clips: i64,
}

/// 一小时的秒数。桶按它对齐 —— 「今晚几点提交」问的就是小时粒度。
const HOUR: i64 = 3600;

/// 逐小时的排队速度。输入不要求有序。
///
/// 空输入、单点、全是回升段这三种退化情况都回空 vec —— **绝不回 0**：
/// 0 是「队列停住了」这个结论，而这几种情况下我们根本没有结论，
/// 而界面上一个凭空的「0 位/小时」会直接让人把今晚的排产取消掉。
pub fn hourly_rates(samples: &[Sample]) -> Vec<HourRate> {
    // (小时桶, clip) → 该 clip 在该桶内的 (位次消化量, 秒数)
    let mut per_clip: std::collections::HashMap<(i64, i64), (i64, i64)> =
        std::collections::HashMap::new();

    let mut sorted: Vec<Sample> = samples.to_vec();
    sorted.sort_by_key(|s| (s.clip_id, s.at));

    for pair in sorted.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if a.clip_id != b.clip_id {
            continue;
        }
        let secs = b.at - a.at;
        if secs <= 0 {
            continue;
        }
        // 规则 1：位次回升 = 这一单换了位置，不是队列在倒退。
        let drained = a.queue_idx - b.queue_idx;
        if drained < 0 {
            continue;
        }
        // 段跨桶时整段归到起点那个桶：采样间隔（非 VIP 600 秒）远小于桶宽，
        // 按比例切分只会带来一堆几乎为零的碎片，而分辨率并不会因此变高。
        let bucket = a.at - a.at.rem_euclid(HOUR);
        let e = per_clip.entry((bucket, a.clip_id)).or_insert((0, 0));
        e.0 += drained;
        e.1 += secs;
    }

    // 规则 2：一条 clip 在一个桶里只投一票。
    let mut per_bucket: std::collections::HashMap<i64, Vec<f64>> = std::collections::HashMap::new();
    for ((bucket, _clip), (drained, secs)) in per_clip {
        if secs <= 0 {
            continue;
        }
        per_bucket
            .entry(bucket)
            .or_default()
            .push(drained as f64 * HOUR as f64 / secs as f64);
    }

    let mut out: Vec<HourRate> = per_bucket
        .into_iter()
        .map(|(hour_start, mut votes)| {
            let clips = votes.len() as i64;
            HourRate {
                hour_start,
                positions_per_hour: median(&mut votes),
                clips,
            }
        })
        .collect();
    out.sort_by_key(|r| r.hour_start);
    out
}

/// 最近 `window_secs` 内的排队速度（详情栏那句「近 1 小时 −312 位/时」）。
///
/// 与 [`hourly_rates`] 分开：那个是按整点归桶的趋势图，这个是「此刻多快」，
/// 窗口跟着 now 滑动，两者在小时边界上必然给出不同的数字 —— 这不是分叉，是两个问题。
pub fn recent_rate(samples: &[Sample], now: i64, window_secs: i64) -> Option<f64> {
    let cut = now - window_secs;
    let recent: Vec<Sample> = samples.iter().copied().filter(|s| s.at >= cut).collect();
    let mut votes: Vec<f64> = Vec::new();
    let mut sorted = recent;
    sorted.sort_by_key(|s| (s.clip_id, s.at));
    let mut i = 0;
    while i < sorted.len() {
        let clip = sorted[i].clip_id;
        let mut j = i;
        while j + 1 < sorted.len() && sorted[j + 1].clip_id == clip {
            j += 1;
        }
        let (first, last) = (sorted[i], sorted[j]);
        let secs = last.at - first.at;
        let drained = first.queue_idx - last.queue_idx;
        if secs > 0 && drained >= 0 {
            votes.push(drained as f64 * HOUR as f64 / secs as f64);
        }
        i = j + 1;
    }
    (!votes.is_empty()).then(|| median(&mut votes))
}

/// 按当前速度，这一单还要等多久（秒）。
///
/// 速度 ≤ 0 时回 `None` 而不是无穷大：那意味着「这段时间里队列没动」，
/// 而从中推不出任何一个可以拿来排产的时长。宁可界面上少一行。
pub fn eta_secs(queue_idx: i64, rate_per_hour: f64) -> Option<i64> {
    if rate_per_hour <= 0.0 || queue_idx <= 0 {
        return None;
    }
    Some((queue_idx as f64 / rate_per_hour * HOUR as f64).round() as i64)
}

/// 规则 3：中位数。刚提交那几条会经历一次「排进去」的跳变，均值扛不住它。
fn median(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = v.len();
    if n == 0 {
        return 0.0;
    }
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即失败
mod tests {
    use super::*;

    fn s(clip_id: i64, at: i64, queue_idx: i64) -> Sample {
        Sample {
            clip_id,
            at,
            queue_idx,
        }
    }

    // 基本盘：一条 clip 一小时里从 5000 排到 4000 = 1000 位/小时。
    #[test]
    fn one_clip_draining_steadily_gives_its_slope() {
        let r = hourly_rates(&[s(1, 0, 5000), s(1, 1800, 4500), s(1, 3000, 4000)]);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].hour_start, 0);
        // 3000 秒里消化 1000 位 → 1200 位/小时。
        assert!((r[0].positions_per_hour - 1200.0).abs() < 0.5, "{r:?}");
    }

    // **这是这个模块存在的理由**：一个标量答不出「快还是慢」，两个时间点才行。
    // 同样是「第 4485 位」，下面两条队伍对今晚要不要排产给出相反的答案。
    #[test]
    fn the_same_position_means_opposite_things_at_different_speeds() {
        let fast = hourly_rates(&[s(1, 0, 4485), s(1, 3000, 2485)]);
        let slow = hourly_rates(&[s(2, 0, 4485), s(2, 3000, 4470)]);
        let fast_eta = eta_secs(2485, fast[0].positions_per_hour).unwrap();
        let slow_eta = eta_secs(4470, slow[0].positions_per_hour).unwrap();
        assert!(fast_eta < 2 * 3600, "快队应当今晚就能轮到：{fast_eta}s");
        assert!(slow_eta > 24 * 3600, "慢队要过夜：{slow_eta}s");
    }

    // 规则 1：位次回升是「这一单换了位置」（重排/重试），不是队列在倒退。
    // 把它当负速度会把整个小时桶拖成负数，而界面据此会说「队列在变长」。
    #[test]
    fn a_position_going_back_up_is_dropped_not_counted_as_negative_speed() {
        let r = hourly_rates(&[
            s(1, 0, 5000),
            s(1, 600, 4400), // 正常消化 600 位
            s(1, 1200, 9000),
            // 重排：位次跳回去了
            s(1, 1800, 8400), // 又消化 600 位
        ]);
        assert_eq!(r.len(), 1);
        assert!(
            r[0].positions_per_hour > 0.0,
            "回升段被丢掉后剩下的都是前进：{r:?}"
        );
    }

    // 规则 2：采样密的那条 clip 不能自己决定这个桶的值。
    // 三条 clip 就该是三票，哪怕其中一条的点数是另外两条的十倍。
    #[test]
    fn each_clip_votes_once_per_bucket_however_densely_it_was_sampled() {
        let mut samples = vec![s(1, 0, 1000), s(1, 3600 - 1, 100)];
        // clip 2 采样极密，且明显更慢。
        for k in 0..20 {
            samples.push(s(2, k * 100, 5000 - k));
        }
        samples.push(s(3, 0, 3000));
        samples.push(s(3, 3000, 2000));
        let r = hourly_rates(&samples);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].clips, 3, "三条 clip = 三票，与各自采了多少点无关");
    }

    // 退化输入一律回空，**绝不回 0**：0 是「队列停住了」这个结论，
    // 而这几种情况下我们根本没有结论 —— 而一个凭空的 0 会让人当场取消今晚的排产。
    #[test]
    fn degenerate_input_yields_no_reading_rather_than_a_fabricated_zero() {
        assert!(hourly_rates(&[]).is_empty());
        assert!(hourly_rates(&[s(1, 0, 4485)]).is_empty(), "单点算不出斜率");
        assert!(
            hourly_rates(&[s(1, 0, 100), s(2, 60, 4485)]).is_empty(),
            "两个点属于不同 clip，连不成一段"
        );
        assert!(
            hourly_rates(&[s(1, 0, 100), s(1, 0, 90)]).is_empty(),
            "同一秒的两个点，时间差为 0"
        );
        assert!(recent_rate(&[], 1000, 3600).is_none());
    }

    // ETA 在「队列没动」时必须没有答案，而不是一个无穷大或者 0。
    #[test]
    fn eta_has_no_answer_when_the_queue_is_not_moving() {
        assert!(eta_secs(4485, 0.0).is_none());
        assert!(eta_secs(4485, -10.0).is_none());
        assert!(eta_secs(0, 1200.0).is_none(), "已经排到头了，没有 ETA 可言");
        assert_eq!(eta_secs(1200, 1200.0), Some(3600));
    }

    // 滑动窗口只看最近这一段：几小时前那段慢速不该把「此刻很快」拖下来。
    #[test]
    fn recent_rate_ignores_what_happened_before_the_window() {
        let now = 100_000;
        let samples = vec![
            // 远古：极慢
            s(1, now - 20_000, 9000),
            s(1, now - 19_000, 8990),
            // 窗口内：很快
            s(1, now - 1_800, 5000),
            s(1, now, 3000),
        ];
        let r = recent_rate(&samples, now, 3600).unwrap();
        assert!(
            (r - 4000.0).abs() < 1.0,
            "1800 秒消化 2000 位 → 4000 位/时：{r}"
        );
    }
}
