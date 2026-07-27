//! 常驻的非 VIP 队列：完成一条就补一条，让廉价通道永远有活在排。
//!
//! ## 它在赌什么，以及为什么这个赌是划算的
//!
//! 非 VIP 通道实测排在第 4485 位、队列长度 574522 —— 一条要等几小时。VIP 同规格贵
//! 5.5 倍，买到的只是不排队。于是「等」这件事本身是免费的，只要**队列不空着**，
//! 过夜八小时就能白拿几条片子。人做不到这件事（要守着补单），机器可以。
//!
//! ## 但它是在自动花钱，所以闸门必须是机制而不是自觉
//!
//! 四道闸，缺一条都会变成漏水的钱包：
//!
//! 1. **默认关**。开了才跑。
//! 2. **模型必须非 VIP**。这条队列的全部前提就是「便宜」，配一个 `_vip` 模型等于
//!    每晚自动烧 5.5 倍的钱 —— 拒在设置保存那一刻，不是跑起来才发现。
//! 3. **日额度上限**。按**提交**时刻切窗而不是出片时刻：出片要等几小时，用出片切窗
//!    的话补单器能在任何一条出片之前把一整天的额度提交光，而那个上限从头到尾不会触发。
//! 4. **余额兜底**。余额不够就停，且不是「提交到一半开始报错」——即梦逐条扣费，
//!    前面扣掉的退不回来。
//!
//! ## 告急通知
//!
//! 队列断流的**真正原因**从来不是补单器停了，而是**没料了**：待提交的存量见底，
//! 而补起来要人去写提示词（或让 skill 写）。所以通知要在存量见底**之前**发，
//! 且带冷却 —— 每 30 秒弹一次通知，人会在第三次就把通知关掉，然后这条链路就永久失灵了。

use serde::{Deserialize, Serialize};
use specta::Type;
use sqlx::SqlitePool;

use crate::db::now_unix;
use crate::db::repo::v2v as repo;
use crate::v2v::activity::Activity;
use crate::v2v::dreamina::{self, GenOpts};

/// 告急通知的冷却：一天最多吵两次。
pub const NOTICE_COOLDOWN_SECS: i64 = 12 * 3600;

/// 常驻队列配置（`V2vSettings.autofill` 的一部分）。
#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AutofillCfg {
    /// 默认关。自动花钱的东西不该装完就在跑。
    #[serde(default)]
    pub enabled: bool,
    /// 常驻在跑的条数（补单器自己放行的那些）。
    #[serde(default = "d_depth")]
    pub depth: i64,
    /// 这条队列用的模型。**必须非 VIP**（保存时校验）。
    #[serde(default = "d_model")]
    pub model_version: String,
    #[serde(default)]
    pub duration: Option<i64>,
    #[serde(default)]
    pub video_resolution: String,
    /// 待提交存量低于它就发告急通知。
    #[serde(default = "d_low")]
    pub low_water: i64,
    /// 每日额度上限（按提交时刻切窗）。`0` 或负数 = 不限。
    #[serde(default = "d_daily")]
    pub daily_credits: i64,
}

fn d_depth() -> i64 {
    3
}
fn d_model() -> String {
    dreamina::DEFAULT_MODEL.to_string()
}
fn d_low() -> i64 {
    5
}
/// 默认日限 200 额度 ≈ 每天 25 条 4s/720p（`seedance2.0fast` 单价 8）。
///
/// 给一个具体的数而不是「不限」：默认值是绝大多数人唯一会用到的那个值，
/// 而一个默认不限的自动扣费器，其上限实际上等于账户余额。
fn d_daily() -> i64 {
    200
}

impl Default for AutofillCfg {
    fn default() -> Self {
        Self {
            enabled: false,
            depth: d_depth(),
            model_version: d_model(),
            duration: None,
            video_resolution: String::new(),
            low_water: d_low(),
            daily_credits: d_daily(),
        }
    }
}

impl AutofillCfg {
    /// 这条队列实际发往即梦的参数。
    pub fn opts(&self) -> GenOpts {
        let blank = |s: &String| (!s.trim().is_empty()).then(|| s.trim().to_string());
        GenOpts {
            model_version: blank(&self.model_version),
            duration: self.duration,
            video_resolution: blank(&self.video_resolution),
            session: None,
        }
    }

    /// 校验：模型必须给、且必须非 VIP、且组合合法。
    ///
    /// 在**保存设置**那一刻拒，不是跑起来才发现 —— 后者意味着第一晚就已经按 5.5 倍
    /// 价钱跑掉了一批，而那笔钱退不回来。
    pub fn validate(&self) -> crate::error::AppResult<GenOpts> {
        use crate::error::AppError;
        if !self.enabled {
            return Ok(GenOpts {
                model_version: None,
                duration: None,
                video_resolution: None,
                session: None,
            });
        }
        let opts = self.opts();
        let Some(model) = opts.model_version.clone() else {
            return Err(AppError::InvalidInput(
                "常驻队列必须指定模型：这条队列全靠「便宜」成立，把选择交给 CLI 默认\
                 等于把每晚的账单交给一个会随版本变的值"
                    .into(),
            ));
        };
        if crate::v2v::dreamina::is_vip(&model) {
            return Err(AppError::InvalidInput(format!(
                "常驻队列不接受 VIP 通道（{model}）：同规格实测贵 5.5 倍，买到的只是不排队，\
                 而这条队列的前提恰恰是「排队不要钱」。要跑 VIP 请手动提交。"
            )));
        }
        if self.depth < 1 || self.depth > 20 {
            return Err(AppError::InvalidInput(format!(
                "常驻条数要在 1–20 之间，收到 {}",
                self.depth
            )));
        }
        dreamina::normalize_opts(&opts)
    }
}

/// 补单器的一次决策（纯函数产物，便于测试）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    /// 这一轮补几条。
    pub take: i64,
    /// 要不要发「存量告急」通知。
    pub notify_low: bool,
    /// 没补满的原因（补满了就是 None）。界面据此说明「为什么没在跑」。
    pub blocked: Option<Blocked>,
}

/// 停下来的原因。用枚举而不是字符串：界面要按原因给不同的下一步动作
/// （没料了 → 去写提示词；额度满了 → 明天再说；余额不足 → 去充值）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blocked {
    /// 待提交存量见底 —— 唯一一个需要人去做点什么的原因。
    NoStock,
    /// 今日额度上限已用满。
    DailyCap,
    /// 账户余额不足。
    LowBalance,
}

impl Blocked {
    pub fn label(self) -> &'static str {
        match self {
            Self::NoStock => "没有待提交的存量了",
            Self::DailyCap => "今日额度上限已用满",
            Self::LowBalance => "账户余额不足",
        }
    }
}

/// 决定这一轮补几条。
///
/// `unit_cost` 是单条预估额度；查不到单价时为 `None` —— 那时**不做额度裁剪**而不是
/// 按 0 算：按 0 算等于把日限当作不存在，而这里宁可少限一层也不能编一个单价出来。
/// （单价查不到只会发生在没实测过的组合上，而默认模型是实测过的。）
///
/// `in_flight` 是**所有**在跑条目（不只是补单器自己放出去的），`hard_limit` 是即梦的
/// 账户级并发上限（`runner::effective_in_flight`）。0028 之前这两处都只算补单器自己的
/// 那份，理由是「手动提交的一批不该顶掉它的配额」—— 而实测证明配额本来就是共用的：
/// 人占满了唯一那个位子时，补单器再发出去的单子回来的是 `ExceedConcurrencyLimit`。
#[allow(clippy::too_many_arguments)]
pub fn plan(
    cfg: &AutofillCfg,
    in_flight: i64,
    hard_limit: i64,
    stock: i64,
    spent_today: i64,
    balance: Option<i64>,
    unit_cost: Option<i64>,
    since_last_notice: i64,
) -> Plan {
    if !cfg.enabled {
        return Plan::default();
    }
    // 存量告急与「补不补得动」是两件事：即便这一轮补满了，只要补完之后的存量低于
    // 水位线，就该现在提醒 —— 提醒的价值全在「提前」，等断流了再说毫无意义。
    let target = cfg.depth.min(hard_limit.max(1));
    let deficit = (target - in_flight).max(0);
    let mut take = deficit.min(stock);
    let mut blocked = None;

    if take < deficit {
        blocked = Some(Blocked::NoStock);
    }
    if let Some(unit) = unit_cost.filter(|u| *u > 0) {
        if cfg.daily_credits > 0 {
            let room = ((cfg.daily_credits - spent_today).max(0)) / unit;
            if room < take {
                take = room;
                blocked = Some(Blocked::DailyCap);
            }
        }
        if let Some(bal) = balance {
            let affordable = (bal / unit).max(0);
            if affordable < take {
                take = affordable;
                blocked = Some(Blocked::LowBalance);
            }
        }
    }

    let after = (stock - take).max(0);
    let notify_low =
        after < cfg.low_water && since_last_notice >= NOTICE_COOLDOWN_SECS && cfg.low_water > 0;
    Plan {
        take: take.max(0),
        notify_low,
        blocked,
    }
}

/// 循环局部的记忆（上次通知时刻）。放在循环里而不是库里：重启后重发一次可以接受，
/// 每 30 秒发一次不可接受。
#[derive(Debug, Clone, Copy, Default)]
pub struct Memo {
    pub last_notice_at: i64,
}

/// 一轮补单。跟着整表扫描的节拍走（它要看的「在跑几条」正是扫描刚更新过的）。
pub async fn tick(
    pool: &SqlitePool,
    settings: &crate::commands::v2v::V2vSettings,
    app: &tauri::AppHandle,
    log: &Activity,
    memo: &mut Memo,
) {
    let cfg = &settings.autofill;
    if !cfg.enabled {
        return;
    }
    let now = now_unix();
    let opts = match cfg.validate() {
        // 会话跟随全局设置：它决定任务落在即梦哪条历史里，与「这条队列多便宜」无关，
        // 没有理由让补单器把它丢掉（丢了就散落在默认会话里，事后翻都翻不到一起）。
        Ok(o) => GenOpts {
            session: settings.session,
            ..o
        },
        Err(e) => {
            // 配置非法就停下并说清楚，绝不「尽力而为」地按半套参数跑 ——
            // 那正是会按 CLI 默认通道烧钱的路径。
            log.error(
                "submit",
                None,
                format!("常驻队列配置非法，已跳过：{e}"),
                None,
            );
            return;
        }
    };
    let hard_limit = crate::v2v::runner::effective_in_flight(settings.max_in_flight);
    let (in_flight, stock, spent_today) = match tokio::try_join!(
        repo::count_in_flight(pool),
        repo::count_autofill_pool(pool),
        repo::credit_submitted_since(pool, now - 24 * 3600),
    ) {
        Ok(v) => v,
        Err(e) => {
            log.error("submit", None, format!("常驻队列读库失败：{e}"), None);
            return;
        }
    };
    let unit_cost = match (
        opts.model_version.as_deref(),
        opts.video_resolution.as_deref(),
        opts.duration,
    ) {
        (Some(m), Some(r), Some(d)) => dreamina::estimate_credits(m, r, d),
        _ => None,
    };
    // 余额是**尽力而为**：查一次要跑 CLI，而这一步每 30 秒发生一次。只在真要补单时才查。
    let target = cfg.depth.min(hard_limit.max(1));
    let deficit_now = (target - in_flight).max(0);
    let balance = if deficit_now > 0 {
        dreamina::user_credit(&settings.bin, log)
            .await
            .ok()
            .map(|c| c.total_credit)
    } else {
        None
    };

    let p = plan(
        cfg,
        in_flight,
        hard_limit,
        stock,
        spent_today,
        balance,
        unit_cost,
        now - memo.last_notice_at,
    );

    if p.notify_low {
        memo.last_notice_at = now;
        let body = format!(
            "待提交只剩 {stock} 条（水位线 {}）。常驻队列快断流了 —— 去作品库把新验收的图入队，或让改写 skill 跑一轮。",
            cfg.low_water
        );
        log.warn("submit", None, format!("常驻队列告急：{body}"), None);
        use crate::engine::events::EventSink;
        crate::engine::events::TauriSink::new(app.clone()).notify("视频常驻队列告急".into(), body);
    }

    if p.take <= 0 {
        if deficit_now > 0 {
            if let Some(b) = p.blocked {
                // 只记日志不发通知：这三种原因里只有「没料了」需要人动手，
                // 而那一条已经由上面的告急通知负责了。
                log.info(
                    "submit",
                    None,
                    format!(
                        "常驻队列本轮未补单：{}（在跑 {in_flight}/{target}）",
                        b.label()
                    ),
                    None,
                );
            }
        }
        return;
    }

    let ids = match repo::pick_autofill(pool, p.take).await {
        Ok(v) => v,
        Err(e) => {
            log.error("submit", None, format!("常驻队列挑单失败：{e}"), None);
            return;
        }
    };
    if ids.is_empty() {
        return;
    }
    // 参数先写进条目再提交：这样详情栏显示的参数与那条视频实际用的就是同一份，
    // 而不是「界面显示 skill 给的建议、实际发的是补单器的默认」。
    if let Err(e) = repo::set_params(
        pool,
        &ids,
        opts.model_version.as_deref(),
        opts.duration,
        opts.video_resolution.as_deref(),
        now,
    )
    .await
    {
        log.error("submit", None, format!("常驻队列写参数失败：{e}"), None);
        return;
    }
    // 标记必须在提交**之前**：提交成功那一刻钱就扣了，事后再标会在进程恰好被杀时
    // 留下一条补单器认不出是自己放的在跑条目 —— 于是它会以为深度没满，再补一条。
    if let Err(e) = repo::mark_auto(pool, &ids, now).await {
        log.error("submit", None, format!("常驻队列标记失败：{e}"), None);
        return;
    }
    log.info(
        "submit",
        None,
        format!(
            "常驻队列补单 {} 条 · 模型 {} · 在跑 {in_flight}/{target} · 今日已提交 {spent_today} 额度",
            ids.len(),
            opts.model_version.as_deref().unwrap_or("—"),
        ),
        None,
    );
    match crate::v2v::runner::submit_batch(pool, &settings.bin, &ids, &opts, log).await {
        Ok(sum) => {
            if sum.submitted > 0 {
                crate::commands::v2v::emit_changed(pool, app, None).await;
            }
        }
        Err(e) => log.error("submit", None, format!("常驻队列提交失败：{e}"), None),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即失败
mod tests {
    use super::*;

    fn cfg() -> AutofillCfg {
        AutofillCfg {
            enabled: true,
            depth: 3,
            model_version: "seedance2.0fast".into(),
            duration: Some(4),
            video_resolution: "720p".into(),
            low_water: 5,
            daily_credits: 200,
        }
    }

    // 关着就是关着 —— 默认值必须是「不自动花钱」。
    #[test]
    fn disabled_never_spends() {
        let mut c = cfg();
        c.enabled = false;
        assert_eq!(
            plan(&c, 0, 99, 100, 0, Some(9999), Some(8), 99999),
            Plan::default()
        );
        assert!(!AutofillCfg::default().enabled, "默认必须是关的");
    }

    // 常驻深度：跑满了就不补，缺几条补几条。
    #[test]
    fn refills_exactly_the_deficit() {
        assert_eq!(plan(&cfg(), 3, 99, 100, 0, Some(9999), Some(8), 0).take, 0);
        assert_eq!(plan(&cfg(), 1, 99, 100, 0, Some(9999), Some(8), 0).take, 2);
        assert_eq!(plan(&cfg(), 0, 99, 100, 0, Some(9999), Some(8), 0).take, 3);
    }

    // 即梦的账户级并发上限压过配置深度：设了 3 而上限是 1 时，只补到 1。
    //
    // 这是 0028 的核心修正。旧版只数补单器自己放出去的条目，于是人手动占满了唯一
    // 那个位子时它照样往外发，而那些单子回来的是 `ExceedConcurrencyLimit`。
    #[test]
    fn account_wide_concurrency_limit_wins_over_configured_depth() {
        assert_eq!(plan(&cfg(), 0, 1, 100, 0, Some(9999), Some(8), 0).take, 1);
        assert_eq!(
            plan(&cfg(), 1, 1, 100, 0, Some(9999), Some(8), 0).take,
            0,
            "那一个位子已经被占了 —— 无论占它的是谁"
        );
    }

    // VIP 模型必须在**保存设置**那一刻被拒：这条队列的全部前提就是便宜，
    // 配上 vip 等于每晚自动烧 5.5 倍的钱，而那笔钱是提交即扣、退不回来的。
    #[test]
    fn vip_channel_is_rejected_at_configuration_time() {
        let mut c = cfg();
        c.model_version = "seedance2.0fast_vip".into();
        let err = c.validate().unwrap_err().to_string();
        assert!(err.contains("VIP"), "{err}");
        // 关着的时候不校验：没开的配置不该挡住整个设置页保存。
        c.enabled = false;
        assert!(c.validate().is_ok());
    }

    // 「跟随 CLI 默认」同样要拒 —— 那等于把每晚的账单交给一个会随版本变的选择。
    #[test]
    fn blank_model_is_rejected_because_the_bill_must_be_explicit() {
        let mut c = cfg();
        c.model_version = "  ".into();
        assert!(c.validate().is_err());
    }

    // 日限按**提交**额度裁剪：剩下的额度只够两条就只补两条。
    #[test]
    fn daily_cap_clamps_the_batch() {
        // 日限 200，已提交 184 → 还剩 16 → 单价 8 → 只够 2 条。
        let p = plan(&cfg(), 0, 99, 100, 184, Some(9999), Some(8), 0);
        assert_eq!(p.take, 2);
        assert_eq!(p.blocked, Some(Blocked::DailyCap));
        // 用满了就一条都不补。
        let p = plan(&cfg(), 0, 99, 100, 200, Some(9999), Some(8), 0);
        assert_eq!(p.take, 0);
        assert_eq!(p.blocked, Some(Blocked::DailyCap));
    }

    // 余额不足要**提前**停，不能提交到一半才开始报错：即梦逐条扣费，
    // 前面扣掉的退不回来。
    #[test]
    fn low_balance_stops_before_spending_the_last_credits() {
        let p = plan(&cfg(), 0, 99, 100, 0, Some(9), Some(8), 0);
        assert_eq!(p.take, 1, "余额 9 单价 8 → 只够一条");
        assert_eq!(p.blocked, Some(Blocked::LowBalance));
        let p = plan(&cfg(), 0, 99, 100, 0, Some(3), Some(8), 0);
        assert_eq!(p.take, 0);
    }

    // 存量不够就只补得动几条补几条，并把原因标成「没料了」——
    // 这是三种停因里唯一一个需要人动手的。
    #[test]
    fn empty_stock_is_the_only_reason_that_needs_a_human() {
        let p = plan(&cfg(), 0, 99, 1, 0, Some(9999), Some(8), 0);
        assert_eq!(p.take, 1);
        assert_eq!(p.blocked, Some(Blocked::NoStock));
    }

    // 告急通知在存量**见底之前**发（提醒的价值全在提前），且要有冷却 ——
    // 每 30 秒弹一次的通知，人会在第三次就把它关掉，然后这条链路永久失灵。
    #[test]
    fn low_water_warning_fires_early_and_only_after_the_cooldown() {
        // 存量 6，补 3 条之后剩 3 < 水位线 5 → 现在就该提醒。
        assert!(
            plan(
                &cfg(),
                0,
                99,
                6,
                0,
                Some(9999),
                Some(8),
                NOTICE_COOLDOWN_SECS
            )
            .notify_low
        );
        // 刚提醒过 → 冷却期内不再吵。
        assert!(!plan(&cfg(), 0, 99, 6, 0, Some(9999), Some(8), 60).notify_low);
        // 存量充足 → 不提醒。
        assert!(!plan(&cfg(), 0, 99, 50, 0, Some(9999), Some(8), 99999).notify_low);
    }

    // 单价查不到时**不做额度裁剪**，而不是按 0 算把日限当作不存在。
    #[test]
    fn unknown_unit_price_skips_clamping_rather_than_treating_it_as_free() {
        let p = plan(&cfg(), 0, 99, 100, 100_000, Some(0), None, 0);
        assert_eq!(
            p.take, 3,
            "算不出单价就不裁剪，交给下一层（提交时余额不足会报错）"
        );
        assert_eq!(p.blocked, None);
    }

    // 深度上限：把它设成 500 会在一夜之间把余额跑光。
    #[test]
    fn depth_is_bounded() {
        let mut c = cfg();
        c.depth = 500;
        assert!(c.validate().is_err());
        c.depth = 0;
        assert!(c.validate().is_err());
    }
}
