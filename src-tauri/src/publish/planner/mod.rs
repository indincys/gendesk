//! 任务单编排（发布模块执行计划 §5.1 planner/）。
//!
//! set_picker：套装选取（查重过滤 → 最少使用 → 同分随机）纯函数。
//! scheduler：当日应发清单 → 展开 → 约束过滤 → 时段分配纯函数（proptest 不变量）。
//! mod：generate_sheet 事务编排（DB 写在单事务内）。

pub mod frequency;
pub mod scheduler;
pub mod set_picker;

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use specta::Type;
use sqlx::SqlitePool;

use crate::commands::publish_settings::PublishSettings;
use crate::db::repo::{accounts, assets, ledger, planning, skus, texts};
use crate::error::{AppError, AppResult};
use crate::publish::planner::frequency::{FreqRules, SkuFreq};
use crate::publish::planner::scheduler::{DueSet, SchedAccount, ScheduleInput};
use crate::publish::planner::set_picker::{PackCand, PickInput, TextCand};
use crate::publish::platform::Platform;

/// 缺料/提示清单一项（生成副产物，存入 task_sheets.shortage_json）。
///
/// `reason` 为机器码，中文由前端 `shortageLabel()` 单点映射：
/// `no_pack` 无可用素材包 · `no_title` 无可用标题 · `no_body` 无可用正文 ·
/// `timeout_backfill` 昨日超时失败，今日已补排（**不是缺料，是提示**）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ShortageItem {
    pub sku_id: i64,
    pub code: String,
    pub reason: String,
    /// 相关平台（部分原因有意义，如无账号/查重冲突）；无则空。
    #[serde(default)]
    pub platforms: Vec<String>,
}

/// 前一日「网络超时」失败的 SKU → 今日无条件纳入应发（需求 §6.3 timeout 处置：
/// 自动重排次日）。返回 (sku_id 集合, 用于提示的 (sku_id, platforms) 明细)。
async fn timeout_backfill(pool: &SqlitePool, date: &str) -> AppResult<HashMap<i64, Vec<String>>> {
    let Ok(d) = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d") else {
        return Ok(HashMap::new());
    };
    let prev = (d - chrono::Duration::days(1))
        .format("%Y-%m-%d")
        .to_string();
    let mut out: HashMap<i64, Vec<String>> = HashMap::new();
    for (sku_id, platform, _account_id) in planning::timeout_fails_of_date(pool, &prev).await? {
        let entry = out.entry(sku_id).or_default();
        if !entry.contains(&platform) {
            entry.push(platform);
        }
    }
    Ok(out)
}

/// `YYYY-MM-DD` → `YYMMDD`（任务 ID 用）。
fn date_yymmdd(date: &str) -> String {
    date.chars()
        .filter(|c| c.is_ascii_digit())
        .skip(2)
        .collect()
}

/// 分钟自午夜 → `HH:MM`。
fn minute_to_hhmm(m: i64) -> String {
    format!("{:02}:{:02}", m / 60, m % 60)
}

/// 日期派生 seed（同日重生成可复现）。
fn date_seed(date: &str) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in date.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01B3);
    }
    h
}

/// SKU 生效平台（覆盖优先，否则全局矩阵）。
fn enabled_platforms(sku_platforms: Option<&str>, s: &PublishSettings) -> Vec<String> {
    if let Some(json) = sku_platforms {
        if let Ok(v) = serde_json::from_str::<Vec<String>>(json) {
            return v;
        }
    }
    let m = &s.platform_matrix;
    Platform::ALL
        .into_iter()
        .filter(|p| match p {
            Platform::Douyin => m.douyin,
            Platform::Xhs => m.xhs,
            Platform::Kuaishou => m.kuaishou,
            Platform::Shipinhao => m.shipinhao,
            Platform::Bilibili => m.bilibili,
        })
        .map(|p| p.code().to_string())
        .collect()
}

fn to_text_cand(r: &texts::TextItemRow) -> TextCand {
    TextCand {
        id: r.id,
        platform: r.platform.clone(),
        use_count: r.use_count,
    }
}

/// 生成某日任务单草稿（套装选取 + 排期展开 + 缺料，单事务），返回 sheet_id。
/// 草稿存在则重算覆盖；已确认及之后报错。
pub async fn generate_sheet(pool: &SqlitePool, date: &str, s: &PublishSettings) -> AppResult<i64> {
    if let Some(existing) = planning::get_sheet_by_date(pool, date).await? {
        if existing.status != "draft" {
            return Err(AppError::InvalidInput(format!(
                "{date} 任务单已是「{}」状态，不能重新生成；请先退回草稿",
                existing.status
            )));
        }
    }

    let agg = skus::list_agg(pool).await?;
    let sched_skus: Vec<&skus::SkuAggRow> = agg
        .iter()
        .filter(|r| r.is_general == 0 && r.status == "active")
        .collect();

    let rules = FreqRules {
        hot_daily: s.tier_rules.hot_daily,
        warm_weekly: s.tier_rules.warm_weekly,
        cold_weekly_rotate: s.tier_rules.cold_weekly_rotate,
    };
    let freq_in: Vec<SkuFreq> = sched_skus
        .iter()
        .map(|r| SkuFreq {
            id: r.id,
            tier: r.tier.clone(),
        })
        .collect();
    let mut due_ids = frequency::due_skus(date, &freq_in, &rules);

    // 昨日网络超时失败的 SKU：即便按频率今日不该发，也补进应发集（需求 §6.3）。
    // 展开仍走正常约束（日限/间隔），补排不是插队。
    let backfill = timeout_backfill(pool, date).await?;
    for id in backfill.keys() {
        if sched_skus.iter().any(|r| r.id == *id) && !due_ids.contains(id) {
            due_ids.push(*id);
        }
    }

    let all_accts = accounts::list(pool).await?;
    let slots: Vec<scheduler::Slot> = s
        .time_slots
        .iter()
        .filter_map(|t| scheduler::parse_slot(t))
        .collect();
    let seed = date_seed(date);
    let now = crate::db::now_unix();

    // 套装选取。
    struct Chosen {
        sku_id: i64,
        pick: set_picker::SetPick,
        platforms: Vec<String>,
    }
    let mut chosen: Vec<Chosen> = Vec::new();
    let mut shortage: Vec<ShortageItem> = Vec::new();

    for r in &sched_skus {
        if !due_ids.contains(&r.id) {
            continue;
        }
        let target_platforms = enabled_platforms(r.platforms_json.as_deref(), s);
        if target_platforms.is_empty() {
            continue;
        }
        let packs = assets::list_by_sku(pool, r.id).await?;
        let mut pack_cands = Vec::with_capacity(packs.len());
        for p in &packs {
            let last = ledger::pack_platform_last(pool, p.id).await?;
            pack_cands.push(PackCand {
                id: p.id,
                kind: p.kind.clone(),
                lifecycle: p.lifecycle.clone(),
                last_pub: last,
            });
        }
        let mut conn = pool.acquire().await?;
        let titles = texts::list_enabled(&mut conn, r.id, "title").await?;
        let bodies = texts::list_enabled(&mut conn, r.id, "body").await?;
        drop(conn);

        let input = PickInput {
            packs: pack_cands,
            titles: titles.iter().map(to_text_cand).collect(),
            bodies: bodies.iter().map(to_text_cand).collect(),
            target_platforms: target_platforms.clone(),
            dedup_days: s.dedup_days,
            now,
            seed: seed ^ (r.id as u64),
        };
        match set_picker::pick(&input) {
            Ok(pick) => {
                if let Some(platforms) = backfill.get(&r.id) {
                    // 工作台据此显示「补排」徽标，说明这个 SKU 今天为什么出现。
                    shortage.push(ShortageItem {
                        sku_id: r.id,
                        code: r.code.clone(),
                        reason: "timeout_backfill".into(),
                        platforms: platforms.clone(),
                    });
                }
                chosen.push(Chosen {
                    sku_id: r.id,
                    pick,
                    platforms: target_platforms,
                });
            }
            Err(e) => shortage.push(ShortageItem {
                sku_id: r.id,
                code: r.code.clone(),
                reason: e.code().to_string(),
                platforms: Vec::new(),
            }),
        }
    }

    // 排期。
    let due_sets: Vec<DueSet> = chosen
        .iter()
        .map(|c| DueSet {
            sku_id: c.sku_id,
            platforms: c.platforms.clone(),
            content_kind: c.pick.content_kind.clone(),
        })
        .collect();
    let sched_accounts: Vec<SchedAccount> = all_accts
        .iter()
        .filter(|a| a.status == "active")
        .map(|a| SchedAccount {
            id: a.id,
            platform: a.platform.clone(),
            daily_limit: a.daily_limit,
        })
        .collect();
    let result = scheduler::schedule(&ScheduleInput {
        due: due_sets,
        accounts: sched_accounts,
        global_slots: slots,
        min_gap_minutes: s.min_gap_minutes,
        seed,
    });

    // 事务落库。
    let mut tx = pool.begin().await?;
    let sheet_id = match planning::get_sheet_by_date(pool, date).await? {
        Some(existing) => {
            planning::clear_sheet_children(&mut tx, existing.id, date).await?;
            planning::set_sheet_status(&mut tx, existing.id, "draft").await?;
            existing.id
        }
        None => planning::create_sheet(&mut tx, date).await?,
    };

    let mut set_ids: HashMap<i64, i64> = HashMap::new();
    for c in &chosen {
        let set_id = planning::insert_daily_set(
            &mut tx,
            &planning::NewDailySet {
                date: date.to_string(),
                sku_id: c.sku_id,
                pack_id: c.pick.pack_id,
                title_id: c.pick.title_id,
                body_id: c.pick.body_id,
            },
        )
        .await?;
        set_ids.insert(c.sku_id, set_id);
    }

    let yy = date_yymmdd(date);
    let mut seq = 0i64;
    let mut rows = 0usize;
    for row in &result.rows {
        let Some(set_id) = set_ids.get(&row.sku_id) else {
            continue;
        };
        seq += 1;
        let task_code = format!("T{yy}-{seq:03}");
        planning::insert_publish_task(
            &mut tx,
            &planning::NewPublishTask {
                sheet_id,
                task_code,
                set_id: *set_id,
                account_id: row.account_id,
                platform: row.platform.clone(),
                content_kind: row.content_kind.clone(),
                planned_time: row.planned_minute.map(minute_to_hhmm),
            },
        )
        .await?;
        rows += 1;
    }

    let shortage_json = serde_json::to_string(&shortage)?;
    planning::set_shortage(&mut tx, sheet_id, &shortage_json).await?;
    tx.commit().await?;

    let _ = rows; // 行数已由 sheet_rows 复算，无需回传
    Ok(sheet_id)
}

/// 确定性 RNG（splitmix64）：固定 seed 可复现随机（执行计划 §2.2/2.3 DoD）。
/// 不引第三方 rand，避免依赖膨胀。
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }
    /// 下一个 u64。
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// [0, n) 内均匀整数（n=0 返回 0）。
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next_u64() % n as u64) as usize
        }
    }
    /// 原地 Fisher–Yates 洗牌。
    pub fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (1..v.len()).rev() {
            let j = self.below(i + 1);
            v.swap(i, j);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即测试失败，是期望行为
mod gen_tests {
    use super::*;
    use crate::db::test_support::test_pool;

    async fn seed(pool: &SqlitePool) {
        // 一个热款 SKU（每日应发）+ 视频包（active）+ 标题 + xhs 账号。
        let sku = skus::insert(
            pool,
            &skus::NewSku {
                code: "SF-1".into(),
                style_name: "款".into(),
                product_name: String::new(),
                tier: "hot".into(),
                topics_json: "[]".into(),
                platforms_json: Some("[\"xhs\"]".into()),
                note: String::new(),
            },
        )
        .await
        .unwrap();
        let pack = assets::insert(
            pool,
            &assets::NewPack {
                sku_id: sku,
                kind: "video".into(),
                dir_rel: "资产库/SF-1/v1".into(),
                files_json: "[]".into(),
                cover: None,
                source: "manual".into(),
            },
        )
        .await
        .unwrap();
        assets::set_lifecycle(pool, pack, "active").await.unwrap();
        texts::insert(
            pool,
            &texts::NewTextItem {
                sku_id: sku,
                kind: "title".into(),
                text: "标题一".into(),
                platform: "general".into(),
                source: "manual".into(),
            },
        )
        .await
        .unwrap();
        accounts::insert(
            pool,
            &accounts::NewAccount {
                platform: "xhs".into(),
                name: "小红书主号".into(),
                daily_limit: 3,
                slots_json: None,
            },
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn generate_idempotent_then_confirm_rejects_regen() {
        let (pool, _d) = test_pool().await;
        seed(&pool).await;
        let s = PublishSettings::default();

        let sheet_id = generate_sheet(&pool, "2026-07-15", &s).await.unwrap();
        let rows1 = planning::list_tasks_by_sheet(&pool, sheet_id)
            .await
            .unwrap();
        assert_eq!(rows1.len(), 1, "1 SKU × xhs × 1 账号 = 1 行");
        assert_eq!(rows1[0].task_code, "T260715-001");

        // 重生成覆盖：仍是同一单，1 行（幂等，不叠加）。
        let sheet_id2 = generate_sheet(&pool, "2026-07-15", &s).await.unwrap();
        assert_eq!(sheet_id2, sheet_id);
        let rows2 = planning::list_tasks_by_sheet(&pool, sheet_id)
            .await
            .unwrap();
        assert_eq!(rows2.len(), 1, "重生成不叠加");

        // 确认后拒绝重生成。
        let mut conn = pool.acquire().await.unwrap();
        planning::set_sheet_status(&mut conn, sheet_id, "confirmed")
            .await
            .unwrap();
        drop(conn);
        let err = generate_sheet(&pool, "2026-07-15", &s).await;
        assert!(err.is_err(), "已确认单不能重生成");
    }

    #[tokio::test]
    async fn shortage_when_no_title() {
        let (pool, _d) = test_pool().await;
        // SKU + 包但无标题 → 缺料，不排任务。
        let sku = skus::insert(
            &pool,
            &skus::NewSku {
                code: "SF-2".into(),
                style_name: "款".into(),
                product_name: String::new(),
                tier: "hot".into(),
                topics_json: "[]".into(),
                platforms_json: Some("[\"xhs\"]".into()),
                note: String::new(),
            },
        )
        .await
        .unwrap();
        let pack = assets::insert(
            &pool,
            &assets::NewPack {
                sku_id: sku,
                kind: "video".into(),
                dir_rel: "资产库/SF-2/v1".into(),
                files_json: "[]".into(),
                cover: None,
                source: "manual".into(),
            },
        )
        .await
        .unwrap();
        assets::set_lifecycle(&pool, pack, "active").await.unwrap();
        accounts::insert(
            &pool,
            &accounts::NewAccount {
                platform: "xhs".into(),
                name: "号".into(),
                daily_limit: 3,
                slots_json: None,
            },
        )
        .await
        .unwrap();

        let s = PublishSettings::default();
        let sheet_id = generate_sheet(&pool, "2026-07-15", &s).await.unwrap();
        let rows = planning::list_tasks_by_sheet(&pool, sheet_id)
            .await
            .unwrap();
        assert!(rows.is_empty(), "缺标题 → 无任务行");
        let sheet = planning::get_sheet(&pool, sheet_id).await.unwrap().unwrap();
        let shortage: Vec<ShortageItem> = serde_json::from_str(&sheet.shortage_json).unwrap();
        assert_eq!(shortage.len(), 1);
        assert_eq!(shortage[0].code, "SF-2");
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即测试失败，是期望行为
mod tests {
    use super::*;

    #[test]
    fn rng_is_deterministic_for_seed() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        for _ in 0..100 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn shuffle_is_permutation() {
        let mut r = Rng::new(7);
        let mut v: Vec<i32> = (0..20).collect();
        let orig = v.clone();
        r.shuffle(&mut v);
        let mut sorted = v.clone();
        sorted.sort();
        assert_eq!(sorted, orig, "洗牌后仍是同一多重集");
    }

    #[test]
    fn below_in_range() {
        let mut r = Rng::new(1);
        for _ in 0..100 {
            assert!(r.below(5) < 5);
        }
        assert_eq!(r.below(0), 0);
    }
}
