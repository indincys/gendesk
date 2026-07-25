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

/// 默认可执行名。设置里留空即用它，并走 [`resolve_bin`] 自动探测。
pub const DEFAULT_BIN: &str = "dreamina";

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

/// 即梦模型族的取值与约束。**只作提交前的本地预检**，最终真相仍是 CLI 自己的 `-h`。
///
/// 预检的价值在于「花钱之前拦住」：组合不合法时 CLI 会拒，但那时已经走了一趟网络，
/// 而批量提交 20 条会连报 20 次同样的错。
const MODELS: &[(&str, i64, i64, &[&str])] = &[
    // (model_version, 最短时长, 最长时长, 允许的分辨率)
    ("seedance1.0fast", 3, 10, &["720p"]),
    ("seedance1.0", 3, 10, &["720p"]),
    ("seedance1.5pro", 4, 12, &["720p"]),
    ("seedance2.0", 4, 15, &["720p"]),
    ("seedance2.0fast", 4, 15, &["720p"]),
    ("seedance2.0_vip", 4, 15, &["720p", "1080p", "4k"]),
    ("seedance2.0fast_vip", 4, 15, &["720p"]),
    ("seedance2.0mini", 4, 15, &["720p"]),
];

/// 受控模型清单（前端选择器渲染源）。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub model_version: String,
    pub min_duration: i64,
    pub max_duration: i64,
    pub resolutions: Vec<String>,
}

pub fn models() -> Vec<ModelInfo> {
    MODELS
        .iter()
        .map(|(m, lo, hi, res)| ModelInfo {
            model_version: (*m).to_string(),
            min_duration: *lo,
            max_duration: *hi,
            resolutions: res.iter().map(|r| (*r).to_string()).collect(),
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
    /// 队列位次（伪进度用）。
    pub queue_idx: Option<i64>,
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
        credit_count: v.get("credit_count").and_then(|x| x.as_i64()),
        queue_idx: v
            .get("queue_info")
            .and_then(|q| q.get("queue_idx"))
            .and_then(|x| x.as_i64()),
    }
}

/// 跑一条 CLI 命令，回 stdout。
///
/// 在 `spawn_blocking` 里同步 exec：CLI 一次调用要走网络，秒级到十几秒，
/// 占着异步执行器会把别的 IPC 命令一起卡住（v0.14.0 的上传静默就是这么来的）。
async fn run(argv: Vec<String>) -> AppResult<String> {
    let pretty = display_command(&argv);
    let out = tokio::task::spawn_blocking(move || {
        let (bin, args) = argv.split_first().ok_or_else(|| {
            AppError::Internal("命令行为空（不应发生：command_line 至少给出 bin）".into())
        })?;
        std::process::Command::new(bin)
            .args(args)
            .output()
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => AppError::InvalidInput(format!(
                    "找不到即梦 CLI「{bin}」。请先安装并 `dreamina login`，或在设置里填它的绝对路径。"
                )),
                _ => AppError::Io(format!("启动即梦 CLI 失败：{e}")),
            })
    })
    .await
    .map_err(|e| AppError::Internal(format!("即梦 CLI 任务panic：{e}")))??;

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        // stdout 也带进去：CLI 的业务错误（额度不足、需网页授权）常打在 stdout。
        let detail: String = format!("{stderr}{stdout}").chars().take(400).collect();
        return Err(AppError::Internal(format!(
            "即梦 CLI 返回失败（{}）：{}\n命令：{pretty}",
            out.status
                .code()
                .map_or_else(|| "signal".to_string(), |c| c.to_string()),
            detail.trim()
        )));
    }
    Ok(stdout)
}

/// 查余额（提交前预检 + 设置页显示）。
pub async fn user_credit(bin: &str) -> AppResult<i64> {
    let bin = resolve_bin(bin)?;
    let v = extract_json(&run(vec![bin, "user_credit".to_string()]).await?)?;
    v.get("total_credit")
        .and_then(|x| x.as_i64())
        .ok_or_else(|| AppError::Internal("即梦 CLI 未返回 total_credit（可能未登录）".into()))
}

/// 提交一条图生视频任务，返回 submit_id。
pub async fn submit(bin: &str, image: &Path, prompt: &str, opts: &GenOpts) -> AppResult<String> {
    if !image.is_file() {
        return Err(AppError::InvalidInput(format!(
            "首帧图不存在：{}",
            image.display()
        )));
    }
    let opts = normalize_opts(opts)?;
    let bin = resolve_bin(bin)?;
    let argv = command_line(&bin, &image.to_string_lossy(), prompt, &opts);
    let stdout = run(argv.clone()).await?;
    extract_submit_id(&extract_json(&stdout)?).ok_or_else(|| {
        AppError::Internal(format!(
            "提交成功但未能从返回里取到 submit_id。命令：{}",
            display_command(&argv)
        ))
    })
}

/// 查一条任务；`download_dir` 非空则同时把成片下载到该目录。
pub async fn query(
    bin: &str,
    submit_id: &str,
    download_dir: Option<&Path>,
) -> AppResult<QueryResult> {
    let mut argv = vec![
        resolve_bin(bin)?,
        "query_result".to_string(),
        format!("--submit_id={submit_id}"),
    ];
    if let Some(d) = download_dir {
        argv.push(format!("--download_dir={}", d.to_string_lossy()));
    }
    Ok(parse_query(&extract_json(&run(argv).await?)?))
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
        let n = normalize_opts(&opts(Some("seedance1.0"), None, None)).unwrap();
        assert_eq!(n.duration, Some(3), "1.0 族最短是 3 秒");
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

    // 时长范围按模型族不同（1.0 族 3–10、1.5pro 4–12、2.0 族 4–15）。
    #[test]
    fn duration_range_is_enforced_per_model() {
        assert!(normalize_opts(&opts(Some("seedance1.5pro"), Some(12), None)).is_ok());
        let err = normalize_opts(&opts(Some("seedance1.5pro"), Some(15), None)).unwrap_err();
        assert!(format!("{err}").contains("4–12"), "{err}");
        let err = normalize_opts(&opts(Some("seedance1.0fast"), Some(2), None)).unwrap_err();
        assert!(format!("{err}").contains("3–10"), "{err}");
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
}
