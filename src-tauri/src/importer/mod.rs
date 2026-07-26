//! 提示词 txt 导入（执行计划 1.6 / 需求 6.4）。
//!
//! 解析「分组 / 前缀 / 场景 / 标签 / 小标题 / 正文」字段 + UTF-8/GBK 编码探测；
//! 两段式：`parse`（纯函数，不落库）→ 命令层 `commit`（落库 + 号池发放）。
//!
//! 格式约定（宽泛解析，多种写法并存；目标：常见 txt 结构无需改格式即可正确识别）：
//! - 显式分组头：关键字 `分组`/`组`/`group`（大小写不敏感）后接以下任一分隔即开启新分组——
//!   冒号 `分组: 名称`（半/全角）· 短横线/等号 `分组-名称`/`分组=名称` · 内联括号 `分组【名称】`
//!   · 空白 `分组 名称`（仅 ≥2 字关键字，避免单字 `组` 误伤正文）；一个文件多头即按分组自动拆分。
//! - **括号包裹的分组头**：整行括号块内部本身是分组头（如 `【分组：鹿晗】`/`[组-A]`）→ 直接作分组，
//!   不参与下述前瞻判层。这样「`【分组：X】` 换行 正文…」这类把「分组：」一起括起来的写法也能识别。
//! - **裸括号自动判层**：独占一行的括号块 `【名称】`（亦支持 `[名称]`/`［名称］`/`〖名称〗`），
//!   若其下一条非空行是「正文」→ 视为该正文的**小标题**；否则（下一条是另一括号块 / 元信息头）
//!   → 视为**分组头**。这样图中「`【分组】` 换行 `【小标题】` 换行 `1．正文`」结构可零配置识别。
//! - **无标记文档的形态推断**（`heuristic_mode`：全文一处显式分组关键字都没有时才启用）：
//!   「明显短于正文的独立行」按其**管辖的正文条数**判层 —— 管 ≥2 条 → 分组头；恰好 1 条 → 小标题。
//!   这条规则让「首行写个标题、下面全是长段落」这种最常见的手写 txt 零配置就能分对组；
//!   门槛严（正文中位数 ≥60 字、标题 ≤40 字且 ≤ 中位数 1/3、无句末标点、无前导序号）以免误吃正文。
//!   推断出的分组标 `origin = Inferred`，UI 以「疑似」呈现并允许改名/并组。
//! - 其它头部行 `前缀:`/`场景:`/`标签:` 设置当前分组元信息；
//! - 正文行前导序号（`1.`/`2、`/`3）`/`(4)`/`（5）`/`①` 等）自动剥离；
//! - 其余每条非空行 = 一条提示词（一行一提示词，空行忽略）；
//! - 缺分组 → 用文件名兜底（`parse_named`），再兜底「未分组导入」；缺前缀 → 由 commit 阶段自动分配。

/// 单条解析出的提示词（正文 + 可选小标题）。
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedPrompt {
    /// 来自 `【小标题】` 行；无则 None。
    pub title: Option<String>,
    pub text: String,
}

/// 分组名的来源，决定 UI 的确信度呈现。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum GroupOrigin {
    /// 文档里有明确的分组标记（`分组: X` / 独立括号行）。
    Explicit,
    /// 由行的形态推断（短标题行 + 其下多条正文）——UI 标「疑似」。
    Inferred,
    /// 文档里没有任何线索，用文件名或默认名兜底。
    #[default]
    Fallback,
}

/// 单个解析出的分组。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParsedGroup {
    pub name: String,
    pub prefix: Option<String>,
    pub scene: String,
    pub tags: Vec<String>,
    pub prompts: Vec<ParsedPrompt>,
    /// 分组名从哪来（Explicit / Inferred / Fallback）。
    pub origin: GroupOrigin,
    /// 挂靠到本组的参考图（工单目录内相对路径，来自组头 `参考图:`）。
    ///
    /// **位置绑定，不引用组名**：它属于紧跟其后的那个组。相比「按组名引用」的写法，
    /// 改组名不会让挂靠悄悄断掉——而挂靠断掉的后果（整批图配错提示词）要到验收时才看得出来。
    pub refs: Vec<String>,
    /// 组头 `比例:` / `尺寸:` / `格式:` / `抽卡:`。批次参数是**批次级**的，故收件侧会把
    /// 参数相同的组并进同一个批次、不同的拆成多个批次（见 `intake`）。
    pub ratio: Option<String>,
    pub size: Option<String>,
    pub format: Option<String>,
    pub draws: Option<i64>,
}

/// 解析诊断（E37：行号级报错/提示，非致命）。
#[derive(Debug, Clone, PartialEq)]
pub struct ParseWarning {
    /// 1-based 行号。
    pub line: usize,
    pub message: String,
}

/// 解析结果。
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedImport {
    /// 探测到的编码名（如 "UTF-8" / "GBK"）。
    pub encoding: String,
    pub groups: Vec<ParsedGroup>,
    /// 非致命诊断（缺分组标记、悬空小标题等），含行号。
    pub warnings: Vec<ParseWarning>,
}

impl ParsedImport {
    /// 解析出的提示词总条数。
    ///
    /// `allow(dead_code)`：生产侧的总数改由预览按最终分组累加（工单收件的内联提示词
    /// 不经解析器，两条入口要同一个口径）；这里保留给解析器自己的测试断言用。
    #[allow(dead_code)]
    pub fn total_prompts(&self) -> usize {
        self.groups.iter().map(|g| g.prompts.len()).sum()
    }
}

const DEFAULT_GROUP: &str = "未分组导入";

/// 探测编码并解码为字符串。
pub fn decode(bytes: &[u8]) -> (String, String) {
    let mut detector = chardetng::EncodingDetector::new();
    detector.feed(bytes, true);
    let enc = detector.guess(None, true);
    let (cow, _, _) = enc.decode(bytes);
    (enc.name().to_string(), cow.into_owned())
}

/// 解析字节流为结构化导入结果（不落库、不 panic）。
///
/// `file_stem` = 文件名（不含扩展名），用作「文档里没写分组名」时的兜底组名：比固定的
/// 「未分组导入」有意义得多，`B-Roll素材分镜提示词_20260724.txt` → 组名
/// `B-Roll素材分镜提示词`（尾部日期/副本后缀会被清掉）。无文件名时传 None。
pub fn parse_named(bytes: &[u8], file_stem: Option<&str>) -> ParsedImport {
    let (encoding, text) = decode(bytes);
    let (mut groups, mut warnings) = parse_text(&text);

    // 全文没给出任何分组线索 → 用文件名兜底命名那个默认组。
    if groups.len() == 1 && groups[0].origin == GroupOrigin::Fallback {
        let named = file_stem.map(clean_file_stem).filter(|s| !s.is_empty());
        if let Some(name) = named {
            warnings.push(ParseWarning {
                line: 0,
                message: format!(
                    "文件里没有分组标记，已用文件名作为分组名「{name}」。可直接在下方改名。"
                ),
            });
            groups[0].name = name;
        }
    }

    ParsedImport {
        encoding,
        groups,
        warnings,
    }
}

/// 文件名 → 分组名：去掉尾部日期戳（`_20260724` / `-2026-07-24`）、`(1)`、`副本`/`copy`
/// 之类的噪声后缀，压缩空白。清不出东西时返回空串（调用方回落默认名）。
fn clean_file_stem(stem: &str) -> String {
    let mut s = stem.trim();
    loop {
        let before = s;
        s = s
            .trim_end_matches([' ', '_', '-', '－', '—', '·'])
            .trim_end();
        for suffix in ["副本", "copy", "Copy", "COPY"] {
            if let Some(rest) = s.strip_suffix(suffix) {
                s = rest.trim_end();
            }
        }
        // 尾部 `(1)` / `（2）`
        if let Some(rest) = s.strip_suffix([')', '）']) {
            if let Some(open) = rest.rfind(['(', '（']) {
                let inner = &rest[open..];
                if inner.chars().skip(1).all(|c| c.is_ascii_digit()) && inner.chars().count() > 1 {
                    s = rest[..open].trim_end();
                }
            }
        }
        // 尾部日期戳：连续 6~8 位数字，或 `2026-07-24` / `2026_07_24`。
        let tail_len = s
            .chars()
            .rev()
            .take_while(|c| c.is_ascii_digit() || matches!(c, '-' | '_' | '.'))
            .count();
        if tail_len > 0 {
            let cut = s.len()
                - s.chars()
                    .rev()
                    .take(tail_len)
                    .map(char::len_utf8)
                    .sum::<usize>();
            let tail = &s[cut..];
            let digits = tail.chars().filter(char::is_ascii_digit).count();
            // 只吃「像日期」的尾巴，且不能把整个名字吃光。
            if (6..=8).contains(&digits) && cut > 0 {
                s = s[..cut].trim_end();
            }
        }
        if s == before {
            break;
        }
    }
    s.trim().to_string()
}

/// 形态推断的门槛（见模块文档）。定得保守：只在「长段落正文 + 短标题行」这种
/// 一眼可辨的文档上生效，宁可不认也不误吃正文。
const HEADING_MAX_CHARS: usize = 40;
const HEADING_MIN_BODY_MEDIAN: usize = 60;
const HEADING_MIN_BODY_LINES: usize = 3;

/// 「典型正文长度」基准（字符数），取 75 分位而非中位数：标题行本身也计入样本，
/// 中位数会被它们拉低（两层结构「标题 / 小标题 / 长正文」里短行可占一半）。
/// 样本不足或基准过短（=文档本来就都是短句）则返回 None，即不启用形态推断。
fn body_scale(lines: &[(usize, &str)]) -> Option<usize> {
    let mut lens: Vec<usize> = lines
        .iter()
        .filter(|(_, l)| is_body_line(l))
        .map(|(_, l)| l.chars().count())
        .collect();
    if lens.len() < HEADING_MIN_BODY_LINES {
        return None;
    }
    lens.sort_unstable();
    let scale = lens[lens.len() * 3 / 4];
    (scale >= HEADING_MIN_BODY_MEDIAN).then_some(scale)
}

/// 一行是否「长得像标题」：显著短于正文中位数、无句末标点、无前导序号。
/// 仅在 `heuristic_mode`（全文无显式分组标记）下参与判层。
fn is_heading_shape(line: &str, median: usize) -> bool {
    let n = line.chars().count();
    n > 0
        && n <= HEADING_MAX_CHARS
        && n * 3 <= median
        && !line.contains(['。', '！', '？', '；', '，', '!', '?', ';'])
        && strip_leading_number(line) == line
}

fn parse_text(text: &str) -> (Vec<ParsedGroup>, Vec<ParseWarning>) {
    // 预收集非空行（保留 1-based 原始行号），便于对裸括号做前瞻判层。
    let lines: Vec<(usize, &str)> = text
        .lines()
        .enumerate()
        .filter_map(|(idx, raw)| {
            let t = raw.trim_end_matches('\r').trim();
            (!t.is_empty()).then_some((idx + 1, t))
        })
        .collect();

    let mut groups: Vec<ParsedGroup> = Vec::new();
    let mut cur: Option<ParsedGroup> = None;
    let mut warnings: Vec<ParseWarning> = Vec::new();
    // 待挂靠的小标题：遇到 `【小标题】` 行后暂存，附加到下一条正文。含行号供悬空诊断。
    let mut pending_title: Option<(String, usize)> = None;
    // 是否已见过任一「分组」标记（E37：正文出现在分组标记前时告警一次）。
    let mut seen_group_header = false;
    let mut orphan_warning: Option<ParseWarning> = None;

    // 形态推断只在「全文一处显式分组关键字都没有」时启用：文档一旦自己表过态，就完全听它的。
    let heuristic_mode = !lines.iter().any(|(_, l)| {
        parse_group_header(l).is_some()
            || parse_bracket_line(l).is_some_and(|inner| parse_group_header(&inner).is_some())
    });
    let median = if heuristic_mode {
        body_scale(&lines)
    } else {
        None
    };
    // 第 i 行之后连续「纯正文」行的条数（标题形态的行不算正文）。判层用：管 ≥2 条 → 分组。
    let body_run = |i: usize| -> usize {
        lines[i + 1..]
            .iter()
            .take_while(|(_, l)| is_body_line(l) && !median.is_some_and(|m| is_heading_shape(l, m)))
            .count()
    };

    // 确保存在「当前分组」；否则建默认分组。
    fn ensure(cur: &mut Option<ParsedGroup>) -> &mut ParsedGroup {
        cur.get_or_insert_with(|| ParsedGroup {
            name: DEFAULT_GROUP.to_string(),
            origin: GroupOrigin::Fallback,
            ..Default::default()
        })
    }
    fn new_group(name: String, origin: GroupOrigin) -> ParsedGroup {
        ParsedGroup {
            name,
            origin,
            ..Default::default()
        }
    }
    fn warn_dangling(warnings: &mut Vec<ParseWarning>, pending: Option<(String, usize)>) {
        if let Some((t, tline)) = pending {
            warnings.push(ParseWarning {
                line: tline,
                message: format!("小标题「{t}」后没有正文，已忽略。"),
            });
        }
    }

    // 解析模型：每条非空、非头部/非小标题行 = 一条提示词（一行一提示词）。
    // 空行仅作视觉分隔，被忽略。行号 1-based。
    for i in 0..lines.len() {
        let (line, trimmed) = lines[i];

        // 显式分组头：`分组: 名称` 或 `分组【名称】`（优先于裸括号/小标题判定）。
        if let Some(name) = parse_group_header(trimmed) {
            warn_dangling(&mut warnings, pending_title.take());
            if let Some(g) = cur.take() {
                groups.push(g);
            }
            cur = Some(new_group(name, GroupOrigin::Explicit));
            seen_group_header = true;
            continue;
        }

        if let Some((key, value)) = parse_header(trimmed) {
            match key {
                Header::Prefix => ensure(&mut cur).prefix = Some(value.to_uppercase()),
                Header::Scene => ensure(&mut cur).scene = value.to_string(),
                Header::Tags => ensure(&mut cur).tags = split_tags(value),
            }
            continue;
        }

        // 投单组头（`参考图:` / `比例:` / `抽卡:` / `用途:` / `尺寸:` / `格式:`）。
        //
        // **只在组头区生效**——即该组还没有任何正文时。老键（前缀/场景/标签）为了不改动
        // 既有行为仍是全文任意位置认，但新键不能这样：这些文档的正文是长叙事，
        // 万一某条以「比例：3:4 的竖构图」开头，整行会被当元信息吃掉，那条提示词
        // 当场少一句还不报错。限定在组头区之后，这个风险基本归零。
        // `map_or` 而非 `is_none_or`：后者要 Rust 1.82，本 crate 的 MSRV 是 1.77。
        if cur.as_ref().map_or(true, |g| g.prompts.is_empty()) {
            if let Some((key, value)) = parse_intake_header(trimmed) {
                let g = ensure(&mut cur);
                match key {
                    IntakeHeader::Refs => g.refs = split_list(value),
                    IntakeHeader::Ratio => g.ratio = Some(value.to_string()),
                    IntakeHeader::Size => g.size = Some(value.to_string()),
                    IntakeHeader::Format => g.format = Some(value.to_string()),
                    IntakeHeader::Draws => g.draws = value.trim().parse::<i64>().ok(),
                    // 用途与自由标签共用 tags：并进去即被判为**显式**用途而非关键词预猜，
                    // 下游一处也不用改（受控取值的校验在命令边界，这里只负责收下）。
                    IntakeHeader::Purpose => {
                        for t in split_list(value) {
                            if !g.tags.contains(&t) {
                                g.tags.push(t);
                            }
                        }
                    }
                }
                continue;
            }
        }

        // 独占一行的裸括号块 → 依「下一条非空行」判层：
        //   下一条是正文  → 本行是该正文的小标题；
        //   下一条是括号/元信息头 → 本行是分组头；
        //   已到文件尾（无下一条）→ 视为悬空小标题（结尾告警）。
        if let Some(inner) = parse_bracket_line(trimmed) {
            // 括号内部本身即分组头（如 `【分组：鹿晗】`）→ 直接作分组，忽略前瞻判层。
            if let Some(name) = parse_group_header(&inner) {
                warn_dangling(&mut warnings, pending_title.take());
                if let Some(g) = cur.take() {
                    groups.push(g);
                }
                cur = Some(new_group(name, GroupOrigin::Explicit));
                seen_group_header = true;
                continue;
            }
            // 下一条是正文时通常是小标题；但若尚未出现过任何分组、且它下面管着 ≥2 条正文，
            // 那它是这批正文的分组头（`【某某场景图】` 换行 一堆长正文 的常见写法）。
            let as_group = match lines.get(i + 1) {
                Some(&(_, next)) if is_body_line(next) => !seen_group_header && body_run(i) >= 2,
                Some(_) => true,
                None => false,
            };
            if as_group {
                warn_dangling(&mut warnings, pending_title.take());
                if let Some(g) = cur.take() {
                    groups.push(g);
                }
                cur = Some(new_group(inner, GroupOrigin::Explicit));
                seen_group_header = true;
            } else {
                warn_dangling(&mut warnings, pending_title.replace((inner, line)));
            }
            continue;
        }

        // 无标记文档：按形态判层。短标题行管 ≥2 条正文 → 分组头；恰管 1 条 → 小标题。
        if let Some(m) = median {
            if is_heading_shape(trimmed, m) {
                let run = body_run(i);
                let next_is_heading = lines
                    .get(i + 1)
                    .is_some_and(|(_, l)| !is_body_line(l) || is_heading_shape(l, m));
                if run >= 2 || (run == 0 && next_is_heading) {
                    warn_dangling(&mut warnings, pending_title.take());
                    if let Some(g) = cur.take() {
                        groups.push(g);
                    }
                    cur = Some(new_group(trimmed.to_string(), GroupOrigin::Inferred));
                    seen_group_header = true;
                    continue;
                }
                if run == 1 {
                    warn_dangling(
                        &mut warnings,
                        pending_title.replace((trimmed.to_string(), line)),
                    );
                    continue;
                }
                // run == 0 且已到文件尾 → 当作普通正文落下去。
            }
        }

        // 正文行：若此前从未见过「分组」标记，暂存一条告警（含行号）。
        // 只有当它最终真的成了「一堆正常分组旁边的孤儿组」才发出——整份文档都没分组时
        // 那是常态，不该拿告警吓人（组名由文件名兜底，见 parse_named）。
        if !seen_group_header && orphan_warning.is_none() {
            orphan_warning = Some(ParseWarning {
                line,
                message: format!(
                    "此行起的正文出现在第一个分组标记之前，已单独归入「{DEFAULT_GROUP}」。\
                     可在下方改名，或用「并入下一组」合掉。"
                ),
            });
        }
        ensure(&mut cur).prompts.push(ParsedPrompt {
            title: pending_title.take().map(|(t, _)| t),
            text: strip_leading_number(trimmed).to_string(),
        });
    }

    // 文件结尾仍有未挂靠的小标题。
    warn_dangling(&mut warnings, pending_title.take());

    if let Some(g) = cur.take() {
        groups.push(g);
    }
    // 推断出来却一条正文都没管到的分组，说明这行八成本来就是条（短）提示词——
    // 把它还回相邻分组的正文里，绝不因为猜错而丢内容。
    salvage_empty_inferred(&mut groups);
    // 丢弃完全为空（无提示词）的分组。
    groups.retain(|g| !g.prompts.is_empty());
    // 孤儿组告警：仅当它旁边确实还有别的分组时才有意义。
    if let Some(w) = orphan_warning {
        if groups.len() > 1 {
            warnings.push(w);
        }
    }
    warnings.sort_by_key(|w| w.line);
    (groups, warnings)
}

/// 把「推断出来但没管到任何正文」的分组名还原成提示词，挂到后一个（没有则前一个）分组上。
/// 保证形态推断在猜错时最多是「分组分歧」，不会静默吞掉一条提示词。
fn salvage_empty_inferred(groups: &mut Vec<ParsedGroup>) {
    let mut orphans: Vec<ParsedPrompt> = Vec::new();
    let mut kept: Vec<ParsedGroup> = Vec::with_capacity(groups.len());
    for g in groups.drain(..) {
        if g.origin == GroupOrigin::Inferred && g.prompts.is_empty() {
            orphans.push(ParsedPrompt {
                title: None,
                text: g.name,
            });
            continue;
        }
        let mut g = g;
        if !orphans.is_empty() {
            let mut merged = std::mem::take(&mut orphans);
            merged.append(&mut g.prompts);
            g.prompts = merged;
        }
        kept.push(g);
    }
    if !orphans.is_empty() {
        match kept.last_mut() {
            Some(last) => last.prompts.append(&mut orphans),
            // 全文只推断出空组：退回单个默认组，内容一条不落。
            None => kept.push(ParsedGroup {
                name: DEFAULT_GROUP.to_string(),
                prompts: orphans,
                origin: GroupOrigin::Fallback,
                ..Default::default()
            }),
        }
    }
    *groups = kept;
}

/// 一行是否为「正文」：既非显式分组头、亦非元信息头、亦非裸括号块。
fn is_body_line(line: &str) -> bool {
    parse_group_header(line).is_none()
        && parse_header(line).is_none()
        && parse_bracket_line(line).is_none()
}

/// 识别分组头：关键字 `分组`/`组`/`group`（大小写不敏感）后接冒号(半/全角)、
/// 短横线/等号、内联括号或空白分隔。例：`分组: 名称`·`分组-名称`·`分组【名称】`·`分组 名称`。
fn parse_group_header(line: &str) -> Option<String> {
    // 长关键字在前，`组` 最后（单字，仅允许显式分隔，不吃空白）。
    for kw in ["分组", "组", "group"] {
        let Some(rest) = strip_prefix_ci(line, kw) else {
            continue;
        };
        let rest_trimmed = rest.trim_start();
        // 内联括号形式：分组【名称】 / 分组 【名称】。
        if let Some(inner) = bracket_inner(rest_trimmed) {
            let name = inner.trim();
            if !name.is_empty() {
                return Some(name.to_string());
            }
            continue;
        }
        // 冒号 / 短横线 / 等号分隔（半/全角）：分组：名称 · 分组-名称 · 分组＝名称。
        if let Some(after) = rest_trimmed.strip_prefix([':', '：', '-', '－', '—', '=', '＝'])
        {
            let name = after.trim();
            if !name.is_empty() {
                return Some(name.to_string());
            }
            continue;
        }
        // 纯空白分隔：分组 名称（仅 ≥2 字关键字，避免单字 `组` 误伤以「组」起头的正文）。
        if kw.chars().count() >= 2 && rest.starts_with(|c: char| c.is_whitespace()) {
            let name = rest.trim();
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    None
}

/// 前缀匹配：ASCII 关键字大小写不敏感，非 ASCII 关键字精确匹配。返回剥离关键字后的剩余串。
fn strip_prefix_ci<'a>(line: &'a str, kw: &str) -> Option<&'a str> {
    if kw.is_ascii() {
        let n = kw.len();
        (line.is_char_boundary(n) && line.as_bytes()[..n].eq_ignore_ascii_case(kw.as_bytes()))
            .then(|| &line[n..])
    } else {
        line.strip_prefix(kw)
    }
}

/// 支持的成对括号（宽泛识别常见标题/分组括号；不含 `「」`/`《》` 以免误伤正文引用）。
const BRACKET_PAIRS: &[(char, char)] = &[('【', '】'), ('[', ']'), ('［', '］'), ('〖', '〗')];

/// 取内联括号块的内部文本（要求 `s` 以某个开括号开头，返回到对应闭括号前的内容）。
fn bracket_inner(s: &str) -> Option<&str> {
    for &(open, close) in BRACKET_PAIRS {
        if let Some(rest) = s.strip_prefix(open) {
            if let Some(end) = rest.find(close) {
                return Some(&rest[..end]);
            }
        }
    }
    None
}

/// 若整行恰是单个成对括号块 `【...】`/`[...]`/`［...］`/`〖...〗`，返回括号内文本。
/// 内部若再含闭括号（非单一块，疑似正文），或内容为空 → 不识别。
fn parse_bracket_line(line: &str) -> Option<String> {
    for &(open, close) in BRACKET_PAIRS {
        if let Some(rest) = line.strip_prefix(open) {
            if let Some(inner) = rest.strip_suffix(close) {
                if inner.contains(close) {
                    return None;
                }
                let t = inner.trim();
                return (!t.is_empty()).then(|| t.to_string());
            }
        }
    }
    None
}

/// 剥离正文前导序号，宽泛覆盖常见写法：
/// - `1.` / `2、` / `3）` / `4．` / `5:` / `6,`（数字 + 分隔符）；
/// - `(7)` / `（8）`（括号包裹的数字）；
/// - `①`…`⑳` 圆圈数字（后可再跟分隔符）。
///
/// 无法识别为序号时原样返回。
fn strip_leading_number(s: &str) -> &str {
    // 圆圈数字：①-⑳ (U+2460..=U+2473)。
    if let Some(first) = s.chars().next() {
        if ('\u{2460}'..='\u{2473}').contains(&first) {
            return strip_leading_separator(&s[first.len_utf8()..]);
        }
    }

    // 可选前导开括号：(1) / （1）。
    let (had_paren, body) = match s.strip_prefix('(').or_else(|| s.strip_prefix('（')) {
        Some(rest) => (true, rest),
        None => (false, s),
    };

    let digits_end = body
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(body.len());
    if digits_end == 0 {
        return s; // 无前导数字
    }
    let after = &body[digits_end..];
    match after.chars().next() {
        // 括号包裹：吃掉闭括号（后面可能还紧跟别的分隔符，如「(1). 」）。
        Some(c @ (')' | '）')) if had_paren => strip_leading_separator(&after[c.len_utf8()..]),
        // 普通分隔符：数字后紧跟标点。
        Some(c)
            if matches!(
                c,
                '.' | '．' | '、' | '。' | ')' | '）' | ':' | '：' | ',' | '，'
            ) =>
        {
            after[c.len_utf8()..].trim_start()
        }
        _ => s,
    }
}

/// 若字符串以一个序号分隔符开头则吃掉它，再去掉随后空白；否则仅去掉前导空白。
fn strip_leading_separator(s: &str) -> &str {
    match s.chars().next() {
        Some(c)
            if matches!(
                c,
                '.' | '．' | '、' | '。' | ')' | '）' | ':' | '：' | ',' | '，'
            ) =>
        {
            s[c.len_utf8()..].trim_start()
        }
        _ => s.trim_start(),
    }
}

enum Header {
    Prefix,
    Scene,
    Tags,
}

/// 识别元信息头部行 `键: 值`（半/全角冒号）。分组头另由 `parse_group_header` 处理。
fn parse_header(line: &str) -> Option<(Header, &str)> {
    let idx = line.find([':', '：'])?;
    let key = line[..idx].trim();
    // 冒号可能是全角（3 字节），用 char 边界安全切分。
    let value = line[idx..]
        .char_indices()
        .nth(1)
        .map(|(off, _)| &line[idx + off..])
        .unwrap_or("")
        .trim();
    let header = match key {
        "前缀" | "prefix" | "Prefix" => Header::Prefix,
        "场景" | "scene" | "Scene" => Header::Scene,
        "标签" | "tag" | "tags" | "Tags" => Header::Tags,
        _ => return None,
    };
    Some((header, value))
}

/// 投单组头键（只在组头区识别，见解析主循环里的说明）。
enum IntakeHeader {
    Refs,
    Ratio,
    Size,
    Format,
    Draws,
    Purpose,
}

/// 识别投单组头行 `键: 值`。与 [`parse_header`] 分开，因为两者的**作用域不同**。
fn parse_intake_header(line: &str) -> Option<(IntakeHeader, &str)> {
    let idx = line.find([':', '：'])?;
    let key = line[..idx].trim();
    let value = line[idx..]
        .char_indices()
        .nth(1)
        .map(|(off, _)| &line[idx + off..])
        .unwrap_or("")
        .trim();
    let header = match key {
        "参考图" | "参考图片" | "refs" | "ref" | "images" => IntakeHeader::Refs,
        "比例" | "画幅" | "ratio" | "aspect" => IntakeHeader::Ratio,
        "尺寸" | "size" => IntakeHeader::Size,
        "格式" | "输出格式" | "format" => IntakeHeader::Format,
        "抽卡" | "抽卡次数" | "draws" => IntakeHeader::Draws,
        "用途" | "purpose" | "purposes" => IntakeHeader::Purpose,
        _ => return None,
    };
    Some((header, value))
}

/// 列表分隔：只按 `, ，、; ；` 切，**不按空白**。
///
/// 与 [`split_tags`] 的区别就在这里：参考图是路径，`images/黄 A.jpg` 里的空格是文件名的一部分，
/// 按空白切会把一个路径切成两个都不存在的路径。
fn split_list(value: &str) -> Vec<String> {
    value
        .split([',', '，', '、', ';', '；'])
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

/// 标签分隔：`, ，、; ；` 与空白。
fn split_tags(value: &str) -> Vec<String> {
    value
        .split(|c: char| matches!(c, ',' | '，' | '、' | ';' | '；') || c.is_whitespace())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即失败
mod tests {
    use super::*;

    /// 测试便捷：不带文件名的解析（生产路径一律走 `parse_named`）。
    fn parse(bytes: &[u8]) -> ParsedImport {
        parse_named(bytes, None)
    }

    /// 测试便捷：把 ParsedPrompt 拍平成 (title, text) 便于断言。
    fn flat(g: &ParsedGroup) -> Vec<(Option<&str>, &str)> {
        g.prompts
            .iter()
            .map(|p| (p.title.as_deref(), p.text.as_str()))
            .collect()
    }
    fn texts(g: &ParsedGroup) -> Vec<&str> {
        g.prompts.iter().map(|p| p.text.as_str()).collect()
    }

    #[test]
    fn parses_full_utf8_document() {
        let doc = "\
分组: 电商主图
前缀: dz
场景: 商品
标签: 白底, 3C、主图

第一条提示词正文。
第二条提示词正文。
";
        let out = parse(doc.as_bytes());
        assert_eq!(out.groups.len(), 1);
        let g = &out.groups[0];
        assert_eq!(g.name, "电商主图");
        assert_eq!(g.prefix.as_deref(), Some("DZ")); // 前缀大写归一
        assert_eq!(g.scene, "商品");
        assert_eq!(g.tags, vec!["白底", "3C", "主图"]);
        assert_eq!(texts(g), vec!["第一条提示词正文。", "第二条提示词正文。"]);
        // 无小标题行时 title 为 None。
        assert!(g.prompts.iter().all(|p| p.title.is_none()));
    }

    #[test]
    fn missing_group_falls_back_to_default() {
        let out = parse("只有一条正文，没有分组头。".as_bytes());
        assert_eq!(out.groups.len(), 1);
        assert_eq!(out.groups[0].name, DEFAULT_GROUP);
        assert_eq!(out.groups[0].prefix, None);
        assert_eq!(out.groups[0].prompts.len(), 1);
    }

    #[test]
    fn missing_tags_and_empty_lines_ok() {
        let doc = "分组: A\n\n\n正文1\n\n\n正文2\n\n";
        let out = parse(doc.as_bytes());
        assert_eq!(out.groups[0].tags.len(), 0);
        assert_eq!(texts(&out.groups[0]), vec!["正文1", "正文2"]);
    }

    #[test]
    fn each_nonempty_line_is_one_prompt() {
        let doc = "分组: A\n第一行\n第二行\n第三行\n\n下一条";
        let out = parse(doc.as_bytes());
        assert_eq!(
            texts(&out.groups[0]),
            vec!["第一行", "第二行", "第三行", "下一条"]
        );
    }

    #[test]
    fn very_long_prompt_over_500_chars() {
        let long = "光".repeat(600);
        let doc = format!("分组: A\n{long}");
        let out = parse(doc.as_bytes());
        assert_eq!(out.groups[0].prompts[0].text.chars().count(), 600);
    }

    #[test]
    fn multiple_groups() {
        let doc = "分组: A\n前缀: AA\na1\n\n分组: B\n前缀: BB\nb1\nb2扩展\n\nb3";
        let out = parse(doc.as_bytes());
        assert_eq!(out.groups.len(), 2);
        assert_eq!(out.groups[0].name, "A");
        assert_eq!(texts(&out.groups[0]), vec!["a1"]);
        assert_eq!(out.groups[1].name, "B");
        assert_eq!(texts(&out.groups[1]), vec!["b1", "b2扩展", "b3"]);
    }

    #[test]
    fn decodes_gbk() {
        // "分组: 商品\n正文一" 的 GBK 字节
        let (bytes, _enc, _had_errors) = encoding_rs::GBK.encode("分组: 商品\n正文一");
        let out = parse(&bytes);
        assert_eq!(out.groups[0].name, "商品");
        assert_eq!(texts(&out.groups[0]), vec!["正文一"]);
    }

    #[test]
    fn fullwidth_colon_supported() {
        let out = parse("分组：全角\n正文".as_bytes());
        assert_eq!(out.groups[0].name, "全角");
    }

    #[test]
    fn bracket_group_header_and_titles() {
        // 用户实际格式：分组【名】 + 每条上方独占一行的【小标题】 + 带前导序号的正文。
        let doc = "\
分组【丁禹兮】

【雷雨拆快递】
1. 把图中这只卡套放进随手拍照片里。

【楼道骑车】
2. 把绿色卡套连同配件放进照片里。
";
        let out = parse(doc.as_bytes());
        assert_eq!(out.groups.len(), 1);
        let g = &out.groups[0];
        assert_eq!(g.name, "丁禹兮");
        assert_eq!(
            flat(g),
            vec![
                (Some("雷雨拆快递"), "把图中这只卡套放进随手拍照片里。"),
                (Some("楼道骑车"), "把绿色卡套连同配件放进照片里。"),
            ]
        );
    }

    #[test]
    fn multiple_bracket_groups_split() {
        let doc = "\
分组【组一】
【标题A】
1. 甲正文
分组【组二】
【标题B】
1、乙正文
【标题C】
2）丙正文
";
        let out = parse(doc.as_bytes());
        assert_eq!(out.groups.len(), 2);
        assert_eq!(out.groups[0].name, "组一");
        assert_eq!(flat(&out.groups[0]), vec![(Some("标题A"), "甲正文")]);
        assert_eq!(out.groups[1].name, "组二");
        assert_eq!(
            flat(&out.groups[1]),
            vec![(Some("标题B"), "乙正文"), (Some("标题C"), "丙正文")]
        );
    }

    #[test]
    fn title_without_number_and_prompt_without_title() {
        let doc = "分组【G】\n【只有标题】\n没有序号的正文\n没有标题也没序号的正文";
        let out = parse(doc.as_bytes());
        assert_eq!(
            flat(&out.groups[0]),
            vec![
                (Some("只有标题"), "没有序号的正文"),
                (None, "没有标题也没序号的正文"),
            ]
        );
    }

    #[test]
    fn embedded_brackets_in_body_not_treated_as_title() {
        // 正文中含括号但非整行括号块 → 不误判为小标题。
        let doc = "分组【G】\n把【绿色】卡套放进照片";
        let out = parse(doc.as_bytes());
        assert_eq!(flat(&out.groups[0]), vec![(None, "把【绿色】卡套放进照片")]);
    }

    // 用户真实格式：**裸括号**分组头（无「分组」关键字）+ 小标题 + 带全角点序号正文。
    // 结构：`【分组】` 空行 `【小标题】` `1．正文` …（图 1 卡套提示词批次）。
    #[test]
    fn bare_bracket_group_is_detected() {
        let doc = "\
【时代少年团场景图】

【画室调色台】
1．把图中这只印有男团合照的浅绿色证件卡套放进照片。

【天台画速写】
2．把图中这只卡套连同配件放进照片。
";
        let out = parse(doc.as_bytes());
        assert_eq!(out.groups.len(), 1, "裸括号应识别为分组而非小标题");
        let g = &out.groups[0];
        assert_eq!(g.name, "时代少年团场景图");
        assert_eq!(
            flat(g),
            vec![
                (
                    Some("画室调色台"),
                    "把图中这只印有男团合照的浅绿色证件卡套放进照片。"
                ),
                (Some("天台画速写"), "把图中这只卡套连同配件放进照片。"),
            ]
        );
        // 分组已识别 → 无「正文出现在分组标记前」告警。
        assert!(out.warnings.is_empty(), "不应再落入默认分组并告警");
    }

    // 多个裸括号分组按其后是否紧跟另一括号自动拆分。
    #[test]
    fn multiple_bare_bracket_groups_split() {
        let doc = "\
【场景一】
【标题A】
1．甲正文
【场景二】
【标题B】
2．乙正文
";
        let out = parse(doc.as_bytes());
        assert_eq!(out.groups.len(), 2);
        assert_eq!(out.groups[0].name, "场景一");
        assert_eq!(flat(&out.groups[0]), vec![(Some("标题A"), "甲正文")]);
        assert_eq!(out.groups[1].name, "场景二");
        assert_eq!(flat(&out.groups[1]), vec![(Some("标题B"), "乙正文")]);
        assert!(out.warnings.is_empty());
    }

    // 裸括号分组下紧跟正文（无小标题）：仍识别为分组。
    #[test]
    fn bare_bracket_group_then_direct_body() {
        // 【组】下一行是元信息头 → 判为分组；再下面才是正文。
        let doc = "【只有一个分组】\n前缀: ab\n直接正文一\n直接正文二";
        let out = parse(doc.as_bytes());
        assert_eq!(out.groups.len(), 1);
        assert_eq!(out.groups[0].name, "只有一个分组");
        assert_eq!(out.groups[0].prefix.as_deref(), Some("AB"));
        assert_eq!(texts(&out.groups[0]), vec!["直接正文一", "直接正文二"]);
    }

    // 方括号 / 全角方括号 / 六角括号亦可作分组与小标题。
    #[test]
    fn alternative_bracket_styles() {
        let doc = "[场景]\n［标题一］\n1. 正文一\n〖标题二〗\n2. 正文二";
        let out = parse(doc.as_bytes());
        assert_eq!(out.groups.len(), 1);
        assert_eq!(out.groups[0].name, "场景");
        assert_eq!(
            flat(&out.groups[0]),
            vec![(Some("标题一"), "正文一"), (Some("标题二"), "正文二")]
        );
    }

    // 用户真实格式：`【分组：名称】` —— 把「分组：」一起括进方括号，且分组下直接跟正文（无小标题）。
    // 旧逻辑因前瞻到下一行是正文而误判为小标题，全部塌进默认分组；此处应正确按分组拆分。
    #[test]
    fn bracket_wrapped_group_header_with_direct_body() {
        let doc = "\
【分组：鹿晗】

奶油蜂蜜生日派对餐桌正文。
柠檬汽水气泡池正文。

【分组：鞠婧祎】

玻璃汽水瓶瓶底寻宝正文。
";
        let out = parse(doc.as_bytes());
        assert_eq!(out.groups.len(), 2, "括号包裹的分组头应按分组拆分");
        assert_eq!(out.groups[0].name, "鹿晗");
        assert_eq!(
            texts(&out.groups[0]),
            vec!["奶油蜂蜜生日派对餐桌正文。", "柠檬汽水气泡池正文。"]
        );
        assert_eq!(out.groups[1].name, "鞠婧祎");
        assert_eq!(texts(&out.groups[1]), vec!["玻璃汽水瓶瓶底寻宝正文。"]);
        // 分组已识别 → 不应再落默认分组并告警。
        assert!(out.warnings.is_empty(), "不应误判为默认分组");
    }

    // 分组头分隔符更宽泛：空白 / 短横线 / 等号 / 大小写不敏感 group / 括号包裹的短横线形式。
    #[test]
    fn flexible_group_header_separators() {
        assert_eq!(parse_group_header("分组 鹿晗").as_deref(), Some("鹿晗"));
        assert_eq!(parse_group_header("分组-鞠婧祎").as_deref(), Some("鞠婧祎"));
        assert_eq!(
            parse_group_header("分组＝邓紫棋").as_deref(),
            Some("邓紫棋")
        );
        assert_eq!(parse_group_header("GROUP: alpha").as_deref(), Some("alpha"));
        assert_eq!(parse_group_header("Group【beta】").as_deref(), Some("beta"));
        // 括号包裹的关键字（含短横线分隔）。
        let out = parse("[组-甲]\n正文一\n正文二".as_bytes());
        assert_eq!(out.groups.len(), 1);
        assert_eq!(out.groups[0].name, "甲");
        assert_eq!(texts(&out.groups[0]), vec!["正文一", "正文二"]);
    }

    // 防误伤：单字 `组` 不吃空白分隔；以「组」「分组」起头的正文不被当作分组头。
    #[test]
    fn group_header_does_not_swallow_body() {
        assert_eq!(parse_group_header("组 图排版说明"), None);
        assert_eq!(parse_group_header("组合成一张图"), None);
        assert_eq!(parse_group_header("分组内的说明文字"), None);
        // `组：X` 单字关键字 + 显式冒号仍识别。
        assert_eq!(parse_group_header("组：临时").as_deref(), Some("临时"));
    }

    // 序号剥离：括号数字 `(1)`/`（2）` 与圆圈数字 `③`。
    #[test]
    fn strips_paren_and_circled_numbers() {
        assert_eq!(strip_leading_number("(1) 甲"), "甲");
        assert_eq!(strip_leading_number("（2）乙"), "乙");
        assert_eq!(strip_leading_number("③丙"), "丙");
        assert_eq!(strip_leading_number("④、丁"), "丁");
        assert_eq!(strip_leading_number("5，戊"), "戊");
        // 非序号：括号内非纯数字、或无分隔符时原样保留。
        assert_eq!(strip_leading_number("(a) 保留"), "(a) 保留");
        assert_eq!(strip_leading_number("2024 年不是序号"), "2024 年不是序号");
    }

    // E37：正文出现在任何「分组」标记前 → 告警含首个正文行号。
    #[test]
    fn warns_content_before_group_with_line() {
        // 第 1~2 行为空/说明，第 3 行是缺分组标记的正文。
        let doc = "\n\n没有分组头的正文\n分组: A\n正文A";
        let out = parse(doc.as_bytes());
        assert_eq!(out.warnings.len(), 1);
        assert_eq!(out.warnings[0].line, 3, "告警指向第 3 行");
        assert!(out.warnings[0].message.contains("分组"));
        // 正文仍被容错归入默认组 + A 组。
        assert_eq!(out.total_prompts(), 2);
    }

    // E37：小标题后无正文（文件结尾悬空）→ 告警含小标题行号。
    #[test]
    fn warns_dangling_title_at_eof() {
        let doc = "分组: A\n正文1\n【结尾悬空标题】";
        let out = parse(doc.as_bytes());
        assert_eq!(out.warnings.len(), 1);
        assert_eq!(out.warnings[0].line, 3);
        assert!(out.warnings[0].message.contains("结尾悬空标题"));
    }

    // 正常文档无告警。
    #[test]
    fn clean_document_has_no_warnings() {
        let out = parse("分组: A\n前缀: AA\n【标题】\n正文".as_bytes());
        assert!(out.warnings.is_empty());
    }

    /// 造一条「长段落正文」，长度远超形态推断的中位数门槛。
    fn long_body(tag: &str) -> String {
        format!(
            "参考图中的卡套挂件产品完整原样保留，{}{}",
            tag,
            "光影自然。".repeat(30)
        )
    }

    // 用户真实文件（B-Roll 素材分镜提示词）：首行一个裸标题，下面全是长段落，没有任何分组标记。
    // 旧行为：整份塌进「未分组导入」并告警「请在文件开头加一行 分组:」。现在应推断出分组名。
    #[test]
    fn plain_heading_over_long_bodies_is_inferred_group() {
        let mut doc = String::from("鹿晗-B-Roll素材分镜图\n\n");
        for i in 0..5 {
            doc.push_str(&long_body(&format!("第{i}帧")));
            doc.push_str("\n\n");
        }
        let out = parse(doc.as_bytes());
        assert_eq!(out.groups.len(), 1);
        assert_eq!(out.groups[0].name, "鹿晗-B-Roll素材分镜图");
        assert_eq!(out.groups[0].origin, GroupOrigin::Inferred, "应标为疑似");
        assert_eq!(out.groups[0].prompts.len(), 5, "标题行不得混进正文");
        assert!(out.warnings.is_empty(), "认出来了就不该再告警");
    }

    // 多个裸标题各自管一批长正文 → 按标题拆分多组。
    #[test]
    fn multiple_plain_headings_split_groups() {
        let doc = format!(
            "鹿晗\n{}\n{}\n\n鞠婧祎\n{}\n{}\n{}\n",
            long_body("a"),
            long_body("b"),
            long_body("c"),
            long_body("d"),
            long_body("e"),
        );
        let out = parse(doc.as_bytes());
        assert_eq!(out.groups.len(), 2);
        assert_eq!(out.groups[0].name, "鹿晗");
        assert_eq!(out.groups[0].prompts.len(), 2);
        assert_eq!(out.groups[1].name, "鞠婧祎");
        assert_eq!(out.groups[1].prompts.len(), 3);
    }

    // 短标题与长正文 1:1 交替 → 是小标题而非分组（管辖条数才是判层依据）。
    #[test]
    fn plain_heading_governing_one_body_is_a_title() {
        let doc = format!(
            "奶茶店午后\n{}\n\n画室调色台\n{}\n",
            long_body("a"),
            long_body("b")
        );
        let out = parse(doc.as_bytes());
        assert_eq!(out.groups.len(), 1, "1:1 交替不应拆成分组");
        assert_eq!(out.groups[0].origin, GroupOrigin::Fallback);
        let titles: Vec<_> = out.groups[0]
            .prompts
            .iter()
            .map(|p| p.title.as_deref())
            .collect();
        assert_eq!(titles, vec![Some("奶茶店午后"), Some("画室调色台")]);
    }

    // 顶层分组 + 其下 1:1 小标题：两层结构都认。
    #[test]
    fn plain_heading_two_levels() {
        let doc = format!(
            "鹿晗-B-Roll素材分镜图\n奶茶店午后\n{}\n画室调色台\n{}\n",
            long_body("a"),
            long_body("b")
        );
        let out = parse(doc.as_bytes());
        assert_eq!(out.groups.len(), 1);
        assert_eq!(out.groups[0].name, "鹿晗-B-Roll素材分镜图");
        assert_eq!(out.groups[0].origin, GroupOrigin::Inferred);
        assert_eq!(
            flat(&out.groups[0]),
            vec![
                (Some("奶茶店午后"), long_body("a").as_str()),
                (Some("画室调色台"), long_body("b").as_str()),
            ]
        );
    }

    // 防误吃：短正文文档（无长段落）不启用形态推断，短行仍是一条条提示词。
    #[test]
    fn short_body_document_is_not_reinterpreted() {
        let doc = "白底商品正面\n商品材质细节特写\n楼道骑行随手拍\n天台画速写";
        let out = parse(doc.as_bytes());
        assert_eq!(out.groups.len(), 1);
        assert_eq!(out.groups[0].prompts.len(), 4, "短文档每行仍是一条提示词");
        assert_eq!(out.groups[0].origin, GroupOrigin::Fallback);
    }

    // 防误吃：文档一旦有显式分组标记，就完全听它的，不再做形态推断。
    #[test]
    fn explicit_marker_disables_heuristics() {
        let doc = format!(
            "分组: 正式组\n短行也是一条正文\n{}\n{}\n{}\n",
            long_body("a"),
            long_body("b"),
            long_body("c")
        );
        let out = parse(doc.as_bytes());
        assert_eq!(out.groups.len(), 1);
        assert_eq!(out.groups[0].name, "正式组");
        assert_eq!(out.groups[0].origin, GroupOrigin::Explicit);
        assert_eq!(out.groups[0].prompts.len(), 4, "短行不得被吃成标题");
    }

    // 裸括号 + 其下多条正文（此前会被判成小标题，把正文全塞进默认组）。
    #[test]
    fn bare_bracket_governing_many_bodies_is_group() {
        let doc = "【时代少年团场景图】\n甲正文\n乙正文\n丙正文";
        let out = parse(doc.as_bytes());
        assert_eq!(out.groups.len(), 1);
        assert_eq!(out.groups[0].name, "时代少年团场景图");
        assert_eq!(texts(&out.groups[0]), vec!["甲正文", "乙正文", "丙正文"]);
        assert!(out.warnings.is_empty());
    }

    // 长短混排、其实没有标题的文档：推断可能猜错分层，但**一条都不能丢**。
    #[test]
    fn misjudged_short_lines_are_salvaged_not_dropped() {
        let doc = format!(
            "白底商品正面\n商品材质细节特写\n楼道骑行随手拍\n{}\n{}\n",
            long_body("a"),
            long_body("b")
        );
        let out = parse(doc.as_bytes());
        let all: Vec<&str> = out
            .groups
            .iter()
            .flat_map(|g| g.prompts.iter().map(|p| p.text.as_str()))
            .collect();
        // 最多一行被当成了组名，其余全部作为提示词保留。
        assert!(all.contains(&"白底商品正面"), "短行不得被吞掉：{all:?}");
        assert!(all.contains(&"商品材质细节特写"), "短行不得被吞掉：{all:?}");
        assert_eq!(out.total_prompts(), 4);
    }

    // 文件名兜底：文档确实没线索时，用文件名而不是「未分组导入」。
    #[test]
    fn file_stem_names_the_fallback_group() {
        let out = parse_named(
            "只有一条正文。".as_bytes(),
            Some("B-Roll素材分镜提示词_20260724"),
        );
        assert_eq!(out.groups[0].name, "B-Roll素材分镜提示词");
        assert_eq!(out.groups[0].origin, GroupOrigin::Fallback);
        assert_eq!(out.warnings.len(), 1);
        assert_eq!(out.warnings[0].line, 0, "非行内问题用 0 表示无行号");
    }

    // 文件名兜底只作用于「无线索」的默认组，认出来的分组名不被覆盖。
    #[test]
    fn file_stem_does_not_override_detected_group() {
        let out = parse_named("分组: 真名\n正文".as_bytes(), Some("随便什么文件名"));
        assert_eq!(out.groups[0].name, "真名");
        assert!(out.warnings.is_empty());
    }

    #[test]
    fn cleans_file_stem_noise() {
        assert_eq!(clean_file_stem("提示词_20260724"), "提示词");
        assert_eq!(clean_file_stem("提示词 2026-07-24"), "提示词");
        assert_eq!(clean_file_stem("提示词 (1)"), "提示词");
        assert_eq!(clean_file_stem("提示词 副本"), "提示词");
        assert_eq!(
            clean_file_stem("B-Roll素材分镜提示词"),
            "B-Roll素材分镜提示词"
        );
        // 不把名字吃光：纯数字文件名原样保留。
        assert_eq!(clean_file_stem("20260724"), "20260724");
    }

    #[test]
    fn arbitrary_bytes_do_not_panic() {
        // 属性式冒烟：随机字节不 panic（round-trip 安全）。
        for seed in 0u32..64 {
            let bytes: Vec<u8> = (0..128)
                .map(|i| ((seed.wrapping_mul(31) + i) % 256) as u8)
                .collect();
            let _ = parse(&bytes);
        }
    }

    // ───────── 投单组头（参考图 / 比例 / 抽卡 / 用途 / 尺寸 / 格式） ─────────

    // 标准产出格式：组头带挂靠与参数，正文照旧一段一条。
    #[test]
    fn intake_headers_are_parsed_on_the_group() {
        let doc = "\
分组: 黄色系卡套
前缀: KT
参考图: images/黄A.jpg, images/黄B.jpg
比例: 3:4
抽卡: 2
用途: 图生视频

第一条完整提示词正文

第二条完整提示词正文
";
        let out = parse(doc.as_bytes());
        assert_eq!(out.groups.len(), 1);
        let g = &out.groups[0];
        assert_eq!(g.name, "黄色系卡套");
        assert_eq!(g.prefix.as_deref(), Some("KT"));
        assert_eq!(g.refs, vec!["images/黄A.jpg", "images/黄B.jpg"]);
        assert_eq!(g.ratio.as_deref(), Some("3:4"));
        assert_eq!(g.draws, Some(2));
        // 用途并进 tags → 下游按「显式用途」处理，不必再猜。
        assert!(g.tags.contains(&"图生视频".to_string()));
        // 组头一行都不该被当成提示词。
        assert_eq!(
            texts(g),
            vec!["第一条完整提示词正文", "第二条完整提示词正文"]
        );
    }

    // 每组各自的挂靠与比例（收件侧据此拆多个批次）。
    #[test]
    fn intake_headers_are_per_group() {
        let doc = "\
分组: 黄
参考图: images/黄.jpg
比例: 3:4

黄组正文

分组: 蓝
参考图: images/蓝.jpg
比例: 9:16

蓝组正文
";
        let out = parse(doc.as_bytes());
        assert_eq!(out.groups.len(), 2);
        assert_eq!(out.groups[0].refs, vec!["images/黄.jpg"]);
        assert_eq!(out.groups[0].ratio.as_deref(), Some("3:4"));
        assert_eq!(out.groups[1].refs, vec!["images/蓝.jpg"]);
        assert_eq!(out.groups[1].ratio.as_deref(), Some("9:16"));
    }

    // **本次最要紧的一条**：正文里以「比例：」开头的一句，不得被当成组头吃掉。
    // 这些文档是长叙事提示词，被吃掉的那条会静悄悄少一句话。
    #[test]
    fn intake_header_shaped_body_line_is_not_swallowed() {
        let doc = "\
分组: A
比例: 3:4

第一条正文

比例：3:4 的竖构图，主体居中，这其实是一整条提示词

第三条正文
";
        let out = parse(doc.as_bytes());
        let g = &out.groups[0];
        assert_eq!(g.ratio.as_deref(), Some("3:4"), "组头区那行仍是参数");
        assert_eq!(
            texts(g),
            vec![
                "第一条正文",
                "比例：3:4 的竖构图，主体居中，这其实是一整条提示词",
                "第三条正文"
            ],
            "正文区里长得像组头的行必须原样留在正文里"
        );
    }

    // 路径含空格：只按逗号切，不按空白切（按空白切会切出两个都不存在的路径）。
    #[test]
    fn ref_paths_may_contain_spaces() {
        let doc = "分组: A\n参考图: images/黄 A.jpg, images/蓝 B.jpg\n\n正文";
        let out = parse(doc.as_bytes());
        assert_eq!(
            out.groups[0].refs,
            vec!["images/黄 A.jpg", "images/蓝 B.jpg"]
        );
    }

    // 抽卡写了非数字不该让整份文档失败：当作没写（后续按默认 1 走）。
    #[test]
    fn bad_draws_value_degrades_to_none() {
        let doc = "分组: A\n抽卡: 两次\n\n正文";
        assert_eq!(parse(doc.as_bytes()).groups[0].draws, None);
    }

    // 没有任何投单组头的老文档：新字段全空，行为与从前一致。
    #[test]
    fn legacy_docs_get_empty_intake_fields() {
        let doc = "分组: A\n前缀: AA\n\n正文一\n\n正文二";
        let g = &parse(doc.as_bytes()).groups[0];
        assert!(g.refs.is_empty() && g.ratio.is_none() && g.draws.is_none());
        assert_eq!(texts(g), vec!["正文一", "正文二"]);
    }
}
