//! 即梦（Dreamina）CLI 封装 —— 图生视频的执行下游。
//!
//! 地位等同 `provider::openai`：引擎的下游，而不是流程的主人。提交/轮询/下载在本机做，
//! 因为这些都不是智能任务——让 LLM 在 agent 循环里轮询既慢又贵还不可靠。
//!
//! ## 为什么把命令行摆到确认之前
//!
//! CLI 的 flags 会随版本变（skill 文档自己就写着「不要硬编码模型支持」）。这里的对策不是
//! 赌它不变，而是让 [`command_line`] 成为**执行与展示的同一个来源**：提交确认卡里显示的
//! 就是即将 exec 的那串 argv。「我设了 1080p 却没生效」这类怀疑只能靠把真实请求摆到
//! 确认之前来消除（同 v0.14.0 处理生成参数的做法）。
//!
//! ## 为什么不用 CLI 自带的 `--poll`
//!
//! `--poll=N` 让进程守着等最多 N 秒。那意味着轮询状态活在一个子进程里：应用重启、
//! 用户关窗、进程被杀，全部丢失，而额度已经扣了。改由本机轮询器按 submit_id 认领，
//! 状态在库里，重启照样接得上。

use std::path::Path;

use serde::Serialize;
use specta::Type;

use crate::error::{AppError, AppResult};
use crate::v2v::activity::{Activity, Who};

/// 默认可执行名。设置里留空即用它，并走 [`resolve_bin`] 自动探测。
pub const DEFAULT_BIN: &str = "dreamina";

/// 默认模型：够用的最便宜档。
///
/// 实测（2026-07-27，同账号同一张首帧图，都是 4s / 720p / 竖版 720×1280）：
/// - `seedance2.0fast` → 队列 `dreamina_fusion_video40`，**8 额度**
/// - `seedance2.0fast_vip` → 队列 `dreamina_fusion_video40_vision`，**44 额度**
///
/// 5.5 倍差价，输出规格一模一样；vip 通道换来的是不排队（实测直接 `Generating`，
/// 而非 vip 排在第 4485 位）。B-Roll 空镜是可以过夜的活，用不着为插队付 5.5 倍。
/// 要赶时间就在设置里临时改成 vip —— 但那必须是一次显式的选择。
pub const DEFAULT_MODEL: &str = "seedance2.0fast";

/// 除 `PATH` 之外还要翻的安装目录。
///
/// **「走 PATH」对 GUI 应用基本是句空话**：macOS 上从 Finder/Dock 启动的进程不经过登录
/// shell，`PATH` 恒为 `/usr/bin:/bin:/usr/sbin:/sbin`，而 dreamina 的默认安装位置
/// `~/.local/bin` 不在其中 —— 于是终端里跑得好好的命令，在应用里必然「找不到」。
/// 从终端 `pnpm tauri dev` 起的开发实例反而继承了完整 PATH，正好把这个坑藏起来。
fn probe_dirs() -> Vec<std::path::PathBuf> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from);
    let rel = [
        ".local/bin",
        "bin",
        ".cargo/bin",
        ".bun/bin",
        "go/bin",
        ".npm-global/bin",
    ];
    let abs = [
        "/opt/homebrew/bin",
        "/usr/local/bin",
        "/opt/local/bin",
        "/usr/bin",
    ];
    home.iter()
        .flat_map(|h| rel.iter().map(|r| h.join(r)))
        .chain(abs.iter().map(std::path::PathBuf::from))
        .collect()
}

/// 该路径是否是一个能执行的文件。
fn is_exec(p: &Path) -> bool {
    if !p.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Windows 下同一个名字要试几个扩展名。
fn name_variants(name: &str) -> Vec<String> {
    #[cfg(windows)]
    {
        if Path::new(name).extension().is_some() {
            return vec![name.to_string()];
        }
        ["", ".exe", ".cmd", ".bat"]
            .iter()
            .map(|e| format!("{name}{e}"))
            .collect()
    }
    #[cfg(not(windows))]
    {
        vec![name.to_string()]
    }
}

/// 把设置里的 `bin` 解析成一个**确实存在且可执行**的绝对路径。
///
/// 三条分支：
/// - 填了路径（含分隔符）→ 只认它，不存在就直说是哪个路径不存在（别偷偷回退到探测结果，
///   否则用户填错了路径却「跑起来了」，下次换台机器又神秘失败）。
/// - 留空 → 用默认名去探。
/// - 裸名字 → 先 `PATH`，再 [`probe_dirs`]。
///
/// 找不到时把翻过的目录一并报出来：这个错误的唯一有用信息就是「我找过哪儿」。
pub fn resolve_bin(configured: &str) -> AppResult<String> {
    let path_dirs = std::env::var_os("PATH")
        .map(|v| std::env::split_paths(&v).collect::<Vec<_>>())
        .unwrap_or_default();
    let dirs: Vec<std::path::PathBuf> = path_dirs.into_iter().chain(probe_dirs()).collect();
    resolve_in(configured, &dirs)
}

/// [`resolve_bin`] 的内核，搜索目录由外部给。
///
/// 抽出来是为了可测：本仓库 `-F unsafe-code`，而改 `PATH` 需要 `unsafe`（Rust 2024 起
/// `set_var` 是 unsafe fn），所以测试只能从这一侧注入目录。
fn resolve_in(configured: &str, dirs: &[std::path::PathBuf]) -> AppResult<String> {
    let raw = configured.trim();
    if raw.contains('/') || raw.contains('\\') {
        let p = Path::new(raw);
        return if is_exec(p) {
            Ok(p.to_string_lossy().to_string())
        } else {
            Err(AppError::InvalidInput(format!(
                "设置里填的即梦 CLI 路径不可用：{raw}\n（文件不存在，或没有执行权限）"
            )))
        };
    }
    let name = if raw.is_empty() { DEFAULT_BIN } else { raw };
    for dir in dirs {
        for variant in name_variants(name) {
            let cand = dir.join(&variant);
            if is_exec(&cand) {
                return Ok(cand.to_string_lossy().to_string());
            }
        }
    }
    let looked: Vec<String> = probe_dirs()
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    Err(AppError::InvalidInput(format!(
        "找不到即梦 CLI「{name}」。请先安装并在终端跑一次 `dreamina login`；\
         若已安装，请在设置页填它的绝对路径（终端里 `which dreamina` 的输出）。\n\
         已找过 PATH 与：{}",
        looked.join("、")
    )))
}

/// 探测结果（设置页展示用）：找到就给绝对路径，找不到给 `None`。
pub fn detect_bin(configured: &str) -> Option<String> {
    resolve_bin(configured).ok()
}

/// 即梦模型族的取值与约束。**只作提交前的本地预检**，最终真相仍是服务端。
///
/// 预检的价值在于「花钱之前拦住」：组合不合法时 CLI 会拒，但那时已经走了一趟网络，
/// 而批量提交 20 条会连报 20 次同样的错。
///
/// **这张表的取值来自实测，不是抄 `dreamina image2video -h`**（2026-07-27）。
/// CLI 帮助文本里三处与服务端不符，照抄会在花钱那一刻才炸：
///   - `seedance1.0fast` 帮助写 3–10，服务端要 **5–10**（`ret=10001 duration should >=5 && <=10`）
///   - `seedance1.5pro`  帮助写 4–12，服务端要 **5–12**
///   - `seedance1.0`     帮助写 720p，服务端只收 **1080p**，而 CLI 本地又拦着
///     「1080p 只能配 seedance2.0_vip」—— 两边打架，此模型经 CLI **根本发不出去**，
///     故整条从表里删掉：留着只会让人选中后必然失败。
const MODELS: &[(&str, i64, i64, &[&str])] = &[
    // (model_version, 最短时长, 最长时长, 允许的分辨率)
    ("seedance1.0fast", 5, 10, &["720p"]),
    ("seedance1.5pro", 5, 12, &["720p"]),
    ("seedance2.0", 4, 15, &["720p"]),
    ("seedance2.0fast", 4, 15, &["720p"]),
    ("seedance2.0_vip", 4, 15, &["720p", "1080p", "4k"]),
    ("seedance2.0fast_vip", 4, 15, &["720p"]),
    ("seedance2.0mini", 4, 15, &["720p"]),
];

/// 每秒单价（额度/秒），按 (model_version, video_resolution) 查。
///
/// 即梦**没有价格查询接口**，价格只在提交回体的 `credit_count` 里出现一次 —— 而那时
/// 已经扣完了。所以这张表是 2026-07-27 逐个通道实拍出来的：同一张首帧图各发一条，
/// 记回执单价，账面 13785→13631 与五条回执之和 154 分毫不差，确认「提交即扣费」。
///
/// **线性**：`credit = 单价 × 时长秒数`，两个通道各有两个时长点可交叉验证
/// （fast 8@4s/10@5s → 2；fast_vip 44@4s/55@5s → 11）。分辨率单列一维，因为
/// 2.0_vip 的 720p 与 4k 差 5.7 倍，不是时长能解释的。
///
/// 查不到 = **不猜**。`estimate_credits` 返回 None，界面显示「未实测」，
/// 宁可说不知道，也不能给一个像模像样的错数字诱导人点确认。
const PRICES: &[(&str, &str, i64)] = &[
    // (model_version, video_resolution, 额度/秒)
    ("seedance1.0fast", "720p", 2),      // 实测 10 @ 5s
    ("seedance1.5pro", "720p", 8),       // 实测 40 @ 5s
    ("seedance2.0", "720p", 3),          // 实测 12 @ 4s
    ("seedance2.0fast", "720p", 2),      // 实测 8 @ 4s、10 @ 5s
    ("seedance2.0fast_vip", "720p", 11), // 实测 44 @ 4s、55 @ 5s
    ("seedance2.0mini", "720p", 9),      // 实测 36 @ 4s
    ("seedance2.0_vip", "720p", 14),     // 实测 56 @ 4s
    ("seedance2.0_vip", "4k", 80),       // 实测 320 @ 4s
                                         // 2.0_vip / 1080p 未实测：留空比编一个数诚实。
];

/// 预估一次提交要扣多少额度；查不到单价返回 `None`（界面必须显示「未实测」而非 0）。
pub fn estimate_credits(model_version: &str, video_resolution: &str, duration: i64) -> Option<i64> {
    PRICES
        .iter()
        .find(|(m, r, _)| *m == model_version && *r == video_resolution)
        .map(|(_, _, per_sec)| per_sec * duration)
}

/// 某分辨率下的每秒单价（前端算「这一批预估多少额度」用）。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ResPrice {
    pub resolution: String,
    /// 额度/秒。查不到实测值的组合**不出现在这个列表里** —— 缺席即「未实测」，
    /// 前端据此标「≥」，不会把一个编出来的数字摆到「确认提交」旁边。
    pub credit_per_sec: i64,
}

/// 受控模型清单（前端选择器渲染源）。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub model_version: String,
    /// 通道简写（[`short_label`]）。界面上凡是要在一行里挤下型号的地方都读它。
    pub label: String,
    pub min_duration: i64,
    pub max_duration: i64,
    pub resolutions: Vec<String>,
    /// 最短时长 + 首个分辨率下的预估额度 —— 选择器里那行「≈N 额度/条」。
    /// 选模型这一刻才是价格该出现的地方：44 与 8 差 5.5 倍，选完再告知就晚了。
    pub credit_at_min: Option<i64>,
    /// 单价表切片。看板要在**分节头**上算「确认提交 18 条 · 预估 144 额度」，
    /// 那是每次筛选/勾选都会变的数，不能每渲染一次就往后端跑一趟；而把价格表抄一份
    /// 到前端又必然与 `PRICES` 分叉。故把这一小片真相随模型清单一起发过去。
    pub res_prices: Vec<ResPrice>,
    /// 是否是 vip 通道 —— 界面上要把它标出来（同规格贵 5.5 倍，只买到不排队）。
    pub vip: bool,
}

/// 这个通道要不要花「不排队」那笔钱。**判据单点定义在这里**。
///
/// 五处曾各写一遍 `ends_with("_vip")`（补单器的拒收闸、轮询分档、模型清单、前端两处）。
/// 即梦哪天出一个不带 `_vip` 后缀的付费加急档，补单器会整夜按 5.5 倍价往外提交，
/// 而那五处会一处一处地被人想起来 —— 或者想不起来。
///
/// 前端不再自己判：`ModelInfo.vip` 就是这个函数的结果，随模型清单一起下发。
pub fn is_vip(model_version: &str) -> bool {
    model_version.ends_with("_vip")
}

/// 通道简写 —— 顶部那排通道状态灯上的名字（`seedance2.0fast` → `2.0Fast`）。
///
/// **单点定义在这里**，随 [`ModelInfo::label`] 一起下发。型号全名在一个 20px 高的
/// pill 里放不下，而让每个消费者各自 `replace(/^seedance/, "")` 一遍的结果是
/// 「2.0fast」「2.0Fast」「2.0 Fast」三种拼法同屏出现（前端此前那个 `shortModel`
/// 就是这么来的）。
///
/// 空串 = 设置里没指定模型，实际通道由 CLI 自己挑 —— 那时**必须**说「CLI 默认」
/// 而不是编一个型号名：它是一条我们叫不出名字的通道，而叫错名字比不叫更糟。
pub fn short_label(model_version: &str) -> String {
    let m = model_version.trim();
    if m.is_empty() {
        return "CLI 默认".to_string();
    }
    let (base, vip) = match m.strip_suffix("_vip") {
        Some(b) => (b, true),
        None => (m, false),
    };
    let base = base.strip_prefix("seedance").unwrap_or(base);
    // 只抬「字母段的首字母」：数字与点原样留着，`2.0fast` → `2.0Fast`。
    let mut out = String::with_capacity(base.len() + 4);
    let mut head = true;
    for c in base.chars() {
        if c.is_ascii_alphabetic() {
            if head {
                out.extend(c.to_uppercase());
            } else {
                out.push(c);
            }
            head = false;
        } else {
            out.push(c);
            head = true;
        }
    }
    if vip {
        out.push_str(" VIP");
    }
    out
}

pub fn models() -> Vec<ModelInfo> {
    MODELS
        .iter()
        .map(|(m, lo, hi, res)| ModelInfo {
            model_version: (*m).to_string(),
            label: short_label(m),
            min_duration: *lo,
            max_duration: *hi,
            resolutions: res.iter().map(|r| (*r).to_string()).collect(),
            credit_at_min: estimate_credits(m, res[0], *lo),
            res_prices: res
                .iter()
                .filter_map(|r| {
                    PRICES
                        .iter()
                        .find(|(pm, pr, _)| pm == m && pr == r)
                        .map(|(_, _, per_sec)| ResPrice {
                            resolution: (*r).to_string(),
                            credit_per_sec: *per_sec,
                        })
                })
                .collect(),
            vip: is_vip(m),
        })
        .collect()
}

/// 一次提交的生成参数（三个「高级控制」要么都不给、要么给一套合法组合）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenOpts {
    pub model_version: Option<String>,
    pub duration: Option<i64>,
    pub video_resolution: Option<String>,
    pub session: Option<i64>,
}

/// 校验并补全三件套。
///
/// CLI 的规则是「omit advanced controls to use the default path」或「三者提供一套合法组合」，
/// 半套是最容易踩的坑：只填 duration 会被拒，而错误信息发生在花钱之后。
/// 故这里的策略是**给了任一项就补齐三项**，补不出合法值就当场报错。
pub fn normalize_opts(opts: &GenOpts) -> AppResult<GenOpts> {
    if opts.model_version.is_none() && opts.duration.is_none() && opts.video_resolution.is_none() {
        // 全不给 → 走 CLI 默认路径（最稳，且不锁定模型名）。
        return Ok(GenOpts {
            model_version: None,
            duration: None,
            video_resolution: None,
            session: opts.session,
        });
    }
    let model = opts.model_version.as_deref().ok_or_else(|| {
        AppError::InvalidInput(
            "填了时长或分辨率就必须同时指定模型：即梦 CLI 只接受「三者都不给」或「一套完整组合」"
                .into(),
        )
    })?;
    let Some((_, lo, hi, res)) = MODELS.iter().find(|(m, ..)| *m == model) else {
        return Err(AppError::InvalidInput(format!(
            "未知模型 {model}；受支持的取值见设置页选择器（最终以 dreamina image2video -h 为准）"
        )));
    };
    let duration = opts.duration.unwrap_or(*lo);
    if duration < *lo || duration > *hi {
        return Err(AppError::InvalidInput(format!(
            "{model} 的时长范围是 {lo}–{hi} 秒，收到 {duration}"
        )));
    }
    let resolution = opts
        .video_resolution
        .clone()
        .unwrap_or_else(|| res[0].to_string());
    if !res.contains(&resolution.as_str()) {
        return Err(AppError::InvalidInput(format!(
            "{model} 只支持 {} 分辨率，收到 {resolution}",
            res.join(" / ")
        )));
    }
    Ok(GenOpts {
        model_version: Some(model.to_string()),
        duration: Some(duration),
        video_resolution: Some(resolution),
        session: opts.session,
    })
}

/// 构造 `image2video` 的完整 argv（**执行与展示的同一来源**）。
///
/// 提示词整条作为一个 argv 元素传入，不经 shell：改写结果里必然有引号、换行、中文标点，
/// 任何形式的字符串拼接都会在某条上炸掉或被 shell 吃掉一部分。
pub fn command_line(bin: &str, image: &str, prompt: &str, opts: &GenOpts) -> Vec<String> {
    let mut argv = vec![
        bin.to_string(),
        "image2video".to_string(),
        format!("--image={image}"),
        format!("--prompt={prompt}"),
    ];
    if let Some(m) = &opts.model_version {
        argv.push(format!("--model_version={m}"));
    }
    if let Some(d) = opts.duration {
        argv.push(format!("--duration={d}"));
    }
    if let Some(r) = &opts.video_resolution {
        argv.push(format!("--video_resolution={r}"));
    }
    if let Some(s) = opts.session {
        argv.push(format!("--session={s}"));
    }
    // 自建轮询器接管等待：子进程守着等，重启即失忆，而额度已经扣了。
    argv.push("--poll=0".to_string());
    argv
}

/// 供 UI 展示的一行命令（提示词截断，避免确认卡被 400 字的提示词撑爆）。
pub fn display_command(argv: &[String]) -> String {
    argv.iter()
        .map(|a| {
            if let Some(rest) = a.strip_prefix("--prompt=") {
                let short: String = rest.chars().take(40).collect();
                let ellipsis = if rest.chars().count() > 40 { "…" } else { "" };
                format!("--prompt=\"{short}{ellipsis}\"")
            } else if a.contains(' ') {
                format!("\"{a}\"")
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// 从 CLI 的 stdout 里抽出 JSON 体。
///
/// CLI 当前只打 JSON，但它也会在别的路径上打人类可读的提示行；容错到「第一个 `{` 到
/// 最后一个 `}`」比要求整段是纯 JSON 稳得多，且不会误吃 JSON 内部的花括号。
pub fn extract_json(stdout: &str) -> AppResult<serde_json::Value> {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(stdout.trim()) {
        return Ok(v);
    }
    // 对象与数组各试一次，取先解析成功的那个。
    //
    // 不能把两种括号混在一个 find/rfind 里：CLI 打的日志行常以 `[info]` 开头，
    // 混着找会把 `[` 当成 JSON 起点，于是切出 `[info] …{…}` 这种必然解析失败的片段
    // —— 明明有合法 JSON 却报「不是合法 JSON」。
    let mut last_err: Option<String> = None;
    for (open, close) in [('{', '}'), ('[', ']')] {
        if let (Some(s), Some(e)) = (stdout.find(open), stdout.rfind(close)) {
            if e > s {
                match serde_json::from_str(&stdout[s..=e]) {
                    Ok(v) => return Ok(v),
                    Err(err) => last_err = Some(err.to_string()),
                }
            }
        }
    }
    Err(match last_err {
        Some(err) => AppError::Internal(format!("dreamina 输出不是合法 JSON：{err}")),
        None => AppError::Internal(format!(
            "dreamina 输出里找不到 JSON：{}",
            stdout.chars().take(200).collect::<String>()
        )),
    })
}

/// 递归找 `submit_id`。
///
/// 提交返回体的确切形状随版本变（顶层 / 嵌在 data 里都可能），而我们只需要那一个值。
/// 递归查找让「字段挪了个层级」这种变动不至于让整条链断掉。
pub fn extract_submit_id(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::Object(map) => {
            if let Some(s) = map.get("submit_id").and_then(|x| x.as_str()) {
                if !s.is_empty() {
                    return Some(s.to_string());
                }
            }
            map.values().find_map(extract_submit_id)
        }
        serde_json::Value::Array(arr) => arr.iter().find_map(extract_submit_id),
        _ => None,
    }
}

/// 轮询判定结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// 还在跑（pending / queue / running / querying）。
    Running,
    /// 出片了。
    Done,
    /// 终态失败（expired / canceled / 未知态兜底）。
    Failed,
}

/// `gen_status` → 判定。
///
/// 取值来自 CLI 二进制里的枚举：pending / queue / running / querying / success /
/// PartialSuccess / expired / Expired / canceled（大小写不统一，故一律小写后比对）。
///
/// **未知态判 Running 而不是 Failed**：CLI 加一个新的中间态时，判失败会把正在跑、
/// 额度已扣的任务当场标死；判运行只是多轮询几轮，最坏由超时兜底。
pub fn classify_status(gen_status: &str) -> Outcome {
    match gen_status.to_ascii_lowercase().as_str() {
        "success" | "partialsuccess" => Outcome::Done,
        "expired" | "canceled" | "cancelled" | "failed" | "fail" => Outcome::Failed,
        _ => Outcome::Running,
    }
}

/// 这份失败原因是不是「同时在跑的太多了」。
///
/// 实测回体：`api error: ret=1310, message=ExceedConcurrencyLimit, logid=2026…`。
/// 2026-07-28 一批 9 条同时提交，即梦逐条给了 submit_id（提交侧「成功 9 · 失败 0」），
/// 随后 8 条以这个原因判失败 —— 非 VIP 通道同一时间只跑得动 1 条。
///
/// 它和别的失败**不是一类东西**：一分钱没扣、任务从没跑过、提示词与图都没问题，
/// 唯一的问题是排在了后面。记成 `fail` 就得人一条条去点重跑，而重跑又会撞上同一堵墙。
/// 故 `runner::settle` 见到它是把条目放回本地队列，不是判死。
///
/// 认 `ret=1310` 与英文 message 两路：数字是协议、文案会被翻译或改写，
/// 而两者同时变的可能性远低于任何一个单独变。
pub fn is_concurrency_reject(reason: &str) -> bool {
    let r = reason.to_ascii_lowercase();
    r.contains("exceedconcurrencylimit")
        || r.contains("ret=1310")
        || r.contains("concurrency limit")
}

/// 一次 `query_result` 的解析结果。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct QueryResult {
    pub gen_status: String,
    pub fail_reason: String,
    /// 已下载到本地的视频绝对路径（传了 `--download_dir` 且成功才有）。
    pub video_path: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub fps: Option<f64>,
    pub duration_sec: Option<f64>,
    pub credit_count: Option<i64>,
    /// 队列位次。**健康的任务从排队第一秒起就有它**。
    ///
    /// 早前这里写着「即梦当前不回传它」，那条结论是从一批坏单上归纳出来的 ——
    /// 取样正是下面 `queued_payload_has_no_queue_position` 用的 `027e202c`，
    /// 而那条属于 2026-07-27 事故里 18 条从未入队的幽灵单。
    ///
    /// 实测（同账号、同 `seedance2.0fast` 通道、提交后 25 秒）：
    /// `{queue_idx: 4485, priority: 1, queue_status: "Queueing", queue_length: 574522}`。
    /// 完成后变成 `{queue_idx: 0, queue_status: "Finish", queue_length: 0}`。
    ///
    /// 所以它缺席不是常态而是**征兆**：`queue_idx` 与 `credit_count` 双双为 None，
    /// 就是「即梦接了单但没入队」。判定见 `runner::is_phantom`。
    pub queue_idx: Option<i64>,
    /// 整条全局队列有多长（实测样本 `queue_length: 574522`）。
    ///
    /// 它与 [`Self::queue_idx`] 一起才说得出「排在什么位置」：第 4485 位在 57 万人的队里
    /// 是前 1%，在 5000 人的队里是队尾。轨迹图上它是第二条曲线 —— 位次不动而队长在涨，
    /// 说明是新单涌进来而不是这条队停了。
    pub queue_length: Option<i64>,
    /// 实际计费型号（`commerce_info.triplets[].benefit_type`），形如
    /// `dreamina_seedance_20_fast_5s`。这是**回执**，不是我们的输入 ——
    /// 「到底走的哪个模型」只有它答得准。
    pub benefit_type: Option<String>,
}

/// 解析 `query_result` 的 JSON。
pub fn parse_query(v: &serde_json::Value) -> QueryResult {
    let gen_status = v
        .get("gen_status")
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_string();
    let video = v
        .get("result_json")
        .and_then(|r| r.get("videos"))
        .and_then(|x| x.as_array())
        .and_then(|arr| arr.first());
    QueryResult {
        gen_status,
        fail_reason: v
            .get("fail_reason")
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_string(),
        // 只认 `path`（--download_dir 落盘后的本地路径），不认 `video_url`：
        // URL 带签名会过期，存进库里等于存了一条几小时后必然 404 的引用。
        video_path: video
            .and_then(|x| x.get("path"))
            .and_then(|x| x.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string()),
        width: video.and_then(|x| x.get("width")).and_then(|x| x.as_i64()),
        height: video.and_then(|x| x.get("height")).and_then(|x| x.as_i64()),
        fps: video.and_then(|x| x.get("fps")).and_then(|x| x.as_f64()),
        duration_sec: video
            .and_then(|x| x.get("duration"))
            .and_then(|x| x.as_f64()),
        // 扣费额度两处都认：`query_result` 打在顶层，`list_task` 塞在 commerce_info 里。
        // 同一个数字换了个位置就读不到，是最不值得的那种失败。
        credit_count: v.get("credit_count").and_then(|x| x.as_i64()).or_else(|| {
            v.get("commerce_info")
                .and_then(|c| c.get("credit_count"))
                .and_then(|x| x.as_i64())
        }),
        queue_idx: v
            .get("queue_info")
            .and_then(|q| q.get("queue_idx"))
            .and_then(|x| x.as_i64()),
        queue_length: v
            .get("queue_info")
            .and_then(|q| q.get("queue_length"))
            .and_then(|x| x.as_i64()),
        benefit_type: v
            .get("commerce_info")
            .and_then(|c| {
                // 复数 `triplets` 是实际用到的那条；单数 `triplet` 实测是空壳，
                // 只在复数缺席时才退回去看它。
                c.get("triplets")
                    .and_then(|t| t.as_array())
                    .and_then(|a| a.first())
                    .or_else(|| c.get("triplet"))
            })
            .and_then(|t| t.get("benefit_type"))
            .and_then(|x| x.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string()),
    }
}

/// 一次 CLI 调用等多久才放弃 —— **按「杀掉它的代价」分档，不是按「它一般跑多久」**。
///
/// ## 为什么必须有超时
///
/// 原来这里一个都没有：`Command::output()` 一直等到子进程自己结束。于是 CLI 一挂
/// （网络吊住、等一个永远不来的响应），那条 IPC 命令就永不返回，前端的 `busyRef`
/// 永不释放 —— 用户看到的「提交页面卡住」，最狠的那个版本就是这么来的。
///
/// ## 为什么不能一刀切一个数
///
/// 超时的动作是**杀掉子进程**，而杀掉它的代价三档完全不同：
///
/// - 只读查询：杀了什么也没发生，下一轮再问就是了 → 可以给得很紧。
/// - 带下载的查询：杀了只是这一次没下成，成片还在即梦那边 → 给宽一点，别把大文件
///   下到一半掐了；重来一次不花钱。
/// - **提交：杀掉的那个进程可能已经下过单、扣过费了**。submit_id 是那笔钱唯一的凭证，
///   而它随进程一起没了。这与 `runner::persist_submit` 防的是同一类事故，所以这一档
///   给到十分钟（正常几秒），并且真超时了要按 error 级喊出来、判给人看，
///   绝不能当成「没花钱的失败」悄悄退回去重跑。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Timeout {
    /// 只读查询（`user_credit` / `list_task` / 不带下载的 `query_result` / `session`）。
    Read,
    /// 带 `--download_dir` 的 `query_result`：要把成片拉到本地。
    Download,
    /// 提交。杀它可能丢掉一笔已经花出去的钱。
    Submit,
    /// 测试用的短超时。生产档位最短也有 60 秒，等它跑完的测试没人会留着。
    #[cfg(test)]
    Custom(std::time::Duration),
}

impl Timeout {
    pub fn duration(self) -> std::time::Duration {
        match self {
            Self::Read => std::time::Duration::from_secs(60),
            Self::Download => std::time::Duration::from_secs(300),
            Self::Submit => std::time::Duration::from_secs(600),
            #[cfg(test)]
            Self::Custom(d) => d,
        }
    }

    /// 超时后写进执行日志与错误里的那句话。**提交那一档必须写明钱的下落**：
    /// 它是人此刻唯一能拿到的线索，而下一步（重跑还是先去核对）取决于它。
    fn message(self, ms: u128) -> String {
        let secs = self.duration().as_secs();
        match self {
            Self::Read => format!("即梦 CLI 超过 {secs} 秒没有响应，已终止（{ms}ms）。只读查询，未产生任何副作用，下一轮会自动恢复。"),
            Self::Download => format!("即梦 CLI 下载成片超过 {secs} 秒未完成，已终止（{ms}ms）。成片仍在即梦那边，下一轮会重新下载，不额外花钱。"),
            Self::Submit => format!(
                "即梦 CLI 提交超过 {secs} 秒没有响应，已终止（{ms}ms）。\
                 **这一单可能已经下出去并扣了费，而 submit_id 随进程一起丢了。**\
                 恢复等于再花一份钱 —— 先用这条提示词去即梦的任务列表核对有没有同一条在跑，\
                 确认没有再恢复。"
            ),
            #[cfg(test)]
            Self::Custom(_) => format!("即梦 CLI 超过 {secs} 秒没有响应，已终止（{ms}ms）。"),
        }
    }
}

/// 跑一条 CLI 命令，回 stdout。
///
/// 子进程走 `tokio::process` + `kill_on_drop`，外面套一层 [`Timeout`]：CLI 一次调用要走
/// 网络，秒级到十几秒，而它挂住的时候必须有个尽头（见 [`Timeout`] 的说明）。
///
/// **失败一律记进执行日志**（含退出码、耗时与 CLI 打出来的原文）：这是「有没有报错」
/// 在 GUI 里唯一的答案来源 —— 打包后的应用没有终端，`tracing` 写进的那个文件用户不会去看。
///
/// 成功只在 `loud` 时记。轮询每 6 秒问 19 条，成功也记就是每分钟 190 条，
/// 500 条的缓冲两分半钟就被冲干净 —— 那等于用「一切正常」把真正的报错挤出了窗口。
/// 付费动作（提交）与人按下的按钮才配 `loud`。
async fn run(
    argv: Vec<String>,
    log: &Activity,
    who: Who<'_>,
    loud: bool,
    timeout: Timeout,
) -> AppResult<String> {
    let pretty = display_command(&argv);
    let started = std::time::Instant::now();
    let spawned = (|| {
        let (bin, args) = argv.split_first().ok_or_else(|| {
            AppError::Internal("命令行为空（不应发生：command_line 至少给出 bin）".into())
        })?;
        tokio::process::Command::new(bin)
            .args(args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            // 超时那一刻我们只是丢掉这个 future，子进程得跟着一起走 —— 否则「超时」
            // 只是让我们不再等它，而那个还连着网络的进程留在原地。
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => AppError::InvalidInput(format!(
                    "找不到即梦 CLI「{bin}」。请先安装并 `dreamina login`，或在设置里填它的绝对路径。"
                )),
                _ => AppError::Io(format!("启动即梦 CLI 失败：{e}")),
            })
    })();

    let child = match spawned {
        Ok(c) => c,
        Err(e) => {
            let ms = started.elapsed().as_millis();
            log.error(
                "cli",
                who,
                format!("即梦 CLI 无法启动（{ms}ms）：{e}"),
                Some(pretty),
            );
            return Err(e);
        }
    };

    let waited = tokio::time::timeout(timeout.duration(), child.wait_with_output()).await;
    let ms = started.elapsed().as_millis();
    let out = match waited {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => {
            let e = AppError::Io(format!("等待即梦 CLI 结束失败：{e}"));
            log.error(
                "cli",
                who,
                format!("即梦 CLI 出错（{ms}ms）：{e}"),
                Some(pretty),
            );
            return Err(e);
        }
        Err(_) => {
            log.error("cli", who, timeout.message(ms), Some(pretty.clone()));
            return Err(AppError::Timeout(format!(
                "{}\n命令：{pretty}",
                timeout.message(ms)
            )));
        }
    };

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        // stdout 也带进去：CLI 的业务错误（额度不足、需网页授权）常打在 stdout。
        let detail: String = format!("{stderr}{stdout}").chars().take(400).collect();
        let code = out
            .status
            .code()
            .map_or_else(|| "signal".to_string(), |c| c.to_string());
        log.error(
            "cli",
            who,
            format!(
                "即梦 CLI 返回失败（退出码 {code}，{ms}ms）：{}",
                detail.trim()
            ),
            Some(pretty.clone()),
        );
        return Err(AppError::Internal(format!(
            "即梦 CLI 返回失败（{code}）：{}\n命令：{pretty}",
            detail.trim()
        )));
    }
    if loud {
        log.info("cli", who, format!("即梦 CLI 完成（{ms}ms）"), Some(pretty));
    }
    Ok(stdout)
}

/// 账号与余额（`user_credit` 的完整回体）。
///
/// 不只取 `total_credit`：「走的是哪个账号、什么等级」与「还剩多少」是同一个问题的两半，
/// 而余额对不上时，第一个要排除的正是「登录的不是我以为的那个号」。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CreditInfo {
    pub total_credit: i64,
    pub user_id: Option<i64>,
    pub user_name: String,
    pub vip_level: String,
}

/// 查余额（提交前预检 + 设置页显示）。
pub async fn user_credit(bin: &str, log: &Activity) -> AppResult<CreditInfo> {
    let bin = resolve_bin(bin)?;
    let v = extract_json(
        &run(
            vec![bin, "user_credit".to_string()],
            log,
            None,
            false,
            Timeout::Read,
        )
        .await?,
    )?;
    let total_credit = v
        .get("total_credit")
        .and_then(|x| x.as_i64())
        .ok_or_else(|| AppError::Internal("即梦 CLI 未返回 total_credit（可能未登录）".into()))?;
    Ok(CreditInfo {
        total_credit,
        user_id: v.get("user_id").and_then(|x| x.as_i64()),
        user_name: v
            .get("user_name")
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_string(),
        vip_level: v
            .get("vip_level")
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_string(),
    })
}

/// 即梦会话（`--session` 的可选值）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub id: i64,
    pub name: String,
    pub pinned: bool,
    pub updated_at: String,
}

/// 列出会话。
///
/// 会话是即梦那边组织生成历史的容器 —— 也就是用户说的「哪个通道」。原先设置页只给一个
/// 裸数字输入框，而「这个数字是哪条会话」在应用里根本无从得知。
pub async fn sessions(bin: &str, log: &Activity) -> AppResult<Vec<SessionInfo>> {
    let bin = resolve_bin(bin)?;
    let out = run(
        vec![
            bin,
            "session".into(),
            "list".into(),
            "-n".into(),
            "100".into(),
        ],
        log,
        None,
        false,
        Timeout::Read,
    )
    .await?;
    Ok(parse_sessions(&out))
}

/// 解析 `dreamina session list` 的表格输出。
///
/// 这个子命令打的是**给人看的表**而不是 JSON，所以只能按行拆。按列宽切是错的：
/// 会话名可以是中文，而表格是按**显示宽度**对齐的，按字节偏移切必然错位。
/// 故从两端认：首个 token 是 id，末两个 token 是日期与时间，倒数第三个是 PINNED，
/// 中间剩下的全是名字（名字里有空格也不会被拆散）。
pub fn parse_sessions(stdout: &str) -> Vec<SessionInfo> {
    let mut out = Vec::new();
    for line in stdout.lines() {
        let t: Vec<&str> = line.split_whitespace().collect();
        // 表头与分隔线：首列不是数字，天然被下面这个 parse 挡掉。
        if t.len() < 4 {
            continue;
        }
        let Ok(id) = t[0].parse::<i64>() else {
            continue;
        };
        let n = t.len();
        let updated_at = format!("{} {}", t[n - 2], t[n - 1]);
        let pinned = t[n - 3].eq_ignore_ascii_case("yes");
        let name = t[1..n - 3].join(" ");
        out.push(SessionInfo {
            id,
            name,
            pinned,
            updated_at,
        });
    }
    out
}

/// 一次提交的回执。
///
/// 原先 `submit()` 只从回体里挑走 `submit_id`，其余整个丢掉。2026-07-27 那次事故
/// 之后要复盘「提交当时即梦到底怎么答的」，发现库里一个字都没有 —— 于是把回执
/// 整体带出来，落进 `submit_credit` / `submit_status`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmitReceipt {
    pub submit_id: String,
    /// 提交回体里的 `gen_status`（正常是 `querying`）。
    pub gen_status: String,
    /// 提交回体里的 `credit_count`。**健康的提交当场就有它**（实测 8 / 44 两档）；
    /// 缺席是最早可得的异常信号。
    pub credit_count: Option<i64>,
}

impl SubmitReceipt {
    /// 回执是否看起来正常。用于**记日志**，不用于拒收 —— 见 `submit()` 的说明。
    pub fn looks_healthy(&self) -> bool {
        self.credit_count.is_some() && classify_status(&self.gen_status) != Outcome::Failed
    }
}

/// 两个形状的构造器 —— 生产代码只从 CLI 回体里解析回执，故只给测试用。
#[cfg(test)]
impl SubmitReceipt {
    /// 正常回执：`querying` + 有计费。
    pub fn healthy(submit_id: &str, credit: i64) -> Self {
        Self {
            submit_id: submit_id.to_string(),
            gen_status: "querying".into(),
            credit_count: Some(credit),
        }
    }

    /// 只有 submit_id 的回执 —— 2026-07-27 那 18 条幽灵单在提交这一刻的形状。
    pub fn bare(submit_id: &str) -> Self {
        Self {
            submit_id: submit_id.to_string(),
            gen_status: "querying".into(),
            credit_count: None,
        }
    }
}

/// 提交一条图生视频任务，返回回执。
///
/// **回执异常不在这里报错**。CLI skill 写的是「`gen_status` 为 fail 才算提交失败」，
/// 但本仓库的铁律更严：拿到 submit_id 就必须记下来（`额度不可撤回`）。一条既有
/// submit_id 又缺 `credit_count` 的回执，究竟是没扣费还是只是回体少给了一个字段，
/// 提交这一刻答不了 —— 而猜错的代价不对称：判它没扣费就丢掉 submit_id，万一扣了
/// 就是花钱买了个认不出主人的孤儿。所以这里照收，把判断交给轮询（`is_phantom`），
/// 那时有连续多轮的观测可用，比一次回体可靠得多。
pub async fn submit(
    bin: &str,
    image: &Path,
    prompt: &str,
    opts: &GenOpts,
    log: &Activity,
    who: Who<'_>,
) -> AppResult<SubmitReceipt> {
    if !image.is_file() {
        return Err(AppError::InvalidInput(format!(
            "首帧图不存在：{}",
            image.display()
        )));
    }
    let opts = normalize_opts(opts)?;
    let bin = resolve_bin(bin)?;
    let argv = command_line(&bin, &image.to_string_lossy(), prompt, &opts);
    let stdout = run(argv.clone(), log, who, true, Timeout::Submit).await?;
    let v = extract_json(&stdout)?;
    let submit_id = extract_submit_id(&v).ok_or_else(|| {
        AppError::Internal(format!(
            "提交成功但未能从返回里取到 submit_id。命令：{}",
            display_command(&argv)
        ))
    })?;
    Ok(SubmitReceipt {
        submit_id,
        gen_status: v
            .get("gen_status")
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_string(),
        credit_count: v.get("credit_count").and_then(|x| x.as_i64()),
    })
}

/// `list_task` 里的一条。
///
/// 载荷与 `query_result` 的形状**几乎一致**，故直接复用 [`parse_query`] —— 两个解析器
/// 迟早分叉，而分叉的表现是「同一个任务在两条路径上被读成两种状态」。
#[derive(Debug, Clone, PartialEq)]
pub struct TaskBrief {
    pub submit_id: String,
    pub q: QueryResult,
}

/// 一次 `list_task` 最多能拿回多少条。CLI 默认 20，我们按在跑条数翻页。
pub const LIST_PAGE: i64 = 100;

/// 列出账号下的任务（一次进程调用拿回**一整页**的状态）。
///
/// ## 为什么这是比「每条单查 + 退避」更好的机制
///
/// 即梦 CLI 没有任何推送（无 watch / stream / webhook / subscribe，`--poll=N` 只是把
/// 1 秒一次的轮询搬进子进程），所以「事件驱动」在这条链路上做不到；能做的是**把问一次
/// 的代价从 O(条数) 降到 O(1)**。原来 19 条在跑就要起 19 个 `query_result` 进程，
/// 退避只能把频率压下去（代价是出片延迟最长 10 分钟才被发现）。而 `list_task` 一次
/// 就把全部在跑任务的 `gen_status` / `credit_count` / `benefit_type` / 视频元数据一起
/// 给回来 —— 于是进程数与条数脱钩，频率反而可以**调高**，出片延迟从十分钟降到半分钟。
///
/// ## 两个字段这里拿不到，必须知道
///
/// - **没有 `queue_info`**（实测：排队中的条目只有 submit_id / prompt / gen_status /
///   fail_reason / commerce_info）。故幽灵单判定不能只靠这里的 `credit_count` 缺席 ——
///   要判死之前必须回落 `query_result` 拿队列位次确认（见 `runner::settle`）。
/// - **没有 `result_json.videos[].path`**：这里没下载动作。出片的条目仍要单发一次
///   `query_result --download_dir` 才能落盘。
pub async fn list_tasks(
    bin: &str,
    limit: i64,
    offset: i64,
    log: &Activity,
) -> AppResult<Vec<TaskBrief>> {
    let argv = vec![
        resolve_bin(bin)?,
        "list_task".to_string(),
        format!("--limit={limit}"),
        format!("--offset={offset}"),
    ];
    Ok(parse_list(&extract_json(
        &run(argv, log, None, false, Timeout::Read).await?,
    )?))
}

/// 解析 `list_task` 的 JSON 数组。非数组（CLI 改了输出）回空列表，由调用方回落单条查询。
pub fn parse_list(v: &serde_json::Value) -> Vec<TaskBrief> {
    v.as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    let submit_id = item
                        .get("submit_id")
                        .and_then(|x| x.as_str())
                        .filter(|s| !s.is_empty())?;
                    Some(TaskBrief {
                        submit_id: submit_id.to_string(),
                        q: parse_query(item),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 查一条任务；`download_dir` 非空则同时把成片下载到该目录。
pub async fn query(
    bin: &str,
    submit_id: &str,
    download_dir: Option<&Path>,
    log: &Activity,
    who: Who<'_>,
) -> AppResult<QueryResult> {
    let mut argv = vec![
        resolve_bin(bin)?,
        "query_result".to_string(),
        format!("--submit_id={submit_id}"),
    ];
    if let Some(d) = download_dir {
        argv.push(format!("--download_dir={}", d.to_string_lossy()));
    }
    // 带下载的那一次要把整个 mp4 拉下来，与只问一句状态不是一个量级；
    // 而两者都杀得起（成片还在即梦那边），故只是宽窄之分。
    let timeout = if download_dir.is_some() {
        Timeout::Download
    } else {
        Timeout::Read
    };
    Ok(parse_query(&extract_json(
        &run(argv, log, who, false, timeout).await?,
    )?))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即失败
mod tests {
    use super::*;

    /// 造一个可执行的假 CLI。
    fn fake_bin(dir: &Path, name: &str) -> std::path::PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, b"#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        p
    }

    // 填了绝对路径就只认它。
    #[test]
    fn resolve_bin_takes_explicit_path() {
        let td = tempfile::tempdir().unwrap();
        let p = fake_bin(td.path(), "dreamina");
        let got = resolve_bin(&p.to_string_lossy()).unwrap();
        assert_eq!(got, p.to_string_lossy());
    }

    // 填错的路径必须直说是哪个路径错了，**不能**偷偷回退到探测结果：
    // 那样用户在这台机器上「跑起来了」，换台机器又神秘失败，且错的那行还留在设置里。
    #[test]
    fn resolve_bin_rejects_bad_explicit_path_without_falling_back() {
        let td = tempfile::tempdir().unwrap();
        let dir = td.path().to_path_buf();
        fake_bin(&dir, "dreamina"); // 探测得到的话就会「意外成功」
        let bogus = dir.join("nope").join("dreamina");
        let err = resolve_bin(&bogus.to_string_lossy())
            .unwrap_err()
            .to_string();
        assert!(err.contains("nope"), "{err}");
    }

    // 留空 → 用默认名去搜索目录里探。
    //
    // 这条同时守住那个把整件事引爆的回归：设置里存了空串时，argv[0] 曾原样变成 ""，
    // 于是报错报成「找不到即梦 CLI「」」——一个连名字都没有的提示。
    #[test]
    fn resolve_bin_finds_default_name_when_blank() {
        let td = tempfile::tempdir().unwrap();
        let name = if cfg!(windows) {
            "dreamina.exe"
        } else {
            "dreamina"
        };
        let want = fake_bin(td.path(), name);
        let got = resolve_in("  ", &[td.path().to_path_buf()]).unwrap();
        assert_eq!(got, want.to_string_lossy());
    }

    // 裸名字（用户手打了 "dreamina"）与留空走同一条路。
    #[test]
    fn resolve_bin_finds_bare_name_in_search_dirs() {
        let td = tempfile::tempdir().unwrap();
        let name = if cfg!(windows) {
            "dreamina.exe"
        } else {
            "dreamina"
        };
        let want = fake_bin(td.path(), name);
        let got = resolve_in("dreamina", &[td.path().to_path_buf()]).unwrap();
        assert_eq!(got, want.to_string_lossy());
    }

    // 不可执行的同名文件不算数（否则会拿到一个必然 exec 失败的路径）。
    #[cfg(unix)]
    #[test]
    fn resolve_bin_skips_non_executable_file() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(td.path().join("dreamina"), b"not a program").unwrap();
        assert!(resolve_in("dreamina", &[td.path().to_path_buf()]).is_err());
    }

    // 探不到时，错误里要写清「找过哪儿」——这个错误的全部价值就在这一句。
    #[test]
    fn resolve_bin_error_lists_where_it_looked() {
        let err = resolve_bin("no-such-cli-anywhere").unwrap_err().to_string();
        assert!(err.contains("已找过 PATH 与"), "{err}");
        assert!(err.contains(".local/bin"), "{err}");
    }

    fn opts(m: Option<&str>, d: Option<i64>, r: Option<&str>) -> GenOpts {
        GenOpts {
            model_version: m.map(|s| s.to_string()),
            duration: d,
            video_resolution: r.map(|s| s.to_string()),
            session: None,
        }
    }

    // 三者全不给 → 走 CLI 默认路径，一个高级 flag 都不发。
    // 这是最稳的默认：不锁定模型名，CLI 改默认模型时我们跟着走。
    #[test]
    fn omitting_all_advanced_controls_sends_none() {
        let n = normalize_opts(&opts(None, None, None)).unwrap();
        let argv = command_line("dreamina", "/a.jpg", "提示词", &n);
        assert!(
            !argv.iter().any(|a| a.starts_with("--model_version")),
            "不该凭空发模型: {argv:?}"
        );
        assert!(!argv.iter().any(|a| a.starts_with("--duration")));
        assert!(!argv.iter().any(|a| a.starts_with("--video_resolution")));
    }

    // 半套组合是最容易踩的坑（CLI 只接受「都不给」或「一套完整组合」）。
    // 报错必须发生在花钱之前。
    #[test]
    fn partial_advanced_controls_are_rejected_before_spending() {
        let err = normalize_opts(&opts(None, Some(5), None)).unwrap_err();
        assert!(
            format!("{err}").contains("同时指定模型"),
            "半套组合须明确报错: {err}"
        );
        let err = normalize_opts(&opts(None, None, Some("1080p"))).unwrap_err();
        assert!(format!("{err}").contains("同时指定模型"));
    }

    // 给了模型就补齐三项：补出来的必须是该模型的合法值。
    #[test]
    fn model_only_is_filled_with_legal_defaults() {
        let n = normalize_opts(&opts(Some("seedance2.0fast"), None, None)).unwrap();
        assert_eq!(n.duration, Some(4), "补最短时长");
        assert_eq!(n.video_resolution.as_deref(), Some("720p"));
        let n = normalize_opts(&opts(Some("seedance1.0fast"), None, None)).unwrap();
        assert_eq!(
            n.duration,
            Some(5),
            "1.0 族最短是 5 秒（实测，非 -h 写的 3）"
        );
    }

    // 分辨率约束：只有 vip 支持 1080p/4k，其余一律 720p。
    // 提交前拦住，否则批量 20 条会连报 20 次同样的错、每次都走一趟网络。
    #[test]
    fn resolution_constraint_is_enforced_per_model() {
        assert!(normalize_opts(&opts(Some("seedance2.0_vip"), Some(5), Some("4k"))).is_ok());
        let err =
            normalize_opts(&opts(Some("seedance2.0fast"), Some(5), Some("1080p"))).unwrap_err();
        assert!(
            format!("{err}").contains("720p"),
            "须指出该模型只支持 720p: {err}"
        );
    }

    // 时长范围按模型族不同（1.0fast 5–10、1.5pro 5–12、2.0 族 4–15）。
    // 下界取**实测值**：CLI 的 -h 把 1.0fast 写成 3、1.5pro 写成 4，服务端两个都要 5。
    #[test]
    fn duration_range_is_enforced_per_model() {
        assert!(normalize_opts(&opts(Some("seedance1.5pro"), Some(12), None)).is_ok());
        let err = normalize_opts(&opts(Some("seedance1.5pro"), Some(15), None)).unwrap_err();
        assert!(format!("{err}").contains("5–12"), "{err}");
        let err = normalize_opts(&opts(Some("seedance1.0fast"), Some(2), None)).unwrap_err();
        assert!(format!("{err}").contains("5–10"), "{err}");
    }

    #[test]
    fn unknown_model_is_rejected() {
        let err = normalize_opts(&opts(Some("seedance9.9"), None, None)).unwrap_err();
        assert!(format!("{err}").contains("未知模型"), "{err}");
    }

    // 提示词整条作为一个 argv 元素：改写结果里必然有引号和换行，
    // 一旦经 shell 拼接就会在某条上被吃掉一半。
    #[test]
    fn prompt_is_a_single_argv_element_with_no_shell_escaping() {
        let tricky = "镜头缓推：\"焦点\"由近及远\n第二行 'single' $HOME `cmd`";
        let argv = command_line("dreamina", "/a.jpg", tricky, &opts(None, None, None));
        let found = argv
            .iter()
            .find(|a| a.starts_with("--prompt="))
            .expect("须有 --prompt");
        assert_eq!(
            found,
            &format!("--prompt={tricky}"),
            "提示词须原样进入单个 argv 元素，不做任何转义/截断"
        );
    }

    // 自建轮询器接管等待：必须显式关掉 CLI 的 --poll，
    // 否则状态活在一个随时会被杀掉的子进程里，而额度已经扣了。
    #[test]
    fn polling_is_delegated_to_our_own_poller() {
        let argv = command_line("dreamina", "/a.jpg", "p", &opts(None, None, None));
        assert!(
            argv.contains(&"--poll=0".to_string()),
            "须显式 --poll=0: {argv:?}"
        );
    }

    // 确认卡显示的命令行必须与即将执行的 argv 同源，只截断提示词。
    #[test]
    fn display_command_truncates_only_the_prompt() {
        let long: String = "很长的提示词".repeat(20);
        // 走 submit() 的真实路径：先 normalize 补齐三件套，再构造 argv。
        // 确认卡显示的必须是**最终**发出去的那串，含补出来的 --video_resolution。
        let n = normalize_opts(&opts(Some("seedance2.0fast"), Some(5), None)).unwrap();
        let argv = command_line("dreamina", "/a.jpg", &long, &n);
        let shown = display_command(&argv);
        assert!(shown.contains("--image=/a.jpg"), "路径须原样可见: {shown}");
        assert!(shown.contains("--model_version=seedance2.0fast"));
        assert!(shown.contains("--video_resolution=720p"));
        assert!(shown.contains('…'), "过长提示词须截断: {shown}");
        assert!(shown.len() < long.len(), "展示串不应比提示词还长");
    }

    // 真实 user_credit 输出。
    #[test]
    fn parses_real_user_credit_payload() {
        let v = extract_json(
            r#"{"total_credit": 13779,"user_id": 1989474043311592,"vip_level": "maestro"}"#,
        )
        .unwrap();
        assert_eq!(v.get("total_credit").unwrap().as_i64(), Some(13779));
    }

    // `session list` 打的是给人看的表，不是 JSON。真实输出（本机 CLI 抓的）。
    #[test]
    fn parses_real_session_table() {
        let raw = "ID  NAME     PINNED  UPDATED_AT\n\
                   --  -------  ------  ----------------\n\
                   0   default  Yes     2026-03-03 10:28\n";
        let s = parse_sessions(raw);
        assert_eq!(s.len(), 1, "表头与分隔线不得被当成数据行");
        assert_eq!(s[0].id, 0);
        assert_eq!(s[0].name, "default");
        assert!(s[0].pinned);
        assert_eq!(s[0].updated_at, "2026-03-03 10:28");
    }

    // 会话名可以带空格、也可以是中文。**不能按列宽切**：表格是按显示宽度对齐的，
    // 中文占两格而只有一个 char，按偏移切必然错位。故从两端认，中间全是名字。
    #[test]
    fn session_names_with_spaces_and_cjk_survive_parsing() {
        let raw = "ID   NAME              PINNED  UPDATED_AT\n\
                   ---  ----------------  ------  ----------------\n\
                   12   My Video Project  No      2026-07-01 09:05\n\
                   7    卡套 B-Roll 项目    Yes     2026-06-30 18:44\n";
        let s = parse_sessions(raw);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].name, "My Video Project");
        assert!(!s[0].pinned);
        assert_eq!(s[1].id, 7);
        assert_eq!(s[1].name, "卡套 B-Roll 项目");
        assert_eq!(s[1].updated_at, "2026-06-30 18:44");
    }

    // 认不出的行整行跳过，绝不 panic：这个子命令的输出格式随时可能变，
    // 而「会话列表读不出来」最坏只该是选择器空着，不该把设置页整页拖垮。
    #[test]
    fn unparsable_session_output_yields_empty_not_panic() {
        assert!(parse_sessions("").is_empty());
        assert!(parse_sessions("Error: not logged in\n").is_empty());
        assert!(parse_sessions("ID NAME\n").is_empty());
    }

    // 真实 query_result 输出（已下载）：只认落盘 path，不认会过期的签名 URL。
    #[test]
    fn parses_real_query_payload_and_ignores_signed_url() {
        let raw = r#"{
          "submit_id": "02c1cafe",
          "gen_status": "success",
          "result_json": { "images": [], "videos": [
            { "path": "/tmp/dl/02c1cafe_video_1.mp4", "video_url": "https://v3-artist.vlabvod.com/x?a=1",
              "fps": 24, "width": 960, "height": 960, "format": "mp4", "duration": 4.042 } ] },
          "credit_count": 44,
          "queue_info": { "queue_idx": 0, "queue_status": "Finish" }
        }"#;
        let q = parse_query(&extract_json(raw).unwrap());
        assert_eq!(q.gen_status, "success");
        assert_eq!(classify_status(&q.gen_status), Outcome::Done);
        assert_eq!(
            q.video_path.as_deref(),
            Some("/tmp/dl/02c1cafe_video_1.mp4")
        );
        assert_eq!(q.width, Some(960));
        assert_eq!(q.fps, Some(24.0));
        assert_eq!(q.duration_sec, Some(4.042));
        assert_eq!(q.credit_count, Some(44));
    }

    // **健康的排队回体**（2026-07-27 本机 CLI 抓的，`seedance2.0fast`，提交后 25 秒）。
    //
    // 这条推翻了此前那句「即梦排队期不回传位次」——那个结论是从 18 条坏单上归纳的。
    // 排队中位次与计费**都有**，这正是识别幽灵单的基线。
    // 「同时在跑的太多了」必须与真失败区分开：前者一分钱没扣、放回队列即可，
    // 后者要人去看原因。认 ret 码与英文文案两路 —— 数字是协议，文案会被改写。
    #[test]
    fn concurrency_reject_is_recognised_by_either_code_or_message() {
        // 2026-07-28 实测原文。
        assert!(is_concurrency_reject(
            "api error: ret=1310, message=ExceedConcurrencyLimit, logid=202607280301581921680310396A"
        ));
        assert!(is_concurrency_reject("ExceedConcurrencyLimit"));
        assert!(is_concurrency_reject("ret=1310"));
        assert!(is_concurrency_reject("Concurrency limit exceeded"));
        // 别的失败一律不许沾边：把它们也放回队列，等于让一条真坏掉的片子无限重投。
        for s in [
            "",
            "expired",
            "content policy violation",
            "api error: ret=1200, message=InternalError",
            "余额不足",
        ] {
            assert!(!is_concurrency_reject(s), "{s}");
        }
    }

    #[test]
    fn healthy_queued_payload_carries_position_and_credit() {
        let raw = r#"{
          "submit_id": "4584a328-11de-4edf-a541-cc6af0422915",
          "logid": "2026072711423919216803103916668AA",
          "gen_status": "querying",
          "credit_count": 8,
          "queue_info": {
            "queue_idx": 4485, "priority": 1,
            "queue_status": "Queueing", "queue_length": 574522
          }
        }"#;
        let q = parse_query(&extract_json(raw).unwrap());
        assert_eq!(classify_status(&q.gen_status), Outcome::Running);
        assert_eq!(q.queue_idx, Some(4485), "排队中就该有位次");
        assert_eq!(q.credit_count, Some(8), "排队中就该有计费回执");
        assert!(q.video_path.is_none());
    }

    // **幽灵单的真实回体**：只有四个字段。取样 `027e202c` 正是 2026-07-27 事故里
    // 18 条从未入队的其中一条 —— 与上面那条健康回体的差别就是判据本身。
    #[test]
    fn phantom_payload_has_neither_position_nor_credit() {
        let raw = r#"{
          "submit_id": "027e202c-7b5f-4fa0-99ef-dea5c6ab556f",
          "prompt": "首帧自然延续：窗台另一头趴着的那只猫极缓地呼吸……",
          "logid": "202607262322321921680310396825E37",
          "gen_status": "querying"
        }"#;
        let q = parse_query(&extract_json(raw).unwrap());
        assert_eq!(classify_status(&q.gen_status), Outcome::Running);
        assert!(q.queue_idx.is_none());
        assert!(q.credit_count.is_none(), "从未入队，故从未计费");
        assert!(q.video_path.is_none());
    }

    // 提交回执必须**整份**带出来，不能只挑走 submit_id：
    // 「这条到底计没计费」事后要靠它回答。真实提交回体形状。
    #[test]
    fn submit_receipt_keeps_status_and_credit() {
        let healthy = r#"{
          "submit_id": "4584a328-11de-4edf-a541-cc6af0422915",
          "logid": "2026072711423919216803103916668AA",
          "gen_status": "querying",
          "credit_count": 8
        }"#;
        let v = extract_json(healthy).unwrap();
        let r = SubmitReceipt {
            submit_id: extract_submit_id(&v).unwrap(),
            gen_status: v["gen_status"].as_str().unwrap_or_default().to_string(),
            credit_count: v["credit_count"].as_i64(),
        };
        assert_eq!(r.credit_count, Some(8));
        assert!(r.looks_healthy());

        // 没有 credit_count 的回执不算健康 —— 但它**依然带着 submit_id**，
        // 提交层照收（额度不可撤回），判死留给轮询。
        let bare = SubmitReceipt::bare("027e202c");
        assert!(!bare.looks_healthy());
        assert_eq!(bare.submit_id, "027e202c");
    }

    // 计费型号来自**回执**而不是我们的输入：`--model_version` 被上游忽略或降级时，
    // 输入侧一个字都不会变，只有 benefit_type 能证伪。真实 `list_task` 形状。
    #[test]
    fn billed_model_and_credit_come_from_the_receipt() {
        let raw = r#"{
          "submit_id": "02c1cafe",
          "gen_status": "success",
          "result_json": { "videos": [{ "fps": 24, "width": 960, "height": 960 }] },
          "commerce_info": {
            "credit_count": 44,
            "triplet": { "resource_type": "", "resource_id": "", "benefit_type": "" },
            "triplets": [{ "resource_type": "aigc", "resource_id": "generate_video",
                           "benefit_type": "dreamina_seedance_20_fast_5s" }]
          }
        }"#;
        let q = parse_query(&extract_json(raw).unwrap());
        assert_eq!(
            q.benefit_type.as_deref(),
            Some("dreamina_seedance_20_fast_5s")
        );
        assert_eq!(q.credit_count, Some(44), "commerce_info 里的扣费也要认");
    }

    // `list_task` 一次回一整页 —— 这是「用一个进程问到全部在跑任务」的全部依据。
    // 取样是 2026-07-27 实跑 `dreamina list_task --limit=3` 的回体形状。
    #[test]
    fn list_payload_yields_one_brief_per_task() {
        let raw = r#"[
          { "submit_id": "7278dbb1", "gen_task_type": "image2video", "gen_status": "success",
            "fail_reason": "",
            "result_json": { "images": [], "videos": [
              { "fps": 24, "width": 720, "height": 1280, "format": "mp4", "duration": 4.042 }] },
            "commerce_info": { "credit_count": 44,
              "triplets": [{ "benefit_type": "dreamina_seedance_20_fast_5s" }] } },
          { "submit_id": "c4a2fbe1", "gen_status": "querying", "fail_reason": "",
            "commerce_info": { "credit_count": 40,
              "triplets": [{ "benefit_type": "dreamina_video_seedance_15_pro" }] } }
        ]"#;
        let list = parse_list(&extract_json(raw).unwrap());
        assert_eq!(list.len(), 2);
        let done = &list[0];
        assert_eq!(done.submit_id, "7278dbb1");
        assert_eq!(classify_status(&done.q.gen_status), Outcome::Done);
        assert_eq!(done.q.width, Some(720));
        assert_eq!(done.q.duration_sec, Some(4.042));
        assert_eq!(done.q.credit_count, Some(44));
        assert_eq!(
            done.q.benefit_type.as_deref(),
            Some("dreamina_seedance_20_fast_5s")
        );
        // **出片条目在这里拿不到本地路径**（list_task 不做下载）。落盘仍须单发一次
        // `query_result --download_dir`，这条断言就是那个流程的存在理由。
        assert!(
            done.q.video_path.is_none(),
            "list_task 没有下载动作，不该出现本地路径"
        );

        let queued = &list[1];
        assert_eq!(classify_status(&queued.q.gen_status), Outcome::Running);
        assert_eq!(
            queued.q.credit_count,
            Some(40),
            "排队中的条目在 list_task 里也带计费 —— 它是「不是幽灵单」的证据"
        );
        assert!(
            queued.q.queue_idx.is_none(),
            "list_task 不回传 queue_info：幽灵判定不能只凭这一条路径"
        );
    }

    // CLI 哪天把输出换成对象/报错文本，解析要回空而不是 panic ——
    // 调用方据此回落到单条查询，而不是让整个轮询循环挂掉。
    #[test]
    fn non_array_list_payload_degrades_to_empty() {
        assert!(parse_list(&serde_json::json!({"error": "not logged in"})).is_empty());
        // 没有 submit_id 的条目直接跳过：认不出主人的状态对我们毫无用处。
        assert!(parse_list(&serde_json::json!([{"gen_status": "success"}])).is_empty());
    }

    // 单数 `triplet` 实测是空壳。空 benefit_type 不该被当成「型号是空字符串」显示出来。
    #[test]
    fn empty_receipt_fields_are_none_not_empty_strings() {
        let raw = r#"{"gen_status":"success","commerce_info":{"triplet":{"benefit_type":""}}}"#;
        let q = parse_query(&extract_json(raw).unwrap());
        assert!(q.benefit_type.is_none());
    }

    // 未下载时（没传 --download_dir）只有 video_url → video_path 必须为 None，
    // 否则会把一条几小时后必然 404 的签名 URL 当成成片路径存进库里。
    #[test]
    fn url_only_payload_yields_no_local_path() {
        let raw = r#"{"gen_status":"success","result_json":{"videos":[{"video_url":"https://x/y","width":720}]}}"#;
        let q = parse_query(&extract_json(raw).unwrap());
        assert!(q.video_path.is_none(), "签名 URL 不得当作本地路径");
        assert_eq!(q.width, Some(720));
    }

    // gen_status 大小写不统一（二进制里 Success/success/Expired/expired 都有）。
    #[test]
    fn status_classification_is_case_insensitive() {
        for s in ["success", "Success", "PartialSuccess", "partialsuccess"] {
            assert_eq!(classify_status(s), Outcome::Done, "{s}");
        }
        for s in ["expired", "Expired", "canceled", "failed"] {
            assert_eq!(classify_status(s), Outcome::Failed, "{s}");
        }
        for s in ["pending", "queue", "running", "querying"] {
            assert_eq!(classify_status(s), Outcome::Running, "{s}");
        }
    }

    // **未知态判 Running**：CLI 加新中间态时，判失败会把正在跑、额度已扣的任务标死。
    #[test]
    fn unknown_status_is_treated_as_running_not_failed() {
        assert_eq!(
            classify_status("some_new_intermediate_state"),
            Outcome::Running
        );
        assert_eq!(classify_status(""), Outcome::Running);
    }

    // submit_id 可能挪层级：递归查找让「字段搬了个家」不至于让整条链断掉。
    #[test]
    fn submit_id_is_found_at_any_depth() {
        for raw in [
            r#"{"submit_id":"abc-123"}"#,
            r#"{"data":{"submit_id":"abc-123"}}"#,
            r#"{"result":[{"task":{"submit_id":"abc-123"}}]}"#,
        ] {
            let v = extract_json(raw).unwrap();
            assert_eq!(extract_submit_id(&v).as_deref(), Some("abc-123"), "{raw}");
        }
        // 空串不算命中，否则会拿一个空 submit_id 去轮询到超时。
        let v = extract_json(r#"{"submit_id":""}"#).unwrap();
        assert!(extract_submit_id(&v).is_none());
    }

    // CLI 有时会在 JSON 前后打人类可读的行；容错到「首个 { 到末个 }」。
    #[test]
    fn extract_json_tolerates_surrounding_log_lines() {
        let raw = "[info] submitting...\n{\"gen_status\":\"queue\",\"n\":{\"a\":1}}\ndone\n";
        let v = extract_json(raw).unwrap();
        assert_eq!(v.get("gen_status").unwrap().as_str(), Some("queue"));
    }

    #[test]
    fn extract_json_reports_when_there_is_no_json() {
        let err = extract_json("command not found\n").unwrap_err();
        assert!(format!("{err}").contains("找不到 JSON"), "{err}");
    }

    // 受控模型清单是前端选择器与后端校验的共同契约：两侧都从 models() 取。
    #[test]
    fn model_list_matches_validation_table() {
        for m in models() {
            assert!(
                normalize_opts(&opts(Some(&m.model_version), None, None)).is_ok(),
                "清单里的模型 {} 必须能通过校验",
                m.model_version
            );
            assert!(!m.resolutions.is_empty());
            assert!(m.min_duration <= m.max_duration);
        }
    }

    // 2026-07-27 逐通道实测的回执单价，同一张首帧图各发一条。
    // 这组断言是价格表的**来源凭证**：改动 PRICES 必须先重测，不能顺手调数。
    #[test]
    fn measured_prices_reproduce_the_receipts() {
        for (model, res, dur, want) in [
            ("seedance2.0fast", "720p", 4, 8),
            ("seedance2.0fast", "720p", 5, 10),
            ("seedance2.0", "720p", 4, 12),
            ("seedance2.0mini", "720p", 4, 36),
            ("seedance2.0fast_vip", "720p", 4, 44),
            ("seedance2.0fast_vip", "720p", 5, 55),
            ("seedance2.0_vip", "720p", 4, 56),
            ("seedance2.0_vip", "4k", 4, 320),
            ("seedance1.0fast", "720p", 5, 10),
            ("seedance1.5pro", "720p", 5, 40),
        ] {
            assert_eq!(
                estimate_credits(model, res, dur),
                Some(want),
                "{model}/{res}/{dur}s 的实测计费是 {want}"
            );
        }
    }

    // 没测过的组合宁可说不知道：确认卡显示「未实测」，而不是一个像模像样的 0。
    #[test]
    fn unmeasured_combination_has_no_price() {
        assert_eq!(estimate_credits("seedance2.0_vip", "1080p", 4), None);
        assert_eq!(estimate_credits("seedance2.0fast", "4k", 4), None);
    }

    // CLI 帮助文本说 1.0fast 最短 3 秒、1.5pro 最短 4 秒，服务端两个都要 5 秒
    // （`ret=10001 invalid param:duration`）。这条测试锁住实测值，防止有人照着 -h 改回去。
    #[test]
    fn min_durations_follow_the_server_not_the_cli_help() {
        assert!(normalize_opts(&opts(Some("seedance1.0fast"), Some(3), None)).is_err());
        assert!(normalize_opts(&opts(Some("seedance1.0fast"), Some(5), None)).is_ok());
        assert!(normalize_opts(&opts(Some("seedance1.5pro"), Some(4), None)).is_err());
        assert!(normalize_opts(&opts(Some("seedance1.5pro"), Some(5), None)).is_ok());
    }

    // seedance1.0：服务端只收 1080p，CLI 本地又只许 2.0_vip 用 1080p —— 发不出去。
    // 留在清单里等于摆一个「选中必失败」的坑，故整条删除。
    #[test]
    fn unreachable_model_is_not_offered() {
        assert!(!models().iter().any(|m| m.model_version == "seedance1.0"));
        assert!(normalize_opts(&opts(Some("seedance1.0"), None, None)).is_err());
    }

    // 选择器上那行「≈N 额度/条」不能是空壳：每个在售模型都得有价。
    #[test]
    fn every_offered_model_shows_a_price() {
        for m in models() {
            assert!(
                m.credit_at_min.is_some_and(|c| c > 0),
                "{} 缺单价，选择器会显示成未实测",
                m.model_version
            );
        }
    }

    // 发给前端的单价切片必须与 `estimate_credits` 同源 —— 看板要在分节头上算
    // 「确认提交 18 条 · 预估 144 额度」，那个数一旦与确认卡上的对不上，两边都不可信了。
    #[test]
    fn res_prices_agree_with_estimate_credits() {
        for m in models() {
            for p in &m.res_prices {
                assert_eq!(
                    Some(p.credit_per_sec * m.min_duration),
                    estimate_credits(&m.model_version, &p.resolution, m.min_duration),
                    "{}/{} 的前端单价与后端预估必须同源",
                    m.model_version,
                    p.resolution
                );
            }
            // 未实测的组合**缺席**而不是给 0：界面据此标「≥」。
            for r in &m.resolutions {
                let listed = m.res_prices.iter().any(|p| &p.resolution == r);
                let priced = estimate_credits(&m.model_version, r, m.min_duration).is_some();
                assert_eq!(
                    listed, priced,
                    "{}/{r} 的「有没有价」两边须一致",
                    m.model_version
                );
            }
        }
    }

    // vip 标记是界面上那块琥珀色的依据（同规格贵 5.5 倍，买到的只是不排队）。
    #[test]
    fn vip_flag_matches_the_channel_suffix() {
        for m in models() {
            assert_eq!(m.vip, is_vip(&m.model_version), "{}", m.model_version);
        }
        assert!(models().iter().any(|m| m.vip), "清单里应当有 vip 通道");
        assert!(models().iter().any(|m| !m.vip), "清单里应当有非 vip 通道");
    }

    // ── 超时 ────────────────────────────────────────────────────────────────

    /// 造一个挂住不返回的假 CLI（`sleep`）。
    fn hanging_bin(dir: &Path) -> std::path::PathBuf {
        let p = dir.join("dreamina");
        std::fs::write(&p, b"#!/bin/sh\nsleep 30\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        p
    }

    // 这条测的是「卡住的 CLI 有个尽头」——在此之前 `Command::output()` 一直等，
    // 于是那条 IPC 永不返回、前端的重入锁永不释放，界面就那么停在原地。
    #[cfg(unix)]
    #[tokio::test]
    async fn hanging_cli_times_out_instead_of_blocking_forever() {
        let td = tempfile::tempdir().unwrap();
        let bin = hanging_bin(td.path()).to_string_lossy().to_string();
        let log = Activity::silent();
        let started = std::time::Instant::now();
        let err = run(
            vec![bin, "user_credit".into()],
            &log,
            None,
            false,
            Timeout::Custom(std::time::Duration::from_millis(300)),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, AppError::Timeout(_)),
            "超时必须是自己的分类，调用方要靠它区分「没做成」与「不知道做没做成」：{err}"
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(10),
            "该在超时那一刻返回，而不是等子进程自己跑完"
        );
        // 超时也要留下痕迹：打包后的应用没有终端，日志面板是唯一的答案来源。
        let snap = log.snapshot();
        assert!(
            snap.iter().any(|e| e.level == "error" && e.phase == "cli"),
            "超时须以 error 级进执行日志：{snap:?}"
        );
    }

    // 提交那一档的文案**必须写明钱可能已经花出去了**。它是人此刻唯一的线索，
    // 而下一步（直接重跑 还是 先去核对）取决于它 —— 猜错就是再花一份钱。
    #[test]
    fn submit_timeout_message_warns_about_money() {
        let msg = Timeout::Submit.message(1234);
        assert!(msg.contains("扣"), "{msg}");
        assert!(msg.contains("核对"), "{msg}");
        assert!(msg.contains("submit_id"), "{msg}");
        // 只读那两档相反：必须说清杀掉它没有副作用，否则人会以为出了大事。
        for t in [Timeout::Read, Timeout::Download] {
            let m = t.message(1);
            assert!(!m.contains("扣了费"), "{m}");
        }
    }

    // 档位的顺序就是「杀掉它的代价」的顺序，反了就会把提交按只读的紧度掐掉。
    #[test]
    fn timeout_tiers_are_ordered_by_cost_of_killing() {
        assert!(Timeout::Read.duration() < Timeout::Download.duration());
        assert!(Timeout::Download.duration() < Timeout::Submit.duration());
    }
}
