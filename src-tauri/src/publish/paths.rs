//! 四分区常量、`RelPath` newtype、ASCII 规范化、导出路径拼接（发布模块执行计划 §1.1/1.2/§5.1）。
//!
//! 相对路径是真相：库内与包内只存根目录内相对路径；绝对路径只在 IO 与导出两处出现。
//! ASCII 规则只约束「变量名」（SKU 编码 / 任务 ID / 素材文件名）；四分区目录名与包内固定名
//! 按需求文档保持中文/固定拼写——它们是**常量**，集中于此，代码中禁止字面量散写。

// 部分拼接/常量先于 P2/P3 消费者落地；未使用项在对应任务接入后收紧。
#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use specta::Type;

/// 以 create_new 语义拷贝，目标已存在时拒绝覆盖。写入中途失败会清掉半文件。
pub fn copy_new(source: &std::path::Path, destination: &std::path::Path) -> std::io::Result<u64> {
    let mut input = std::fs::File::open(source)?;
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    match std::io::copy(&mut input, &mut output) {
        Ok(bytes) => Ok(bytes),
        Err(error) => {
            drop(output);
            let _ = std::fs::remove_file(destination);
            Err(error)
        }
    }
}

/// 批量 create_new 拷贝。任一项失败时回滚本批已创建文件，不留下无数据库记录的孤儿。
pub fn copy_batch_new(jobs: &[(std::path::PathBuf, std::path::PathBuf)]) -> std::io::Result<()> {
    let mut created = Vec::new();
    for (source, destination) in jobs {
        let result = if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).and_then(|()| copy_new(source, destination).map(|_| ()))
        } else {
            copy_new(source, destination).map(|_| ())
        };
        if let Err(error) = result {
            for path in created.into_iter().rev() {
                let _ = std::fs::remove_file(path);
            }
            return Err(error);
        }
        created.push(destination.clone());
    }
    Ok(())
}

/// 文件已复制、数据库尚未提交时的补偿守卫。提交成功后逐个 `preserve`；任何 `?`
/// 提前返回都会删除仍无主的文件。
pub struct CreatedFilesGuard {
    paths: Vec<std::path::PathBuf>,
}

impl CreatedFilesGuard {
    pub fn new(paths: impl IntoIterator<Item = std::path::PathBuf>) -> Self {
        Self {
            paths: paths.into_iter().collect(),
        }
    }

    pub fn preserve(&mut self, path: &std::path::Path) {
        self.paths.retain(|candidate| candidate != path);
    }

    pub fn preserve_all(&mut self) {
        self.paths.clear();
    }
}

impl Drop for CreatedFilesGuard {
    fn drop(&mut self) {
        for path in self.paths.iter().rev() {
            let _ = std::fs::remove_file(path);
        }
    }
}

// ── 四分区目录名（根目录下）────────────────────────────────────────────
/// 图片素材库分区。
pub const IMAGE_LIBRARY: &str = "图片素材库";
/// 收件箱分区。
pub const INBOX: &str = "收件箱";
/// 任务包分区。
pub const TASK_PACKAGES: &str = "任务包";
/// 收件箱内的已收录归档子目录。
pub const INGESTED: &str = "已收录";
/// 收件箱内的已丢弃归档子目录（丢弃 = 移档，不删文件；rescan 排除，故不会复活）。
pub const DISCARDED: &str = "已丢弃";

/// 收件箱内 rescan 不进入的归档子目录。
pub const INBOX_ARCHIVES: [&str; 2] = [INGESTED, DISCARDED];

/// 四分区顶层目录（`init` 时创建；`INGESTED` 在 `收件箱/` 内按日期建）。
pub const PARTITIONS: [&str; 3] = [IMAGE_LIBRARY, INBOX, TASK_PACKAGES];

// ── 任务包内固定名（需求文档 §4.6）──────────────────────────────────────
/// 任务单 JSON 文件名。
pub const TASK_JSON: &str = "任务单.json";
/// 执行说明 markdown 文件名。
pub const EXEC_GUIDE: &str = "执行说明.md";
/// 就绪标志文件名（全部文件落盘后最后写入）。
pub const READY: &str = "READY.txt";
/// 图片目录名。
pub const IMAGES_DIR: &str = "图片";
/// RPA 追加写回执。
pub const RECEIPT_JSONL: &str = "回执.jsonl";
/// 旧素材命名辅助仍供迁移期单元测试使用，不再进入新任务包。
pub const VIDEO_STEM: &str = "video";

/// Windows 路径长度上限（超过即告警，防 260 字符截断）。
pub const PATH_LIMIT: usize = 260;

/// 执行机路径分隔风格。默认 Windows（执行侧多为影刀 / RPA）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum PathStyle {
    Windows,
    Unix,
}

impl PathStyle {
    pub fn from_str_or_default(s: &str) -> PathStyle {
        match s {
            "unix" | "mac" | "posix" => PathStyle::Unix,
            _ => PathStyle::Windows,
        }
    }
    /// 分隔符字符串。
    pub fn sep(self) -> &'static str {
        match self {
            PathStyle::Windows => "\\",
            PathStyle::Unix => "/",
        }
    }
}

/// 路径段是否可保留：丢弃空段与 `.`／`..`。
/// `..` 会让 `to_local` 穿越出根目录（SKU 编码等外来输入会流进路径），故在
/// 构造处剔除——`RelPath` 的构造签名不可失败，穿越段只能丢弃而非报错；编码层面的
/// 拒绝由 [`is_valid_sku_code`] 负责（两道防线）。
fn keep_seg(seg: &str) -> bool {
    !seg.is_empty() && seg != "." && seg != ".."
}

/// 根目录内相对路径（真相载体）。内部一律正斜杠、无前导斜杠、无 `.`/`..` 段。
/// repo 层只收此类型（类型强制而非散文约定）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct RelPath(String);

impl RelPath {
    /// 由任意分隔风格的字符串构造：反斜杠 → 正斜杠，去首尾斜杠，折叠重复斜杠，剔除 `.`/`..`。
    pub fn new(raw: impl AsRef<str>) -> RelPath {
        let s = raw.as_ref().replace('\\', "/");
        let joined = s
            .split('/')
            .filter(|seg| keep_seg(seg))
            .collect::<Vec<_>>()
            .join("/");
        RelPath(joined)
    }

    /// 由多个片段拼接。
    pub fn from_parts<I, S>(parts: I) -> RelPath
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let joined = parts
            .into_iter()
            .flat_map(|p| {
                p.as_ref()
                    .replace('\\', "/")
                    .split('/')
                    .filter(|s| keep_seg(s))
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>()
            .join("/");
        RelPath(joined)
    }

    /// 追加一个子片段，返回新 RelPath。
    pub fn join(&self, child: impl AsRef<str>) -> RelPath {
        RelPath::from_parts([self.0.as_str(), child.as_ref()])
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// 相对本机根目录解析为绝对 `PathBuf`（本机 IO 用）。
    pub fn to_local(&self, root: &std::path::Path) -> std::path::PathBuf {
        let mut p = root.to_path_buf();
        for seg in self.0.split('/').filter(|s| !s.is_empty()) {
            p.push(seg);
        }
        p
    }
}

/// 导出唯一转换点：执行机根 + 分隔符 + 包内相对路径（纯字符串拼接，需求 §7.3）。
/// 管理端为 Mac、执行机为 Windows 时照样拼出正确的 `D:\...\video.mp4`。
pub fn exec_join(exec_root: &str, rel: &RelPath, style: PathStyle) -> String {
    let root = exec_root.trim_end_matches(['/', '\\']);
    let rel_conv = rel.as_str().replace('/', style.sep());
    if rel_conv.is_empty() {
        root.to_string()
    } else if root.is_empty() {
        rel_conv
    } else {
        format!("{root}{}{rel_conv}", style.sep())
    }
}

/// 执行机根必须是对应平台的绝对路径；管理端与执行端可能不是同一操作系统，
/// 因而不能用本机 `Path::is_absolute` 判断 Windows 路径。
pub fn is_exec_root_absolute(root: &str, style: PathStyle) -> bool {
    let root = root.trim();
    match style {
        PathStyle::Unix => root.starts_with('/'),
        PathStyle::Windows => {
            let bytes = root.as_bytes();
            (bytes.len() >= 3
                && bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && matches!(bytes[2], b'\\' | b'/'))
                || root.starts_with("\\\\")
                || root.starts_with("//")
        }
    }
}

/// 路径是否超过 Windows 长度上限（导出时对拼接结果告警）。
pub fn exceeds_path_limit(path: &str) -> bool {
    path.chars().count() > PATH_LIMIT
}

/// 规范化为 ASCII 安全「变量名」：小写、仅保留 `[a-z0-9._-]`，其余（含空格、中文）
/// 折叠为单个 `_`，去首尾 `_`/`.`；空则回退 `x`。用于素材文件名 / 包目录名兜底。
pub fn ascii_slug(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut prev_underscore = false;
    for ch in name.chars() {
        let c = ch.to_ascii_lowercase();
        let keep = c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-');
        if keep {
            out.push(c);
            prev_underscore = false;
        } else if !prev_underscore {
            out.push('_');
            prev_underscore = true;
        }
    }
    let trimmed = out.trim_matches(|c| c == '_' || c == '.');
    if trimmed.is_empty() {
        "x".to_string()
    } else {
        trimmed.to_string()
    }
}

/// 校验一个名字是否已是 ASCII 安全（小写、无空格、无 `<>:"|?*` 与路径分隔符）。
pub fn is_ascii_safe_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().all(|c| {
            c.is_ascii()
                && !c.is_ascii_whitespace()
                && !c.is_ascii_uppercase()
                && !matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*' | '/' | '\\')
        })
}

/// SKU 编码长度上限（编码进目录名与任务包，留足 Windows 路径预算）。
pub const SKU_CODE_MAX: usize = 64;

/// Windows 保留设备名：这些名字（含带扩展名的形式，如 `con.txt`）在 Windows 上
/// 无法创建为目录/文件，执行机上会整个 SKU 目录建不出来。
const WIN_RESERVED: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// 校验 SKU 编码：ASCII 字母/数字/`-_.`，非空、无空格、≤64 字符；
/// 拒绝纯点段（`.`/`..`，路径穿越入口）与 Windows 保留设备名。
///
/// 注：SKU 编码作为用户可见标识（任务包 + 目录名），允许大写（如 `SF-YD-201`）；
/// GenDesk 是目录结构唯一写方，但 Windows 文件系统大小写不敏感，故编码的**唯一性**
/// 按大小写不敏感判定（`skus` 表 `idx_skus_code_nocase` 唯一索引 + repo 层 `COLLATE NOCASE`）。
pub fn is_valid_sku_code(code: &str) -> bool {
    if code.is_empty() || code.chars().count() > SKU_CODE_MAX {
        return false;
    }
    if !code
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return false;
    }
    if code.chars().all(|c| c == '.') {
        return false;
    }
    let lower = code.to_ascii_lowercase();
    let stem = lower.split('.').next().unwrap_or(lower.as_str());
    !WIN_RESERVED.contains(&stem)
}

/// 规范化扩展名（去点、转小写）；空则回退 `bin`。
pub fn ascii_ext(path: &str) -> String {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext.is_empty() {
        "bin".to_string()
    } else {
        ext
    }
}

/// 图集包内成员名：`img_01.jpg`（1-based，两位补零）。
pub fn gallery_member(index: usize, ext: &str) -> String {
    format!("img_{index:02}.{ext}")
}

/// 视频包内视频名：`video.mp4`（扩展名跟随源）。
pub fn video_member(ext: &str) -> String {
    format!("{VIDEO_STEM}.{ext}")
}

/// 在已占用名集合上去重：`base`、`base_2`、`base_3`…（重名加序，前置事实 1.2）。
pub fn dedupe_name(base: &str, taken: &dyn Fn(&str) -> bool) -> String {
    if !taken(base) {
        return base.to_string();
    }
    let (stem, ext) = match base.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (base.to_string(), String::new()),
    };
    let mut n = 2;
    loop {
        let candidate = format!("{stem}_{n}{ext}");
        if !taken(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即测试失败，是期望行为
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn relpath_normalizes_separators_and_slashes() {
        assert_eq!(RelPath::new("a\\b\\c").as_str(), "a/b/c");
        assert_eq!(RelPath::new("/a//b/").as_str(), "a/b");
        assert_eq!(
            RelPath::from_parts(["资产库", "SF-YD-201/g1"]).as_str(),
            "资产库/SF-YD-201/g1"
        );
        assert_eq!(RelPath::new("a").join("b").as_str(), "a/b");
    }

    #[test]
    fn to_local_resolves_under_root() {
        let root = std::path::Path::new("/tmp/root");
        let p = RelPath::new("资产库/sku/img_01.jpg").to_local(root);
        assert_eq!(p, std::path::Path::new("/tmp/root/资产库/sku/img_01.jpg"));
    }

    // A5：`..`/`.` 段一律剔除 → 任何外来输入都无法穿越出根目录。
    #[test]
    fn relpath_strips_traversal_segments() {
        assert_eq!(RelPath::new("a/../../b").as_str(), "a/b");
        assert_eq!(RelPath::new("./a/./b").as_str(), "a/b");
        assert_eq!(RelPath::new("../../..").as_str(), "");
        assert_eq!(RelPath::from_parts(["资产库", ".."]).as_str(), "资产库");
        let root = std::path::Path::new("/tmp/root");
        let escaped = RelPath::new("资产库/../../etc/passwd").to_local(root);
        assert!(escaped.starts_with(root), "解析结果必须仍在根目录内");
        assert_eq!(escaped, std::path::Path::new("/tmp/root/资产库/etc/passwd"));
    }

    #[test]
    fn sku_code_rejects_traversal_reserved_and_overlong() {
        assert!(is_valid_sku_code("SF-YD-201"));
        assert!(is_valid_sku_code("sf_1.a"));
        assert!(!is_valid_sku_code(".."));
        assert!(!is_valid_sku_code("."));
        assert!(!is_valid_sku_code("..."));
        assert!(!is_valid_sku_code("CON"));
        assert!(!is_valid_sku_code("com1"));
        assert!(!is_valid_sku_code("nul.txt"), "带扩展名的保留名同样不可用");
        assert!(!is_valid_sku_code(&"a".repeat(SKU_CODE_MAX + 1)));
        assert!(is_valid_sku_code(&"a".repeat(SKU_CODE_MAX)));
        assert!(!is_valid_sku_code("a b"));
        assert!(!is_valid_sku_code(""));
        assert!(is_valid_sku_code("console"), "只有恰好等于保留名才拒绝");
    }

    #[test]
    fn exec_join_windows_and_unix() {
        let rel = RelPath::new("任务包/20260714/素材/SF-YD-201/video.mp4");
        assert_eq!(
            exec_join("D:\\视频发布", &rel, PathStyle::Windows),
            "D:\\视频发布\\任务包\\20260714\\素材\\SF-YD-201\\video.mp4"
        );
        assert_eq!(
            exec_join("/Users/x/视频发布/", &rel, PathStyle::Unix),
            "/Users/x/视频发布/任务包/20260714/素材/SF-YD-201/video.mp4"
        );
        // 空根 / 空 rel 边界
        assert_eq!(exec_join("", &rel, PathStyle::Unix), rel.as_str());
        assert_eq!(
            exec_join("D:\\x", &RelPath::new(""), PathStyle::Windows),
            "D:\\x"
        );
    }

    #[test]
    fn exec_root_absolute_is_checked_for_the_remote_platform() {
        assert!(is_exec_root_absolute(r"D:\GenDesk", PathStyle::Windows));
        assert!(is_exec_root_absolute(r"\\server\share", PathStyle::Windows));
        assert!(!is_exec_root_absolute("GenDesk", PathStyle::Windows));
        assert!(is_exec_root_absolute("/srv/gendesk", PathStyle::Unix));
        assert!(!is_exec_root_absolute("srv/gendesk", PathStyle::Unix));
    }

    #[test]
    fn batch_copy_rolls_back_files_created_before_a_later_failure() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.jpg");
        std::fs::write(&source, b"image").unwrap();
        let first = dir.path().join("dest/first.jpg");
        let second = dir.path().join("dest/second.jpg");
        let missing = dir.path().join("missing.jpg");
        let result = copy_batch_new(&[(source, first.clone()), (missing, second.clone())]);
        assert!(result.is_err());
        assert!(!first.exists());
        assert!(!second.exists());
    }

    #[test]
    fn batch_copy_rolls_back_when_a_later_parent_cannot_be_created() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.jpg");
        std::fs::write(&source, b"image").unwrap();
        let first = dir.path().join("dest/first.jpg");
        let parent_is_file = dir.path().join("not-a-directory");
        std::fs::write(&parent_is_file, b"file").unwrap();
        let second = parent_is_file.join("second.jpg");

        let result = copy_batch_new(&[(source.clone(), first.clone()), (source, second.clone())]);

        assert!(result.is_err());
        assert!(!first.exists());
        assert!(!second.exists());
    }

    #[test]
    fn path_limit_warns_over_260() {
        assert!(!exceeds_path_limit(&"a".repeat(260)));
        assert!(exceeds_path_limit(&"a".repeat(261)));
    }

    #[test]
    fn ascii_slug_lowercases_and_collapses() {
        assert_eq!(ascii_slug("My Video 01.MP4"), "my_video_01.mp4");
        assert_eq!(ascii_slug("我的图片"), "x");
        assert_eq!(ascii_slug("a<>:b|?*c"), "a_b_c");
        assert_eq!(ascii_slug("  __trim__  "), "trim");
        assert_eq!(ascii_slug("视频_a"), "a");
    }

    #[test]
    fn is_ascii_safe_name_rules() {
        assert!(is_ascii_safe_name("img_01.jpg"));
        assert!(is_ascii_safe_name("sf-yd-201"));
        assert!(!is_ascii_safe_name("IMG.JPG")); // 大写
        assert!(!is_ascii_safe_name("a b.jpg")); // 空格
        assert!(!is_ascii_safe_name("a:b")); // 非法字符
        assert!(!is_ascii_safe_name("图.jpg")); // 非 ASCII
        assert!(!is_ascii_safe_name(""));
    }

    #[test]
    fn member_names() {
        assert_eq!(gallery_member(1, "jpg"), "img_01.jpg");
        assert_eq!(gallery_member(12, "png"), "img_12.png");
        assert_eq!(video_member("mov"), "video.mov");
        assert_eq!(ascii_ext("/a/b/My.MP4"), "mp4");
        assert_eq!(ascii_ext("noext"), "bin");
    }

    #[test]
    fn dedupe_adds_suffix_on_collision() {
        let mut taken: HashSet<String> = HashSet::new();
        taken.insert("img_01.jpg".into());
        taken.insert("img_01_2.jpg".into());
        let f = |s: &str| taken.contains(s);
        assert_eq!(dedupe_name("img_01.jpg", &f), "img_01_3.jpg");
        assert_eq!(dedupe_name("fresh.jpg", &f), "fresh.jpg");
        // 无扩展名
        let empty = |_: &str| false;
        assert_eq!(dedupe_name("gallery", &empty), "gallery");
    }
}
