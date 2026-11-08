//! 生图工单收件 —— Claude Code / Codex 侧 skill 投单，GenDesk 自动导入并开跑。
//!
//! ```text
//! <交接根>/生图/收件/<工单目录>/
//!     工单.json                     ← 全部元信息（唯一入口）
//!     refs/王一博-正面.jpg
//!     prompts/W-王一博-55-正面.txt   ← 文件名 = 未来的图片名主干；内容 = 纯正文
//!     说明.md                       ← 可选，给人看的方向地图
//!     READY.txt                     ← 最后写；没有它的目录一律跳过（skill 可能还在写）
//! <交接根>/生图/_已收录/<工单目录>-<ts>/   ← 成功后移档，内含 结果.txt
//! ```
//!
//! ## 为什么是文件而不是本地端口 / MCP 直连
//!
//! 「业务真相只在 Rust」与「单写者事务」两条铁律直接排除了外部进程写库。而 HTTP
//! 端口还有一个更实际的问题：**GenDesk 没开的时候投单会直接失败**。文件不会——
//! 启动时补跑一次扫描，昨晚投的单今天照样进得来。
//!
//! ## 为什么从「提示词.txt + 组头键」换成「工单.json + 一条一文件」
//!
//! 旧格式把元信息（`参考图:` / `比例:` / `用途:`）写在提示词文档的组头里，靠**位置
//! 绑定**——它属于紧跟其后的那个组。这解决了「按组名引用会因改名而静默断掉」，
//! 但代价是两条：
//!
//! **① 正文里不能出现像键的行。** 组头只在该组第一条正文之前生效，可这些文档的正文
//! 是长叙事，「比例：3:4 的竖构图」这种开头随时可能出现，一旦出现那条提示词当场
//! 少一句还不报错。现在正文文件里**零元信息**，这个风险从结构上消失了。
//!
//! **② 「组」这个中间层没有对应的现实。** 生成侧收敛之后，交付的单位是**一张图**：
//! 它有自己的语义名、自己的 SKU、自己的参考图。`items[].name` 即身份，一条 =
//! 一个任务 = 一张图 = 一个文件名主干。
//!
//! 挂靠因此从「组级、位置绑定」变成**条目级、每条必填**——它引用的是包内的一个
//! 文件路径，而不是一个可以被改掉的名字，所以旧格式那个「改名即断」的问题不会回来。
//!
//! ## 三条设计决定（沿用）
//!
//! **1. 收录恰好一次，靠库不靠目录。** 见 0023 的表注释：工单目录会被移动、重建、
//! 手动整理，磁盘上的标记不可靠，而重复收录 = 重复建批 = 重复花钱。
//!
//! **2. 校验与阈值都发生在写任何东西之前。** 参数非法、图片缺失、超阈值——这些
//! 在「一张图都还没进库」的时候就拦下，工单要么整份生效要么整份没发生。
//!
//! **3. 参数只归一化拼法，不归一化取值。** `jpg → jpeg`、`i2v → 图生视频`、
//! 全角冒号 → 半角是拼法；而「1080x1920 边长不是 16 的倍数」是取值问题，
//! **拒单而不是替用户改成 1088**。静默改值正是「我明明写了 9:16，出来的却不是」
//! 这类问题的成因（v0.15.2）。

pub mod ingest;
pub mod preflight;
pub mod watcher;

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::provider::GenParams;

/// 交接根下的固定子目录名。skill 侧写死这几个名字，故它们是契约，不可改。
pub const INTAKE: &str = "生图";
pub const PENDING: &str = "收件";
pub const CONSUMED: &str = "_已收录";
pub const READY: &str = "READY.txt";
/// 工单描述文件。**唯一入口**：找不到它就是拒单，不再去目录里猜哪个是提示词文档。
pub const ORDER: &str = "工单.json";
/// 提示词正文目录：一条一文件，文件名 = 未来的图片名主干。
pub const PROMPTS: &str = "prompts";
/// 参考图目录。`items[].ref` 是工单目录内的相对路径，习惯上落在这里。
pub const REFS: &str = "refs";

/// 回执文件：让 Claude Code / Codex 侧能回读结果，而不必猜后面发生了什么。
pub const ERROR_FILE: &str = "错误.txt";
pub const HOLD_FILE: &str = "待确认.txt";
pub const RESULT_FILE: &str = "结果.txt";
/// 机器可读回执，与文本回执并存；只表示收录状态，不表示生图/验收完成。
pub const RECEIPT_FILE: &str = "回执.json";

/// 文件信箱的 JSON 发布：先写同目录临时文件再 rename，读者不会读到半份 JSON。
/// 调用者负责串行写同一个目标；收录侧由 scan_lock 保证。
pub(crate) fn write_json(path: &Path, value: &impl Serialize) -> crate::error::AppResult<()> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|e| crate::error::AppError::Internal(format!("JSON 序列化失败：{e}")))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let staging = path.with_extension("json.pending");
    std::fs::write(&staging, bytes)?;
    std::fs::rename(staging, path)?;
    Ok(())
}
/// 工单名撞上台账里已有的 jobId：去重挡住了它，**但这不是稳态**。
/// 去重本身是对的（半份工单重放会造重复提示词、重复花钱），错的是不吭声——
/// 一份带着 READY.txt、永远不会被处理、也没人知道为什么的工单，
/// 正是 v0.16.0「操作信号全进了 tracing」那条教训的翻版。
pub const DUPLICATE_FILE: &str = "重名未收录.txt";
/// **确认的唯一表达**。设置页那个按钮做的事就是替你写下这个文件。
pub const CONFIRM_FILE: &str = "确认.txt";

/// 本版认得的工单格式版本。
///
/// 显式拒绝不认识的版本，而不是「尽力解析」：一份 version=2 的工单里，我们读不懂的
/// 那部分很可能正是它与 version=1 的差别所在，按 1 去读会得到一份**看起来成功了的
/// 错误结果**——而这条链路的每一次错误都直接花钱。
pub const ORDER_VERSION: i64 = 1;

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

// ───────────────────────────── 工单.json ─────────────────────────────

/// 一份工单的全部元信息。
///
/// **入参一律宽容**：camelCase 与 snake_case 都收，缺省项都有合理默认。
/// 宽容只针对**拼法**；取值不合法一律拒单。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OrderSpec {
    pub organization: Option<crate::library::Organization>,
    /// 格式版本。必须是 [`ORDER_VERSION`]。
    pub version: Option<i64>,
    /// 工单标识（去重键）。缺省取工单目录名。
    #[serde(alias = "job_id", alias = "id")]
    pub job_id: Option<String>,
    /// 这一单的名字，落到批次名，验收页按它分节。缺省用 `job_id`。
    pub note: Option<String>,
    /// **整单级**受控用途（当前只有图生视频）。取值见 [`normalize_purpose`]。
    ///
    /// 为什么是整单级而不是条目级：一份工单是为一件事写的。真要混，那本来就该是
    /// 两份工单——而两份工单在交付侧也是两个交付批，读起来对得上。
    pub purpose: Option<String>,
    /// 写出这批词的 skill 名（0032）。不声明就没有——下游不猜。
    pub skill: Option<String>,
    /// 工单级默认参数；条目没写时用它。
    pub defaults: ParamsSpec,
    pub items: Vec<ItemSpec>,
    /// 参考图入库分组名（图库目录，`ref_groups`）。缺省不分组。
    #[serde(alias = "ref_group")]
    pub ref_group: Option<String>,
    /// 参考图是否只作本批附件（不进长期图库）。缺省 false。
    pub ephemeral: Option<bool>,
}

/// 一个条目 = 一条提示词 = 一个任务 = 一张图 = 一个文件名主干。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ItemSpec {
    pub organization: Option<crate::library::Organization>,
    /// 语义名，如 `W-王一博-55-正面`。**GenDesk 一个字符都不改**（P1）。
    /// 它同时是 `prompts/{name}.txt` 的文件名。
    pub name: String,
    /// SKU 数字缩写，如 `55`。由 skill 给定，不是批次号也不是序号。
    pub sku: String,
    /// 风格 / 人物，如 `王一博`。交付时按它 + sku 分子目录。
    pub style: String,
    /// 参考图：工单目录内的相对路径，如 `refs/王一博-正面.jpg`。**每条必填**。
    #[serde(rename = "ref", alias = "image", alias = "refImage")]
    pub ref_path: String,
    /// 有序多参考图；与 ref 二选一。第一张仍是任务的主商品图。
    pub refs: Vec<String>,
    /// 条目级参数（覆盖 `defaults`）。
    #[serde(flatten)]
    pub params: ParamsSpec,
}

/// 生成参数：比例 / 尺寸 / 输出格式 / 质量，加上恒定的 `n=1`。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ParamsSpec {
    #[serde(alias = "aspect_ratio", alias = "ratio")]
    pub aspect_ratio: Option<String>,
    pub size: Option<String>,
    #[serde(alias = "output_format", alias = "format")]
    pub output_format: Option<String>,
    pub quality: Option<String>,
    #[serde(alias = "clear_ai_metadata")]
    pub clear_ai_metadata: Option<bool>,
    #[serde(alias = "remove_c2pa")]
    pub remove_c2pa: Option<bool>,
}

impl ParamsSpec {
    /// 条目级覆盖工单级：条目写了就用条目的，没写就继承 `defaults`。
    fn overlay(&self, base: &ParamsSpec) -> ParamsSpec {
        let pick = |a: &Option<String>, b: &Option<String>| a.clone().or_else(|| b.clone());
        ParamsSpec {
            aspect_ratio: pick(&self.aspect_ratio, &base.aspect_ratio),
            size: pick(&self.size, &base.size),
            output_format: pick(&self.output_format, &base.output_format),
            quality: pick(&self.quality, &base.quality),
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
    /// 批次名 / 验收页分节标题。
    pub note: String,
    /// 归一化后的受控用途；空串 = 没有用途。
    pub purpose: String,
    pub skill: Option<String>,
    pub items: Vec<PlannedItem>,
    pub ref_group: Option<String>,
    pub ephemeral: bool,
}

/// 一个已校验、已配好图、已定好参数的条目。
#[derive(Debug, Clone)]
pub struct PlannedItem {
    pub name: String,
    pub sku: String,
    pub style: String,
    /// `prompts/{name}.txt` 的内容，**逐字符原样**（只裁掉首尾空白）。
    pub prompt_text: String,
    /// 参考图绝对路径（已确认存在）。
    pub ref_path: PathBuf,
    pub ref_paths: Vec<PathBuf>,
    /// 本条生效的参数快照（与生成页 `buildParamsJson` 同形）。
    pub params_json: String,
    /// 实际会进 multipart 的字段。
    pub wire_json: String,
}

impl Plan {
    /// 任务数 = 出图张数 = 条目数。一条提示词 = 一个任务 = 一张图。
    pub fn task_count(&self) -> i64 {
        self.items.len() as i64
    }
    /// 本单用到的**去重后**参考图（保持出现顺序）。
    ///
    /// 多个条目常常挂同一张图（正面 / 侧面各写一条提示词，却共用一张原图）。
    /// 按条目逐张入库会得到几份内容相同的 `ref_images` 行，图库里就是几张一模一样
    /// 的图 —— 而落盘与缩略图还各花一遍 CPU。
    pub fn distinct_refs(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = Vec::new();
        for it in &self.items {
            for path in &it.ref_paths {
                if !out.contains(path) {
                    out.push(path.clone());
                }
            }
        }
        out
    }
    /// 这一单实际用到的几套参数（去重，保持出现顺序）。
    ///
    /// 它**不再决定切分**（一次工单 = 一个 run），只回答「我写了 9:16 到底发出去
    /// 没有」——全自动收录没有经过生成页那张确认卡，别处无处可查。
    pub fn param_sets(&self) -> Vec<(String, String)> {
        let mut seen: Vec<(String, String)> = Vec::new();
        for it in &self.items {
            let k = (it.params_json.clone(), it.wire_json.clone());
            if !seen.contains(&k) {
                seen.push(k);
            }
        }
        seen
    }
}

// ───────────────────────────── 归一化 ─────────────────────────────

/// 用途归一化：受控取值的**拼法**别名 → 落库用的那个标签。
///
/// `i2v` 与 `图生视频` 是同一个取值的两种写法（前者对写 JSON 的模型友好，后者是
/// 库里的规范形），互转属于归一化拼法。认不出来的一律**拒单**：静默当成「没有用途」
/// 会让整批本该进视频流水线的图停在作品库里，而没有任何一处会说为什么。
pub fn normalize_purpose(raw: &str) -> Result<String, String> {
    let v = raw.trim();
    if v.is_empty() {
        return Ok(String::new());
    }
    if crate::purpose::is_purpose(v) {
        return Ok(v.to_string());
    }
    if v.eq_ignore_ascii_case("i2v") || v.eq_ignore_ascii_case("v2v") {
        return Ok(crate::purpose::PURPOSE_I2V.to_string());
    }
    Err(format!(
        "未知用途「{v}」，可用：{}（或简写 i2v）",
        crate::purpose::all()
            .into_iter()
            .map(|x| x.tag)
            .collect::<Vec<_>>()
            .join(" / ")
    ))
}

/// 参数快照 + wire 记录。
///
/// 这是本模块最要紧的一段：用户点名的关切是「尺寸比例等参数能正确传递到生成端」。
/// 保证由两件事给出——(1) 产出的快照与生成页 `buildParamsJson` **同形**，走的是
/// 下游同一条解析；(2) 立刻过 `GenParams::parse_checked`，非法当场拒单。
pub fn build_params(p: &ParamsSpec) -> Result<(String, String), String> {
    let norm = |s: &Option<String>| -> Option<String> {
        s.as_deref()
            .map(|v| v.trim().replace('：', ":"))
            .filter(|v| !v.is_empty())
    };
    let aspect_ratio = norm(&p.aspect_ratio);
    let quality = match &p.quality {
        Some(v) => v.trim().to_lowercase(),
        None => crate::provider::DEFAULT_QUALITY.to_string(),
    };
    // 只写了比例没写尺寸时补上配套尺寸：实测单发 aspect_ratio 会回整批正方形
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

    let mut snap = serde_json::Map::new();
    let mut wire = serde_json::Map::new();
    for (wire_key, snap_key, value) in [
        ("aspect_ratio", "aspectRatio", aspect_ratio),
        ("size", "size", size),
        ("output_format", "outputFormat", output_format),
        ("quality", "quality", Some(quality)),
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
    // n 恒为 1：一条提示词 = 一个任务 = 一张图。
    wire.insert("n".into(), serde_json::Value::from(1));

    let params_json = serde_json::Value::Object(snap).to_string();
    // 花钱之前的本地预检，与生成页 `create_batch` 同一个函数。
    GenParams::parse_checked(&params_json)?;
    Ok((params_json, serde_json::Value::Object(wire).to_string()))
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

/// 语义名是否可以直接当文件名用。
///
/// 它会成为 `prompts/{name}.txt`、成为交付文件名的主干、成为 manifest 里的主键。
/// 只要 `sanitize_filename` 动过它一个字符，这三处就会各自看到不同的名字 ——
/// 所以判据是**原样等于清洗后的样子**，而不是「清洗一下就好了」（P1：不改一个字符）。
fn name_is_safe(name: &str) -> bool {
    !name.is_empty() && crate::files::sanitize_filename(name) == name
}

// ───────────────────────────── 校验 ─────────────────────────────

/// 工单目录 → `Plan`。任何一处不对都返回**人能照着改**的错误信息。
///
/// 只读文件、不写任何东西：整份校验通过之前，库里一个字节都不会变。
pub fn plan(dir: &Path, dir_name: &str) -> Result<Plan, String> {
    let order_path = dir.join(ORDER);
    if !order_path.is_file() {
        // 旧格式（提示词.txt + 组头键）已下线。明说它，而不是笼统地「找不到工单」——
        // 存量目录会撞上这条，而人的第一反应是「我明明放了提示词文档」。
        let legacy = dir.join("提示词.txt").is_file() || dir.join("job.json").is_file();
        return Err(if legacy {
            format!(
                "这是**旧格式**工单（提示词.txt / job.json + 组头键），本版只认 `{ORDER}`。\n\
                 新形状：{ORDER} + {PROMPTS}/{{条目名}}.txt + {REFS}/ + 最后写的 {READY}。\n\
                 让写这份工单的 skill 更新到新规范后重投。"
            )
        } else {
            format!("工单目录里没有 `{ORDER}`")
        });
    }
    let raw = std::fs::read_to_string(&order_path).map_err(|e| format!("读不到 {ORDER}：{e}"))?;
    let spec: OrderSpec =
        serde_json::from_str(&raw).map_err(|e| format!("{ORDER} 不是合法 JSON：{e}"))?;
    plan_from_spec(dir, dir_name, &spec)
}

fn plan_from_spec(dir: &Path, dir_name: &str, spec: &OrderSpec) -> Result<Plan, String> {
    plan_with_inputs(dir, dir_name, spec, None)
}

fn plan_with_inputs(
    dir: &Path,
    dir_name: &str,
    spec: &OrderSpec,
    external: Option<&preflight::ExternalInput>,
) -> Result<Plan, String> {
    match spec.version {
        Some(v) if v == ORDER_VERSION => {}
        Some(v) => {
            return Err(format!(
                "这份工单声明 version={v}，本版 GenDesk 只认 {ORDER_VERSION}。\
                 按旧版本尽力解析出来的会是一份「看起来成功了的错误结果」，\
                 而这条链路每错一次都直接花钱，故拒单。"
            ))
        }
        None => return Err(format!("{ORDER} 缺 version，应为 {ORDER_VERSION}")),
    }
    if spec.items.is_empty() {
        return Err(format!("{ORDER} 的 items 是空的：至少要有一个条目"));
    }
    let purpose = normalize_purpose(spec.purpose.as_deref().unwrap_or(""))?;
    let job_id = spec
        .job_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(dir_name)
        .to_string();

    let mut items = Vec::with_capacity(spec.items.len());
    let mut names: HashSet<String> = HashSet::new();
    for (i, it) in spec.items.iter().enumerate() {
        let name = it.name.trim();
        if !name_is_safe(name) {
            return Err(format!(
                "items[{i}] 的 name「{name}」不能直接当文件名用（不得为空，也不得含\
                 路径分隔符或文件系统保留字符）。这个名字会原样成为交付文件名的主干，\
                 GenDesk 一个字符都不会改。"
            ));
        }
        if !names.insert(name.to_string()) {
            return Err(format!(
                "items 里有两个条目都叫「{name}」。名字即身份：重名意味着两张不同的图\
                 共用一个 {PROMPTS}/{name}.txt，挂靠必然错一个。"
            ));
        }
        let sku = it.sku.trim();
        let style = it.style.trim();
        if (sku.is_empty() || style.is_empty())
            && it.organization.is_none()
            && spec.organization.is_none()
        {
            return Err(format!(
                "items[{i}]「{name}」缺 sku 或 style。交付时按 style + sku 分子目录，\
                 缺了就只能靠猜——而从中文名反推 SKU 是静默失败（分错目录不报错）。"
            ));
        }
        // 挂靠不猜：每条必填、且必须真的存在。
        if !it.ref_path.trim().is_empty() && !it.refs.is_empty() {
            return Err(format!("items[{i}]「{name}」的 ref 与 refs 只能填写一种"));
        }
        let raw_refs = if it.refs.is_empty() {
            vec![it.ref_path.clone()]
        } else {
            it.refs.clone()
        };
        if raw_refs.iter().any(|r| r.trim().is_empty()) {
            return Err(format!(
                "items[{i}]「{name}」没写 ref/refs：每个条目都必须点名参考图"
            ));
        }
        if raw_refs.len() > 16 {
            return Err(format!("items[{i}]「{name}」最多支持 16 张参考图"));
        }
        let mut ref_paths = Vec::new();
        for raw in &raw_refs {
            let packaged_path = safe_join(dir, raw)?;
            let path = match external {
                Some(input) => input
                    .references
                    .get(raw)
                    .map(PathBuf::from)
                    .ok_or_else(|| format!("参考图映射缺失：{raw}"))?,
                None => packaged_path,
            };
            if !path.is_file() {
                return Err(format!(
                    "items[{i}]「{name}」的参考图不存在：{}",
                    raw.trim()
                ));
            }
            if ref_paths.contains(&path) {
                return Err(format!("items[{i}]「{name}」重复引用同一张图"));
            }
            ref_paths.push(path);
        }
        let ref_path = ref_paths[0].clone();
        // 正文：一条一文件，文件名 = 条目名。
        let txt = dir.join(PROMPTS).join(format!("{name}.txt"));
        let prompt_text = match external {
            Some(input) => input
                .prompts
                .get(name)
                .cloned()
                .ok_or_else(|| format!("正文映射缺失：{name}"))?,
            None => std::fs::read_to_string(&txt)
                .map_err(|e| format!("读不到 {PROMPTS}/{name}.txt：{e}"))?,
        }
        .trim()
        .to_string();
        if prompt_text.is_empty() {
            return Err(format!("{PROMPTS}/{name}.txt 是空的"));
        }
        let (mut params_json, wire_json) = build_params(&it.params.overlay(&spec.defaults))
            .map_err(|e| format!("items[{i}]「{name}」的参数有问题：{e}"))?;
        let organization = it
            .organization
            .as_ref()
            .or(spec.organization.as_ref())
            .cloned()
            .unwrap_or_else(|| {
                crate::library::Organization::legacy(sku, style, spec.note.as_deref().unwrap_or(""))
            });
        organization.validate().map_err(|e| e.to_string())?;
        let mut params: serde_json::Value =
            serde_json::from_str(&params_json).map_err(|e| e.to_string())?;
        params["organization"] = serde_json::to_value(&organization).map_err(|e| e.to_string())?;
        params_json = params.to_string();
        items.push(PlannedItem {
            name: name.to_string(),
            sku: sku.to_string(),
            style: style.to_string(),
            prompt_text,
            ref_path,
            ref_paths,
            params_json,
            wire_json,
        });
    }

    // 反向核对：`prompts/` 里不许有 items 之外的 txt。
    //
    // 多出来的那一份要么是 skill 漏写了 items 条目，要么是上一版工单的残留 —— 两种
    // 都意味着这份工单与写它的人以为的不一样，而少跑一张图是**不报错**的失败。
    let extras = match external {
        Some(input) => input
            .prompts
            .keys()
            .filter(|name| !names.contains(*name))
            .cloned()
            .collect(),
        None => stray_prompt_files(dir, &names),
    };
    if !extras.is_empty() {
        return Err(format!(
            "{PROMPTS}/ 里有 items 之外的正文：{}。要么给它们补上 items 条目，\
             要么删掉——多一个少一个都说明这份工单与你以为的不一样。",
            extras.join(" / ")
        ));
    }

    Ok(Plan {
        job_id: job_id.clone(),
        dir_name: dir_name.to_string(),
        dir: dir.to_path_buf(),
        note: spec
            .note
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(&job_id)
            .to_string(),
        purpose,
        skill: spec
            .skill
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        items,
        ref_group: spec
            .ref_group
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        ephemeral: spec.ephemeral.unwrap_or(false),
    })
}

/// `prompts/` 下不在 `names` 里的 `.txt`（排序保证报错信息稳定）。
fn stray_prompt_files(dir: &Path, names: &HashSet<String>) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir.join(PROMPTS)) else {
        // 目录读不动时，上面读正文那一步已经先报过错了。
        return Vec::new();
    };
    let mut out: Vec<String> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.extension()
                    .and_then(|s| s.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case("txt"))
        })
        .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
        .filter(|stem| !names.contains(stem))
        .collect();
    out.sort();
    out
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
    /// 一次工单 = 一个 run（0045），故成功时恒为一个元素。
    pub batch_ids: Vec<i64>,
    pub task_count: i64,
    /// 条目数。一条 = 一个任务 = 一张图，故成功时它等于 `task_count`；
    /// 保留成两列是因为台账列（0023 的 `group_count`）是既有的，
    /// 且中途失败时「导入了几条」与「建了几个任务」确实可以不等。
    pub group_count: i64,
    pub ref_count: i64,
    /// 这一单用过的几套参数快照（去重）。**不再与 `batch_ids` 同序**
    /// （0023 的表注释写于按参数拆批的年代）。
    pub params_json: Vec<String>,
    /// 对应的「实际发往接口的字段」，与 `params_json` 同序。
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

    /// 造一个 v2 工单目录。
    fn make_dir(order: &str, prompts: &[(&str, &str)], refs: &[&str]) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join(ORDER), order).unwrap();
        std::fs::create_dir_all(d.path().join(PROMPTS)).unwrap();
        std::fs::create_dir_all(d.path().join(REFS)).unwrap();
        for (name, text) in prompts {
            std::fs::write(d.path().join(PROMPTS).join(format!("{name}.txt")), text).unwrap();
        }
        for n in refs {
            std::fs::write(d.path().join(REFS).join(n), b"x").unwrap();
        }
        d
    }

    const ORDER_OK: &str = r#"{
      "version": 1, "purpose": "i2v",
      "defaults": {"ratio": "9:16", "format": "jpeg"},
      "items": [
        {"name":"W-王一博-55-正面","sku":"55","style":"王一博","ref":"refs/正面.jpg"},
        {"name":"W-王一博-55-侧面","sku":"55","style":"王一博","ref":"refs/侧面.jpg",
         "ratio":"3:4","size":"1088x1440"}
      ]}"#;

    #[test]
    fn explicit_ownership_survives_parameter_snapshot_without_becoming_remote_fields() {
        let d = make_dir(
            r#"{"version":1,"organization":{"product":"陶瓷水杯","style":"奶油白款","requirement":"厨房场景图","source":"任务单"},"items":[{"name":"杯子场景","ref":"refs/a.jpg"}]}"#,
            &[("杯子场景", "正文")],
            &["a.jpg"],
        );
        let first = plan(d.path(), "run-a").unwrap();
        let second = plan(d.path(), "run-b").unwrap();
        let a = crate::library::Organization::from_params(&first.items[0].params_json)
            .unwrap()
            .unwrap();
        let b = crate::library::Organization::from_params(&second.items[0].params_json)
            .unwrap()
            .unwrap();
        assert_eq!(a, b);
        assert_eq!(
            a.directory(Path::new("/library"), true, false),
            PathBuf::from("/library/成品/生成图片/陶瓷水杯/奶油白款/厨房场景图")
        );
        assert!(!first.items[0].wire_json.contains("organization"));
    }

    // 主路径：两条目、各自的图与参数、整单级用途。
    #[test]
    fn a_well_formed_order_plans_one_task_per_item() {
        let d = make_dir(
            ORDER_OK,
            &[
                ("W-王一博-55-正面", "正面正文"),
                ("W-王一博-55-侧面", "侧面正文"),
            ],
            &["正面.jpg", "侧面.jpg"],
        );
        let p = plan(d.path(), "投单-260804-1530").unwrap();
        assert_eq!(p.task_count(), 2, "一条 = 一个任务 = 一张图");
        assert_eq!(
            p.purpose,
            crate::purpose::PURPOSE_I2V,
            "i2v 归一化成受控取值"
        );
        assert_eq!(p.items[0].sku, "55");
        assert_eq!(p.items[0].style, "王一博");
        assert!(p.items[0].ref_path.ends_with("正面.jpg"));
        assert_eq!(p.items[1].prompt_text, "侧面正文");
        // 条目级参数覆盖 defaults，且两个字段都要在。
        let a = GenParams::from_json(&p.items[0].params_json);
        assert_eq!(a.aspect_ratio.as_deref(), Some("9:16"));
        assert_eq!(a.size.as_deref(), Some("1152x2048"), "只写比例要补配套尺寸");
        let b = GenParams::from_json(&p.items[1].params_json);
        assert_eq!(b.aspect_ratio.as_deref(), Some("3:4"));
        assert_eq!(b.size.as_deref(), Some("1088x1440"));
        // 两套参数都要留痕，但参数不再决定切分。
        assert_eq!(p.param_sets().len(), 2);
        assert_eq!(p.distinct_refs().len(), 2);
    }

    // 多条目共用同一张原图时只入库一次：否则图库里躺着几张一模一样的图，
    // 而落盘与缩略图还各花一遍 CPU。
    #[test]
    fn items_sharing_one_ref_ingest_it_once() {
        let d = make_dir(
            r#"{"version":1,"items":[
                {"name":"A","sku":"5","style":"S","ref":"refs/a.jpg"},
                {"name":"B","sku":"5","style":"S","ref":"refs/a.jpg"}]}"#,
            &[("A", "甲"), ("B", "乙")],
            &["a.jpg"],
        );
        let p = plan(d.path(), "j").unwrap();
        assert_eq!(p.task_count(), 2);
        assert_eq!(p.distinct_refs().len(), 1, "同一张图只该入库一次");
    }

    // 正文进库必须**逐字符**等于 txt 内容：新格式的全部意义就是正文里零元信息。
    #[test]
    fn prompt_text_is_the_file_verbatim() {
        let body = "比例：3:4 的竖构图，人物居中\n第二行：尺寸: 不该被吃掉";
        let d = make_dir(
            r#"{"version":1,"items":[{"name":"A","sku":"55","style":"王一博","ref":"refs/a.jpg"}]}"#,
            &[("A", body)],
            &["a.jpg"],
        );
        let p = plan(d.path(), "j").unwrap();
        assert_eq!(
            p.items[0].prompt_text, body,
            "正文里像组头键的行必须原样保留 —— 那正是旧格式会静默吃掉的东西"
        );
    }

    #[test]
    fn a_missing_ref_is_rejected() {
        let d = make_dir(
            r#"{"version":1,"items":[{"name":"A","sku":"55","style":"S","ref":"refs/没有.jpg"}]}"#,
            &[("A", "正文")],
            &["a.jpg"],
        );
        assert!(plan(d.path(), "j").unwrap_err().contains("参考图不存在"));
    }

    #[test]
    fn an_item_without_ref_is_rejected() {
        let d = make_dir(
            r#"{"version":1,"items":[{"name":"A","sku":"55","style":"S"}]}"#,
            &[("A", "正文")],
            &["a.jpg"],
        );
        assert!(plan(d.path(), "j").unwrap_err().contains("必须点名参考图"));
    }

    // prompts/ 与 items 必须一一对应：多一个少一个都拒。
    #[test]
    fn a_missing_prompt_file_is_rejected() {
        let d = make_dir(
            r#"{"version":1,"items":[{"name":"A","sku":"5","style":"S","ref":"refs/a.jpg"}]}"#,
            &[],
            &["a.jpg"],
        );
        assert!(plan(d.path(), "j").unwrap_err().contains("A.txt"));
    }

    #[test]
    fn a_stray_prompt_file_is_rejected() {
        let d = make_dir(
            r#"{"version":1,"items":[{"name":"A","sku":"5","style":"S","ref":"refs/a.jpg"}]}"#,
            &[("A", "正文"), ("B", "多出来的")],
            &["a.jpg"],
        );
        let err = plan(d.path(), "j").unwrap_err();
        assert!(err.contains('B'), "{err}");
    }

    #[test]
    fn duplicate_item_names_are_rejected() {
        let d = make_dir(
            r#"{"version":1,"items":[
                {"name":"A","sku":"5","style":"S","ref":"refs/a.jpg"},
                {"name":"A","sku":"5","style":"S","ref":"refs/a.jpg"}]}"#,
            &[("A", "正文")],
            &["a.jpg"],
        );
        assert!(plan(d.path(), "j").unwrap_err().contains("都叫"));
    }

    // 名字会原样成为文件名，故动过一个字符就不算安全 —— 清洗一下放行是不行的。
    #[test]
    fn an_unsafe_item_name_is_rejected_not_sanitized() {
        for bad in ["a/b", "", "   ", "a:b"] {
            let d = make_dir(
                &format!(
                    r#"{{"version":1,"items":[{{"name":"{bad}","sku":"5","style":"S","ref":"refs/a.jpg"}}]}}"#
                ),
                &[],
                &["a.jpg"],
            );
            assert!(plan(d.path(), "j").is_err(), "「{bad}」不该被放行");
        }
    }

    #[test]
    fn missing_sku_or_style_is_rejected() {
        let d = make_dir(
            r#"{"version":1,"items":[{"name":"A","sku":"","style":"S","ref":"refs/a.jpg"}]}"#,
            &[("A", "正文")],
            &["a.jpg"],
        );
        assert!(plan(d.path(), "j").unwrap_err().contains("sku"));
    }

    // 未知 version 一律拒：按 1 去读会得到一份「看起来成功了的错误结果」。
    #[test]
    fn an_unknown_or_missing_version_is_rejected() {
        let d = make_dir(
            r#"{"version":2,"items":[{"name":"A","sku":"5","style":"S","ref":"refs/a.jpg"}]}"#,
            &[("A", "正文")],
            &["a.jpg"],
        );
        assert!(plan(d.path(), "j").unwrap_err().contains("version=2"));
        let d2 = make_dir(
            r#"{"items":[{"name":"A","sku":"5","style":"S","ref":"refs/a.jpg"}]}"#,
            &[("A", "正文")],
            &["a.jpg"],
        );
        assert!(plan(d2.path(), "j").unwrap_err().contains("version"));
    }

    // 旧格式目录要被明确点名，而不是笼统地「找不到工单」。
    #[test]
    fn a_legacy_order_says_so_plainly() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("提示词.txt"), "分组: 甲\n\n甲1").unwrap();
        let err = plan(d.path(), "j").unwrap_err();
        assert!(err.contains("旧格式"), "{err}");
        assert!(err.contains(ORDER), "要说清新形状：{err}");
    }

    #[test]
    fn an_unknown_purpose_is_rejected() {
        assert!(normalize_purpose("图转视频").is_err());
        assert_eq!(normalize_purpose("").unwrap(), "");
        assert_eq!(
            normalize_purpose("I2V").unwrap(),
            crate::purpose::PURPOSE_I2V
        );
        assert_eq!(
            normalize_purpose(crate::purpose::PURPOSE_I2V).unwrap(),
            crate::purpose::PURPOSE_I2V
        );
    }

    // 用户的核心关切：写下的比例必须原样落进快照，且能被下游解析回来。
    #[test]
    fn aspect_ratio_survives_into_snapshot_and_wire() {
        let (snap, wire) = build_params(&params_from(
            r#"{"aspectRatio":"9:16","outputFormat":"png"}"#,
        ))
        .unwrap();
        // 下游（调度器）读的就是这个快照 —— 用它自己的解析器验一遍。
        let p = GenParams::from_json(&snap);
        assert_eq!(p.aspect_ratio.as_deref(), Some("9:16"));
        assert_eq!(p.output_format.as_deref(), Some("png"));
        let w: serde_json::Value = serde_json::from_str(&wire).unwrap();
        assert_eq!(w["aspect_ratio"], "9:16");
        assert_eq!(w["n"], 1);
        assert_eq!(p.quality.as_deref(), Some("high"));
        assert_eq!(w["quality"], "high");
    }

    #[test]
    fn item_quality_overrides_defaults_and_reaches_snapshot_and_wire() {
        let base = params_from(r#"{"quality":"medium"}"#);
        let item = params_from(r#"{"quality":" MAX "}"#);
        for (params, expected) in [
            (ParamsSpec::default().overlay(&base), "medium"),
            (item.overlay(&base), "max"),
            (ParamsSpec::default(), "high"),
        ] {
            let (snap, wire) = build_params(&params).unwrap();
            assert_eq!(
                GenParams::parse_checked(&snap).unwrap().quality.as_deref(),
                Some(expected)
            );
            let wire: serde_json::Value = serde_json::from_str(&wire).unwrap();
            assert_eq!(wire["quality"], expected);
        }
        for bad in [r#"{"quality":""}"#, r#"{"quality":"hgh"}"#] {
            assert!(build_params(&params_from(bad)).is_err());
        }
    }

    // 拼法归一化：jpg → jpeg、全角冒号 → 半角。取值本身不替用户改。
    #[test]
    fn spelling_is_normalized_but_values_are_not() {
        let (snap, _) = build_params(&params_from(r#"{"ratio":"9：16","format":"jpg"}"#)).unwrap();
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

    // 只写「比例」时补上配套尺寸：单发 aspect_ratio 实测回整批 1024×1024 正方形。
    // 取样必须用竖比例——若换成 1:1，补与不补的结果长得一样，测不出东西。
    #[test]
    fn lone_ratio_gets_its_companion_size() {
        let (snap, wire) = build_params(&params_from(r#"{"ratio":"9:16"}"#)).unwrap();
        let p = GenParams::from_json(&snap);
        assert_eq!(p.size.as_deref(), Some("1152x2048"));
        let w: serde_json::Value = serde_json::from_str(&wire).unwrap();
        assert_eq!(w["aspect_ratio"], "9:16");
        assert_eq!(w["size"], "1152x2048", "两个字段必须一起发");
    }

    // 用户自己写的尺寸不被配套值覆盖 —— 归一化只管拼法不管取值。
    #[test]
    fn explicit_size_wins_over_companion() {
        let (snap, _) =
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
            if let Ok((snap, _)) = built {
                assert_eq!(GenParams::from_json(&snap).size.as_deref(), Some(*size));
            }
        }
    }

    // 未给的参数不该出现在 wire 里（D1：不设置 = 不透传，交给模型默认）。
    #[test]
    fn absent_params_are_absent_from_wire() {
        let (snap, wire) = build_params(&ParamsSpec::default()).unwrap();
        let w: serde_json::Value = serde_json::from_str(&wire).unwrap();
        assert!(w.get("aspect_ratio").is_none() && w.get("size").is_none());
        let s: serde_json::Value = serde_json::from_str(&snap).unwrap();
        assert!(s.get("aspectRatio").is_none());
        assert_eq!(s["clearAiMetadata"], true);
    }

    #[test]
    fn escaping_paths_are_rejected() {
        let dir = Path::new("/tmp/job");
        assert!(safe_join(dir, "../../etc/passwd").is_err());
        assert!(safe_join(dir, "/etc/passwd").is_err());
        assert!(safe_join(dir, "").is_err());
        assert_eq!(
            safe_join(dir, "refs/a.jpg").unwrap(),
            Path::new("/tmp/job/refs/a.jpg")
        );
    }
}
