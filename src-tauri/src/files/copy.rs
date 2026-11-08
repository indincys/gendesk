//! 不覆盖的批量拷贝与补偿守卫，验收交付与实况图共用。

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

    pub fn track(&mut self, path: std::path::PathBuf) {
        self.paths.push(path);
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
