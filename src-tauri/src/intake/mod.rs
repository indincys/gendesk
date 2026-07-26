//! 生图工单收件 —— Claude Code / Codex 侧 skill 投单，GenDesk 自动导入并开跑。
//!
//! ```text
//! <交接根>/生图/收件/<工单目录>/
//!     提示词.txt        ← 标准入口：组头带挂靠与参数，正文一段一条
//!     images/*.jpg
//!     说明.md           ← 可选，给人看的方向地图 / 可替换变量表
//!     READY.txt         ← 最后写；没有它的目录一律跳过（skill 可能还在写）
//!     job.json          ← 可选逃生舱，见下
//! <交接根>/生图/_已收录/<工单目录>-<ts>/   ← 成功后移档，内含 结果.txt
//! ```
//!
//! ## 为什么是文件而不是本地端口 / MCP 直连
//!
//! 「业务真相只在 Rust」与「单写者事务」两条铁律直接排除了外部进程写库。而 HTTP
//! 端口还有一个更实际的问题：**GenDesk 没开的时候投单会直接失败**。文件不会——
//! 启动时补跑一次扫描，昨晚投的单今天照样进得来。
//!
//! ## 为什么参数写在 txt 的组头里，而不是一份 job.json
//!
//! 因为 `参考图:` 写在组头里是**位置绑定**——它属于紧跟其后的那个组，不引用组名。
//! job.json 那种 `{"image":"a.jpg","group":"楼道骑行"}` 是**按名引用**：组名一改，
//! 挂靠当场断掉，而且断得很安静（整批图配错提示词，要到验收时才看得出来）。
//!
//! 单一产物还消灭了「txt 与 json 各存一半真相、改一处漏一处」这个老问题
//! （v0.13.0 的包内 ledger、v0.15.0 的两处真相，都是这个形状）。
//!
//! `job.json` 保留为逃生舱：结构化程度更高的调用方（或需要内联提示词的场景）仍可用它。
//!
//! ## 三条设计决定
//!
//! **1. 收录恰好一次，靠库不靠目录。** 见 0023 的表注释：工单目录会被移动、重建、
//! 手动整理，磁盘上的标记不可靠，而重复收录 = 重复建批 = 重复花钱。
//!
//! **2. 校验与阈值都发生在写任何东西之前。** 参数非法、图片缺失、超阈值——这些
//! 在「一张图都还没进库」的时候就拦下，工单要么整份生效要么整份没发生。
//!
//! **3. 参数只归一化拼法，不归一化取值。** `jpg → jpeg`、全角冒号 → 半角是拼法；
//! 而「1080x1920 边长不是 16 的倍数」是取值问题，**拒单而不是替用户改成 1088**。
//! 静默改值正是「我明明写了 9:16，出来的却不是」这类问题的成因（v0.15.2）。

pub mod ingest;
pub mod watcher;

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::importer::{self, GroupOrigin, ParsedGroup, ParsedPrompt};
use crate::provider::GenParams;

/// 交接根下的固定子目录名。skill 侧写死这几个名字，故它们是契约，不可改。
pub const INTAKE: &str = "生图";
pub const PENDING: &str = "收件";
pub const CONSUMED: &str = "_已收录";
pub const JOB: &str = "job.json";
pub const READY: &str = "READY.txt";
/// 标准提示词文档名。找不到它时退而在目录里找唯一的 `.txt`/`.md`。
pub const DOC: &str = "提示词.txt";
pub const IMAGES: &str = "images";

/// 回执文件：让 Claude Code / Codex 侧能回读结果，而不必猜后面发生了什么。
pub const ERROR_FILE: &str = "错误.txt";
pub const HOLD_FILE: &str = "待确认.txt";
pub const RESULT_FILE: &str = "结果.txt";
/// **确认的唯一表达**。设置页那个按钮做的事就是替你写下这个文件。
pub const CONFIRM_FILE: &str = "确认.txt";

/// 抽卡次数上限（与 `engine::create_batch` 的夹取一致）。
const MAX_DRAWS: i64 = 5;

/// 默认收件根 = 视频流水线那个交接根。
///
/// **刻意共用一个根**：用户只需要知道一个目录、只需要配一次；两个 agent 侧 skill
/// 各写各的子目录，互不打扰。
pub fn default_root() -> PathBuf {
    crate::v2v::handoff::default_root()
}

/// `<root>/生图/收件`
pub fn pending_dir(root: &Path) -> PathBuf {
    root.join(INTAKE).join(PENDING)
}

/// `<root>/生图/_已收录`
pub fn consumed_dir(root: &Path) -> PathBuf {
    root.join(INTAKE).join(CONSUMED)
}

// ───────────────────────────── job.json（逃生舱） ─────────────────────────────

/// 一份工单的结构化描述。标准路径不需要它——`提示词.txt` 的组头已经表达了全部信息。
///
/// **入参一律宽容**：camelCase 与 snake_case 都收，缺省项都有合理默认。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct JobSpec {
    /// 工单标识（去重键）。缺省取工单目录名。
    #[serde(alias = "job_id", alias = "id")]
    pub job_id: Option<String>,
    /// 批次备注名，落到 `batches.note`，任务页可见。
    pub note: Option<String>,
    /// 提示词分组。每项要么给 `file`（txt），要么给 `prompts`（内联）。
    pub groups: Vec<GroupSpec>,
    /// 参考图挂靠（按组名引用）。**仅 job.json 路径有**；txt 路径走组头的位置绑定。
    pub refs: Vec<RefSpec>,
    /// 工单级默认参数，组头没写时用它。
    pub params: ParamsSpec,
    /// 参考图入库分组名（图库目录，`ref_groups`）。缺省不分组。
    #[serde(alias = "ref_group")]
    pub ref_group: Option<String>,
    /// 参考图是否只作本批附件（不进长期图库）。缺省 false。
    pub ephemeral: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GroupSpec {
    pub name: Option<String>,
    /// 编号前缀（如 `BR`）。缺省由组名生成；与库内已有同前缀分组会自动**追加**。
    pub prefix: Option<String>,
    pub scene: Option<String>,
    pub tags: Vec<String>,
    /// 受控用途（当前只有「图生视频」）。取值非法直接拒单，同命令边界的口径。
    pub purposes: Vec<String>,
    /// 工单目录内相对路径的 txt。
    pub file: Option<String>,
    /// 内联提示词正文。
    pub prompts: Vec<String>,
    /// 组级参数（覆盖工单级）。
    pub params: ParamsSpec,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RefSpec {
    #[serde(alias = "path", alias = "file")]
    pub image: String,
    /// 挂靠到哪个组（组名）。只有一个组时可省。
    pub group: Option<String>,
}

/// 生成参数。**只有这三项会发到远端**（v0.15.2），加上恒定的 `n=1`。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ParamsSpec {
    #[serde(alias = "aspect_ratio", alias = "ratio")]
    pub aspect_ratio: Option<String>,
    pub size: Option<String>,
    #[serde(alias = "output_format", alias = "format")]
    pub output_format: Option<String>,
    /// 抽卡次数 k：每个组合独立生成 k 次（= k 个任务，不是发 `n=k`）。
    pub draws: Option<i64>,
    #[serde(alias = "clear_ai_metadata")]
    pub clear_ai_metadata: Option<bool>,
    #[serde(alias = "remove_c2pa")]
    pub remove_c2pa: Option<bool>,
}

impl ParamsSpec {
    /// 组级覆盖工单级：组头写了就用组头的，没写就继承工单默认。
    fn overlay(&self, base: &ParamsSpec) -> ParamsSpec {
        let pick = |a: &Option<String>, b: &Option<String>| a.clone().or_else(|| b.clone());
        ParamsSpec {
            aspect_ratio: pick(&self.aspect_ratio, &base.aspect_ratio),
            size: pick(&self.size, &base.size),
            output_format: pick(&self.output_format, &base.output_format),
            draws: self.draws.or(base.draws),
            clear_ai_metadata: self.clear_ai_metadata.or(base.clear_ai_metadata),
            remove_c2pa: self.remove_c2pa.or(base.remove_c2pa),
        }
    }
}

// ───────────────────────────── 校验产物 ─────────────────────────────

/// 校验通过的工单：此后每一步都不必再判「万一没有呢」。
#[derive(Debug, Clone)]
pub struct Plan {
    pub job_id: String,
    pub dir_name: String,
    pub dir: PathBuf,
    pub note: Option<String>,
    pub groups: Vec<PlannedGroup>,
    pub ref_group: Option<String>,
    pub ephemeral: bool,
}

/// 一个已解析、已配好图、已定好参数的组。
#[derive(Debug, Clone)]
pub struct PlannedGroup {
    pub parsed: ParsedGroup,
    /// 挂靠到本组的参考图（绝对路径，已确认存在）。**位置绑定，不按组名引用。**
    pub refs: Vec<PathBuf>,
    /// 本组生效的批次参数快照（与生成页 `buildParamsJson` 同形）。
    pub params_json: String,
    /// 实际会进 multipart 的字段。
    pub wire_json: String,
    pub draws: i64,
}

impl PlannedGroup {
    /// 本组会产生多少个任务 = 多少张图（`n` 恒为 1）。
    pub fn task_count(&self) -> i64 {
        self.parsed.prompts.len() as i64 * self.refs.len() as i64 * self.draws
    }
    /// 分桶键：参数相同的组并进同一个批次。
    pub fn bucket(&self) -> (String, i64) {
        (self.params_json.clone(), self.draws)
    }
}

impl Plan {
    pub fn task_count(&self) -> i64 {
        self.groups.iter().map(PlannedGroup::task_count).sum()
    }
    /// 会建出几个批次（按参数分桶后的桶数）。
    pub fn batch_count(&self) -> usize {
        let mut seen: Vec<(String, i64)> = Vec::new();
        for g in &self.groups {
            let b = g.bucket();
            if !seen.contains(&b) {
                seen.push(b);
            }
        }
        seen.len()
    }
}

// ───────────────────────────── 参数归一化 ─────────────────────────────

/// 参数快照 + wire 记录 + 抽卡次数。
///
/// 这是本模块最要紧的一段：用户点名的关切是「尺寸比例等参数能正确传递到生成端」。
/// 保证由两件事给出——(1) 产出的快照与生成页 `buildParamsJson` **同形**，走的是
/// 下游同一条解析；(2) 立刻过 `GenParams::parse_checked`，非法当场拒单。
pub fn build_params(p: &ParamsSpec) -> Result<(String, String, i64), String> {
    let norm = |s: &Option<String>| -> Option<String> {
        s.as_deref()
            .map(|v| v.trim().replace('：', ":"))
            .filter(|v| !v.is_empty())
    };
    let aspect_ratio = norm(&p.aspect_ratio);
    // 只写了「比例:」没写「尺寸:」时补上配套尺寸：实测单发 aspect_ratio 会回整批正方形
    // （见 `provider::RATIO_SIZES`）。这是**补一个缺失字段**，不是改用户写下的取值——
    // 他若自己写了尺寸，哪怕与比例不符也照发，归一化只管拼法不管取值。
    let size = norm(&p.size).map(|s| s.to_lowercase()).or_else(|| {
        aspect_ratio
            .as_deref()
            .and_then(crate::provider::companion_size)
            .map(str::to_string)
    });
    // `jpg` 是人（和模型）最常写的拼法，端点只认 `jpeg`。归一化拼法不改变取值。
    let output_format = norm(&p.output_format).map(|f| match f.to_lowercase().as_str() {
        "jpg" => "jpeg".to_string(),
        other => other.to_string(),
    });

    let draws = p.draws.unwrap_or(1);
    if !(1..=MAX_DRAWS).contains(&draws) {
        // **不夹取**：组头写了 9 却悄悄跑 5，任务数对不上而没人知道为什么
        // （v0.14.0 抽卡不进快照那个坑的同一种形状）。
        return Err(format!("抽卡次数「{draws}」超出范围，应为 1~{MAX_DRAWS}"));
    }

    let mut snap = serde_json::Map::new();
    let mut wire = serde_json::Map::new();
    for (wire_key, snap_key, value) in [
        ("aspect_ratio", "aspectRatio", aspect_ratio),
        ("size", "size", size),
        ("output_format", "outputFormat", output_format),
    ] {
        if let Some(v) = value {
            snap.insert(snap_key.into(), serde_json::Value::String(v.clone()));
            wire.insert(wire_key.into(), serde_json::Value::String(v));
        }
    }
    // 本地输出处理开关（不发远端）：显式写入快照，「按此配置再来一批」才还原得回来。
    snap.insert(
        "clearAiMetadata".into(),
        serde_json::Value::Bool(p.clear_ai_metadata.unwrap_or(true)),
    );
    snap.insert(
        "removeC2pa".into(),
        serde_json::Value::Bool(p.remove_c2pa.unwrap_or(true)),
    );
    snap.insert("draws".into(), serde_json::Value::from(draws));
    // n 恒为 1：抽卡 k 次在引擎侧展开成 k 个任务，不是发 n=k。
    wire.insert("n".into(), serde_json::Value::from(1));

    let params_json = serde_json::Value::Object(snap).to_string();
    // 花钱之前的本地预检，与生成页 `create_batch` 同一个函数。
    GenParams::parse_checked(&params_json)?;
    Ok((
        params_json,
        serde_json::Value::Object(wire).to_string(),
        draws,
    ))
}

// ───────────────────────────── 路径安全 ─────────────────────────────

/// 工单目录内的相对路径拼接。
///
/// 工单是**外部输入**：绝对路径与 `..` 一律拒绝，否则一句 `"../../../.ssh/id_rsa"`
/// 就能让我们把任意文件拷进图库（`publish::paths::RelPath` 出于同样理由剔 `..`）。
pub fn safe_join(dir: &Path, rel: &str) -> Result<PathBuf, String> {
    let rel = rel.trim();
    if rel.is_empty() {
        return Err("路径为空".into());
    }
    let p = Path::new(rel);
    for c in p.components() {
        match c {
            Component::Normal(_) | Component::CurDir => {}
            _ => return Err(format!("路径「{rel}」必须是工单目录内的相对路径")),
        }
    }
    Ok(dir.join(p))
}

// ───────────────────────────── 校验 ─────────────────────────────

/// 工单目录 → `Plan`。任何一处不对都返回**人能照着改**的错误信息。
///
/// 只读文件、不写任何东西：整份校验通过之前，库里一个字节都不会变。
pub fn plan(dir: &Path, dir_name: &str) -> Result<Plan, String> {
    let job_path = dir.join(JOB);
    if job_path.is_file() {
        let raw =
            std::fs::read_to_string(&job_path).map_err(|e| format!("读不到 job.json：{e}"))?;
        let spec: JobSpec =
            serde_json::from_str(&raw).map_err(|e| format!("job.json 不是合法 JSON：{e}"))?;
        return plan_from_spec(dir, dir_name, &spec);
    }
    plan_from_doc(dir, dir_name)
}

/// 标准路径：从 `提示词.txt` 的组头读挂靠与参数。
fn plan_from_doc(dir: &Path, dir_name: &str) -> Result<Plan, String> {
    let doc = find_doc(dir)?;
    let bytes = std::fs::read(&doc).map_err(|e| format!("读不到提示词文档：{e}"))?;
    let stem = doc.file_stem().map(|s| s.to_string_lossy().to_string());
    let parsed = importer::parse_named(&bytes, stem.as_deref());
    if parsed.groups.is_empty() {
        return Err("提示词文档里没解析出任何提示词".into());
    }

    let single = parsed.groups.len() == 1;
    let mut groups = Vec::with_capacity(parsed.groups.len());
    for g in parsed.groups {
        // 组头没写 `参考图:` 时：单组工单可以吃 images/ 下全部图；多组必须点名。
        // **绝不按顺序猜**——猜错的代价是整批图配错提示词，而那要到验收时才看得出来。
        let refs = if g.refs.is_empty() {
            if !single {
                return Err(format!(
                    "分组「{}」没写 `参考图:`；工单里有多个分组时每组都必须点名挂靠",
                    g.name
                ));
            }
            list_images(dir)?
        } else {
            let mut out = Vec::with_capacity(g.refs.len());
            for r in &g.refs {
                let p = safe_join(dir, r)?;
                if !p.is_file() {
                    return Err(format!("分组「{}」的参考图不存在：{r}", g.name));
                }
                out.push(p);
            }
            out
        };
        if refs.is_empty() {
            return Err(format!("分组「{}」没有可用的参考图", g.name));
        }
        let spec = ParamsSpec {
            aspect_ratio: g.ratio.clone(),
            size: g.size.clone(),
            output_format: g.format.clone(),
            draws: g.draws,
            ..Default::default()
        };
        let (params_json, wire_json, draws) =
            build_params(&spec).map_err(|e| format!("分组「{}」的参数有问题：{e}", g.name))?;
        groups.push(PlannedGroup {
            parsed: g,
            refs,
            params_json,
            wire_json,
            draws,
        });
    }

    Ok(Plan {
        job_id: dir_name.to_string(),
        dir_name: dir_name.to_string(),
        dir: dir.to_path_buf(),
        note: None,
        groups,
        ref_group: None,
        ephemeral: false,
    })
}

/// 找提示词文档：`提示词.txt` 优先；否则目录下唯一的 `.txt`/`.md`（`说明.md` 除外）。
fn find_doc(dir: &Path) -> Result<PathBuf, String> {
    let std_doc = dir.join(DOC);
    if std_doc.is_file() {
        return Ok(std_doc);
    }
    let mut cands: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("读不到工单目录：{e}"))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let name = p.file_name().map(|s| s.to_string_lossy().to_string());
            // READY / 回执文件不是提示词文档。
            let excluded = matches!(
                name.as_deref(),
                Some(READY)
                    | Some(ERROR_FILE)
                    | Some(HOLD_FILE)
                    | Some(RESULT_FILE)
                    | Some(CONFIRM_FILE)
                    | Some("说明.md")
            );
            p.is_file()
                && !excluded
                && p.extension()
                    .and_then(|s| s.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case("txt") || e.eq_ignore_ascii_case("md"))
        })
        .collect();
    cands.sort();
    match cands.len() {
        0 => Err(format!(
            "工单目录里没有提示词文档（找 `{DOC}`，或目录下唯一的 .txt/.md）"
        )),
        1 => Ok(cands.remove(0)),
        _ => Err(format!(
            "工单目录里有多个 .txt/.md，认不出哪个是提示词文档；把它命名为 `{DOC}`"
        )),
    }
}

/// `images/` 下的全部图片（排序保证稳定）。
fn list_images(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let img_dir = dir.join(IMAGES);
    let mut out: Vec<PathBuf> = std::fs::read_dir(&img_dir)
        .map_err(|_| format!("工单目录里没有 `{IMAGES}/`，也没在组头写 `参考图:`"))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.extension().and_then(|s| s.to_str()).is_some_and(|e| {
                    matches!(
                        e.to_lowercase().as_str(),
                        "jpg" | "jpeg" | "png" | "webp" | "bmp"
                    )
                })
        })
        .collect();
    out.sort();
    Ok(out)
}

/// 逃生舱路径：从 job.json 构造。
fn plan_from_spec(dir: &Path, dir_name: &str, spec: &JobSpec) -> Result<Plan, String> {
    let job_id = spec
        .job_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(dir_name)
        .to_string();
    if spec.groups.is_empty() {
        return Err("job.json 里没有 groups：至少要有一个提示词分组".into());
    }

    // 先把每个 GroupSpec 展开成解析器口径的分组（txt 可能拆出多个）。
    let mut flat: Vec<(ParsedGroup, ParamsSpec)> = Vec::new();
    for (i, g) in spec.groups.iter().enumerate() {
        let params = g.params.overlay(&spec.params);
        let mut extra_tags = g.tags.clone();
        for p in &g.purposes {
            if !crate::purpose::is_purpose(p) {
                return Err(format!(
                    "未知用途「{p}」，可用：{}",
                    crate::purpose::all()
                        .into_iter()
                        .map(|x| x.tag)
                        .collect::<Vec<_>>()
                        .join(" / ")
                ));
            }
            if !extra_tags.contains(p) {
                extra_tags.push(p.clone());
            }
        }
        match (&g.file, g.prompts.is_empty()) {
            (Some(f), _) => {
                let path = safe_join(dir, f)?;
                let bytes =
                    std::fs::read(&path).map_err(|e| format!("读不到提示词文件「{f}」：{e}"))?;
                let stem = path.file_stem().map(|s| s.to_string_lossy().to_string());
                for mut pg in importer::parse_named(&bytes, stem.as_deref()).groups {
                    if let Some(n) = g.name.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                        // 只有解析出单个组时才让 job.json 的组名覆盖它。
                        if spec.groups.len() == 1 {
                            pg.name = n.to_string();
                        }
                    }
                    for t in &extra_tags {
                        if !pg.tags.contains(t) {
                            pg.tags.push(t.clone());
                        }
                    }
                    if pg.prefix.is_none() {
                        pg.prefix = g.prefix.clone();
                    }
                    flat.push((pg, params.clone()));
                }
            }
            (None, false) => {
                let name = g
                    .name
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| format!("groups[{i}] 用了内联 prompts，必须同时给 name"))?;
                let prompts: Vec<ParsedPrompt> = g
                    .prompts
                    .iter()
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .map(|s| ParsedPrompt {
                        title: None,
                        text: s.to_string(),
                    })
                    .collect();
                if prompts.is_empty() {
                    return Err(format!("groups[{i}]「{name}」的 prompts 全是空串"));
                }
                flat.push((
                    ParsedGroup {
                        name: name.to_string(),
                        prefix: g.prefix.clone(),
                        scene: g.scene.clone().unwrap_or_default(),
                        tags: extra_tags,
                        prompts,
                        origin: GroupOrigin::Explicit,
                        ..Default::default()
                    },
                    params,
                ));
            }
            (None, true) => return Err(format!("groups[{i}] 既没有 file 也没有 prompts")),
        }
    }

    // 挂靠：job.json 用组名引用（逃生舱的代价），单组时可省。
    if spec.refs.is_empty() {
        return Err("job.json 里没有 refs：至少要有一张参考图及其挂靠".into());
    }
    let mut by_group: Vec<Vec<PathBuf>> = vec![Vec::new(); flat.len()];
    for (i, r) in spec.refs.iter().enumerate() {
        let path = safe_join(dir, &r.image)?;
        if !path.is_file() {
            return Err(format!("refs[{i}] 的图片不存在：{}", r.image));
        }
        let idx = match r.group.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(want) => flat
                .iter()
                .position(|(g, _)| g.name.trim().eq_ignore_ascii_case(want))
                .ok_or_else(|| {
                    format!(
                        "挂靠的组名「{want}」对不上本工单的分组（{}）",
                        flat.iter()
                            .map(|(g, _)| g.name.as_str())
                            .collect::<Vec<_>>()
                            .join(" / ")
                    )
                })?,
            None if flat.len() == 1 => 0,
            None => {
                return Err(format!(
                    "refs[{i}]「{}」没写挂靠到哪个组；有 {} 个组时每张图都必须点名",
                    r.image,
                    flat.len()
                ))
            }
        };
        by_group[idx].push(path);
    }

    let mut groups = Vec::with_capacity(flat.len());
    for (i, (pg, params)) in flat.into_iter().enumerate() {
        let refs = std::mem::take(&mut by_group[i]);
        if refs.is_empty() {
            return Err(format!("分组「{}」没有挂靠任何参考图", pg.name));
        }
        let (params_json, wire_json, draws) =
            build_params(&params).map_err(|e| format!("分组「{}」的参数有问题：{e}", pg.name))?;
        groups.push(PlannedGroup {
            parsed: pg,
            refs,
            params_json,
            wire_json,
            draws,
        });
    }

    Ok(Plan {
        job_id,
        dir_name: dir_name.to_string(),
        dir: dir.to_path_buf(),
        note: spec
            .note
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        groups,
        ref_group: spec
            .ref_group
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        ephemeral: spec.ephemeral.unwrap_or(false),
    })
}

/// 工单收录结果（事件与设置页列表共用）。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct JobView {
    pub id: i64,
    pub job_id: String,
    pub dir_name: String,
    /// running / done / error / hold
    pub status: String,
    pub batch_ids: Vec<i64>,
    pub task_count: i64,
    pub group_count: i64,
    pub ref_count: i64,
    /// 各批次的参数快照（与 `batch_ids` 同序）。
    pub params_json: Vec<String>,
    /// 各批次实际发往接口的字段（与 `batch_ids` 同序）。
    pub wire_json: Vec<String>,
    pub message: String,
    pub created_at: i64,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即失败
mod tests {
    use super::*;

    fn params_from(json: &str) -> ParamsSpec {
        serde_json::from_str(json).expect("参数 JSON 应能解析")
    }

    /// 造一个工单目录：`提示词.txt` + images/。
    fn make_dir(doc: &str, images: &[&str]) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join(DOC), doc).unwrap();
        std::fs::create_dir_all(d.path().join(IMAGES)).unwrap();
        for n in images {
            std::fs::write(d.path().join(IMAGES).join(n), b"x").unwrap();
        }
        d
    }

    // 用户的核心关切：组头里写的比例必须原样落进批次快照，且能被下游解析回来。
    #[test]
    fn aspect_ratio_survives_into_snapshot_and_wire() {
        let (snap, wire, draws) = build_params(&params_from(
            r#"{"aspectRatio":"9:16","outputFormat":"png"}"#,
        ))
        .unwrap();
        assert_eq!(draws, 1);
        // 下游（调度器）读的就是这个快照 —— 用它自己的解析器验一遍。
        let p = GenParams::from_json(&snap);
        assert_eq!(p.aspect_ratio.as_deref(), Some("9:16"));
        assert_eq!(p.output_format.as_deref(), Some("png"));
        let w: serde_json::Value = serde_json::from_str(&wire).unwrap();
        assert_eq!(w["aspect_ratio"], "9:16");
        assert_eq!(w["n"], 1);
    }

    // 拼法归一化：jpg → jpeg、全角冒号 → 半角。取值本身不替用户改。
    #[test]
    fn spelling_is_normalized_but_values_are_not() {
        let (snap, _, _) =
            build_params(&params_from(r#"{"ratio":"9：16","format":"jpg"}"#)).unwrap();
        let p = GenParams::from_json(&snap);
        assert_eq!(p.aspect_ratio.as_deref(), Some("9:16"));
        assert_eq!(p.output_format.as_deref(), Some("jpeg"));
    }

    // 取值问题一律拒单：1080 不是 16 的倍数（v0.15.2 实测踩到的那个 400）。
    // 绝不替用户改成 1088 —— 静默改值正是「我设了却不生效」这类怀疑的来源。
    #[test]
    fn invalid_size_is_rejected_with_actionable_message() {
        let err = build_params(&params_from(r#"{"size":"1080x1920"}"#)).unwrap_err();
        assert!(err.contains("16 的倍数"), "{err}");
        assert!(err.contains("1088"), "还要给出可用值：{err}");
    }

    // 只写「比例:」时补上配套尺寸：单发 aspect_ratio 实测回整批 1024×1024 正方形
    // （批次 25）。取样必须用竖比例——若换成 1:1，补与不补的结果长得一样，测不出东西。
    #[test]
    fn lone_ratio_gets_its_companion_size() {
        let (snap, wire, _) = build_params(&params_from(r#"{"ratio":"9:16"}"#)).unwrap();
        let p = GenParams::from_json(&snap);
        assert_eq!(p.size.as_deref(), Some("1152x2048"));
        let w: serde_json::Value = serde_json::from_str(&wire).unwrap();
        assert_eq!(w["aspect_ratio"], "9:16");
        assert_eq!(w["size"], "1152x2048", "两个字段必须一起发");
    }

    // 用户自己写的尺寸不被配套值覆盖 —— 归一化只管拼法不管取值。
    #[test]
    fn explicit_size_wins_over_companion() {
        let (snap, _, _) =
            build_params(&params_from(r#"{"ratio":"9:16","size":"1008x1792"}"#)).unwrap();
        assert_eq!(
            GenParams::from_json(&snap).size.as_deref(),
            Some("1008x1792")
        );
    }

    // 每个配套值都得自己过端点预检（边长 16 的倍数），否则等于埋了个必炸的默认值。
    #[test]
    fn every_companion_size_passes_validation() {
        for (ratio, size) in crate::provider::RATIO_SIZES {
            let built = build_params(&params_from(&format!(r#"{{"ratio":"{ratio}"}}"#)));
            assert!(
                built.is_ok(),
                "{ratio} 的配套尺寸没过端点预检：{:?}",
                built.as_ref().err()
            );
            if let Ok((snap, _, _)) = built {
                assert_eq!(GenParams::from_json(&snap).size.as_deref(), Some(*size));
            }
        }
    }

    #[test]
    fn out_of_range_draws_is_rejected_not_clamped() {
        assert!(build_params(&params_from(r#"{"draws":9}"#))
            .unwrap_err()
            .contains("1~5"));
    }

    // 未给的参数不该出现在 wire 里（D1：不设置 = 不透传，交给模型默认）。
    #[test]
    fn absent_params_are_absent_from_wire() {
        let (snap, wire, _) = build_params(&ParamsSpec::default()).unwrap();
        let w: serde_json::Value = serde_json::from_str(&wire).unwrap();
        assert!(w.get("aspect_ratio").is_none() && w.get("size").is_none());
        let s: serde_json::Value = serde_json::from_str(&snap).unwrap();
        assert!(s.get("aspectRatio").is_none());
        assert_eq!(s["clearAiMetadata"], true);
        assert_eq!(s["draws"], 1);
    }

    #[test]
    fn escaping_paths_are_rejected() {
        let dir = Path::new("/tmp/job");
        assert!(safe_join(dir, "../../etc/passwd").is_err());
        assert!(safe_join(dir, "/etc/passwd").is_err());
        assert!(safe_join(dir, "").is_err());
        assert_eq!(
            safe_join(dir, "images/a.jpg").unwrap(),
            Path::new("/tmp/job/images/a.jpg")
        );
    }

    // 标准路径：组头带挂靠与参数，一份文档解析出两个组、各自的图与比例。
    #[test]
    fn doc_headers_drive_mapping_and_params() {
        let d = make_dir(
            "分组: 黄\n参考图: images/黄.jpg\n比例: 3:4\n抽卡: 2\n\n黄1\n\n黄2\n\n\
             分组: 蓝\n参考图: images/蓝.jpg\n比例: 9:16\n\n蓝1\n",
            &["黄.jpg", "蓝.jpg"],
        );
        let p = plan(d.path(), "job-1").unwrap();
        assert_eq!(p.groups.len(), 2);
        assert_eq!(p.groups[0].refs.len(), 1);
        assert_eq!(p.groups[0].draws, 2);
        assert_eq!(
            GenParams::from_json(&p.groups[0].params_json)
                .aspect_ratio
                .as_deref(),
            Some("3:4")
        );
        assert_eq!(
            GenParams::from_json(&p.groups[1].params_json)
                .aspect_ratio
                .as_deref(),
            Some("9:16")
        );
        // 黄：2 条 × 1 图 × 抽卡 2 = 4；蓝：1 条 × 1 图 × 1 = 1。
        assert_eq!(p.task_count(), 5);
        // 比例不同 → 必须拆成两个批次（params_json 是批次级的）。
        assert_eq!(p.batch_count(), 2);
    }

    // 参数完全相同的多个组 → 并进同一个批次，不该拆。
    #[test]
    fn same_params_share_one_batch() {
        let d = make_dir(
            "分组: 甲\n参考图: images/a.jpg\n比例: 3:4\n\n甲1\n\n\
             分组: 乙\n参考图: images/b.jpg\n比例: 3:4\n\n乙1\n",
            &["a.jpg", "b.jpg"],
        );
        let p = plan(d.path(), "job-2").unwrap();
        assert_eq!(p.groups.len(), 2);
        assert_eq!(p.batch_count(), 1);
    }

    // 单组工单可以不写 `参考图:` —— images/ 下全部图都挂给它。
    #[test]
    fn single_group_takes_all_images() {
        let d = make_dir("分组: 甲\n\n正文一\n\n正文二\n", &["a.jpg", "b.png"]);
        let p = plan(d.path(), "job-3").unwrap();
        assert_eq!(p.groups[0].refs.len(), 2);
        assert_eq!(p.task_count(), 4); // 2 条 × 2 图 × 1
    }

    // 多组却没点名挂靠 = 拒单。按顺序猜错要到验收时才看得出来，太晚了。
    #[test]
    fn multi_group_requires_explicit_mapping() {
        let d = make_dir(
            "分组: 甲\n\n甲1\n\n分组: 乙\n参考图: images/b.jpg\n\n乙1\n",
            &["a.jpg", "b.jpg"],
        );
        let err = plan(d.path(), "job-4").unwrap_err();
        assert!(err.contains("必须点名"), "{err}");
    }

    #[test]
    fn missing_ref_image_is_rejected() {
        let d = make_dir("分组: 甲\n参考图: images/没有.jpg\n\n甲1\n", &["a.jpg"]);
        assert!(plan(d.path(), "job-5")
            .unwrap_err()
            .contains("参考图不存在"));
    }

    // 组级参数非法要指名道姓说是哪个组 —— 一份文档十个组，不说清楚等于没说。
    #[test]
    fn bad_group_params_name_the_group() {
        let d = make_dir(
            "分组: 甲\n参考图: images/a.jpg\n比例: 9:15\n\n甲1\n",
            &["a.jpg"],
        );
        let err = plan(d.path(), "job-6").unwrap_err();
        assert!(err.contains("甲") && err.contains("9:15"), "{err}");
    }

    // 目录里有多个 txt/md 且没有标准名 → 明说认不出，不猜。
    #[test]
    fn ambiguous_doc_is_rejected() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.txt"), "分组: 甲\n\n甲1").unwrap();
        std::fs::write(d.path().join("b.txt"), "分组: 乙\n\n乙1").unwrap();
        assert!(plan(d.path(), "job-7").unwrap_err().contains("多个"));
    }

    // 说明.md 是给人看的，不该被当成提示词文档。
    #[test]
    fn readme_is_not_mistaken_for_the_doc() {
        let d = make_dir("分组: 甲\n\n甲1\n", &["a.jpg"]);
        std::fs::rename(d.path().join(DOC), d.path().join("提示词库.txt")).unwrap();
        std::fs::write(d.path().join("说明.md"), "# 方向地图").unwrap();
        let p = plan(d.path(), "job-8").unwrap();
        assert_eq!(p.groups.len(), 1);
    }

    // job.json 逃生舱仍然可用（内联提示词 + 按组名挂靠）。
    #[test]
    fn job_json_escape_hatch_still_works() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join(IMAGES)).unwrap();
        std::fs::write(d.path().join(IMAGES).join("a.jpg"), b"x").unwrap();
        std::fs::write(
            d.path().join(JOB),
            r#"{"note":"N","groups":[{"name":"甲","prompts":["a1","a2"],"purposes":["图生视频"]}],
                "refs":[{"image":"images/a.jpg"}],"params":{"aspectRatio":"9:16","draws":2}}"#,
        )
        .unwrap();
        let p = plan(d.path(), "job-9").unwrap();
        assert_eq!(p.note.as_deref(), Some("N"));
        assert_eq!(p.task_count(), 4); // 2 条 × 1 图 × 2
        assert!(p.groups[0]
            .parsed
            .tags
            .contains(&crate::purpose::PURPOSE_I2V.to_string()));
    }

    #[test]
    fn unknown_purpose_is_rejected() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join(JOB),
            r#"{"groups":[{"name":"甲","prompts":["a"],"purposes":["图转视频"]}],"refs":[]}"#,
        )
        .unwrap();
        assert!(plan(d.path(), "job-10").unwrap_err().contains("未知用途"));
    }
}
