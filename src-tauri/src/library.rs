//! 人的作品目录：产品 / 款式 / 需求。运行批次、来源和日期不参与目录身份。
pub mod maintenance;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, Default, Deserialize, Serialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct Organization {
    pub product: String,
    pub style: String,
    pub requirement: String,
    pub source: String,
    /// 审稿通过后进 Poster 的哪个库位：Poster 图片类型名、`封面` 或 `未分类`。空 = 按需求名自动对应
    /// （见 `poster::slot`）。只影响 Poster，不参与目录；空时不写出，旧快照与内容指纹逐字节不变。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[specta(optional)]
    pub poster_type: Option<String>,
}

impl Organization {
    pub fn validate(&self) -> AppResult<()> {
        for (label, value) in [
            ("产品", self.product.as_str()),
            ("款式", self.style.as_str()),
            ("需求", self.requirement.as_str()),
            ("Poster 类型", self.poster_type()),
        ] {
            if !value.is_empty()
                && (value.trim() != value
                    || value == "."
                    || value == ".."
                    || value.len() > 180
                    || value.ends_with('.')
                    || value
                        .chars()
                        .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c)))
            {
                return Err(AppError::InvalidInput(format!(
                    "{label}名称不能包含路径或保留字符，且不得超过 180 字节"
                )));
            }
        }
        Ok(())
    }

    /// 旧任务仅沿用明确的 SKU、款式、需求描述；不解析图片名、日期或运行编号。
    pub fn legacy(product: &str, style: &str, requirement: &str) -> Self {
        Self {
            product: product.trim().into(),
            style: style.trim().into(),
            requirement: requirement.trim().into(),
            source: String::new(),
            poster_type: None,
        }
    }

    /// 显式指定的 Poster 库位；空 = 按需求名自动对应。
    pub fn poster_type(&self) -> &str {
        self.poster_type.as_deref().unwrap_or_default()
    }

    pub fn from_params(params: &str) -> AppResult<Option<Self>> {
        if params.trim().is_empty() {
            return Ok(None);
        }
        let value: serde_json::Value = serde_json::from_str(params)?;
        value
            .get("organization")
            .map(|v| {
                let result: Self = serde_json::from_value(v.clone())?;
                result.validate()?;
                Ok(result)
            })
            .transpose()
    }

    /// 新归属是明确输入；旧 SKU/款式存在时沿用既有安全绑定，不因整理目录换商品。
    pub fn poster_keys(&self, sku: &str, style: &str) -> (String, String) {
        (
            if sku.is_empty() {
                self.product.clone()
            } else {
                sku.into()
            },
            if style.is_empty() {
                self.style.clone()
            } else {
                style.into()
            },
        )
    }

    pub fn directory(&self, root: &Path, accepted: bool, video: bool) -> PathBuf {
        let mut path = root
            .join(if accepted { "成品" } else { "_草稿" })
            .join(if video {
                "生成视频"
            } else {
                "生成图片"
            });
        if !self.product.is_empty() {
            path.push(component(&self.product));
        }
        if !self.style.is_empty() {
            path.push(component(&self.style));
        }
        path.join(if self.requirement.is_empty() {
            "需求待补充".into()
        } else {
            component(&self.requirement)
        })
    }
}

// 清洗必须不把不同名字压成同一路径：发生清洗/截断时附原值哈希。
pub fn component(value: &str) -> String {
    use sha2::{Digest, Sha256};
    let safe =
        crate::files::sanitize_filename(value).replace([' ', '（', '）', '(', ')', '.'], "_");
    let safe: String = safe.chars().take(48).collect();
    if safe != value || safe.is_empty() {
        let hash = format!("{:x}", Sha256::digest(value.as_bytes()));
        format!("{}_{hash}", if safe.is_empty() { "未命名" } else { &safe })
    } else {
        safe
    }
}

pub fn filename(name: &str, identity: &str, at: i64, ext: &str) -> String {
    let day = chrono::DateTime::from_timestamp(at, 0)
        .unwrap_or_default()
        .with_timezone(&chrono::Local)
        .format("%Y-%m-%d");
    // 内容名保留在元数据；文件名控制长度，身份用于防覆盖。
    let name: String = crate::files::sanitize_filename(name)
        .replace([' ', '.', '（', '）', '(', ')', '-', '_'], "")
        .chars()
        .take(40)
        .collect();
    format!("{day}_{name}_{identity}.{}", ext.to_lowercase())
}

pub fn default_root() -> PathBuf {
    crate::poster::home().join("GenDesk作品库")
}

pub async fn root(pool: &sqlx::SqlitePool) -> AppResult<PathBuf> {
    let settings =
        crate::commands::settings::load_settings(pool, &default_root().to_string_lossy()).await?;
    let root = PathBuf::from(settings.output_dir);
    if !root.is_absolute() {
        return Err(AppError::InvalidInput("作品根目录必须是绝对路径".into()));
    }
    Ok(root)
}

/// 只覆盖相同内容的安全重试；同名不同图永远报错。
pub fn copy_immutable(source: &Path, target: &Path) -> AppResult<()> {
    use std::io::Write;
    let bytes = std::fs::read(source)?;
    if target.exists() {
        if std::fs::read(target)? != bytes {
            return Err(AppError::InvalidInput(format!(
                "成品路径已有不同内容，未覆盖：{}",
                target.display()
            )));
        }
        return Ok(());
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = target.with_extension(format!("{}.partial", crate::poster::new_id()));
    let result = (|| -> AppResult<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        match std::fs::hard_link(&temporary, target) {
            Ok(()) => Ok(()),
            Err(_) if target.exists() && std::fs::read(target)? == bytes => Ok(()),
            Err(error) => Err(error.into()),
        }
    })();
    let _ = std::fs::remove_file(&temporary);
    result?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 隔离路径行为验证。
mod tests {
    use super::*;
    #[test]
    fn same_requirement_merges_runs_and_different_styles_stay_separate() {
        let a = Organization::legacy("水杯", "奶油白款", "厨房场景");
        let mut b = a.clone();
        b.source = "另一个工具".into();
        assert_eq!(
            a.directory(Path::new("/library"), true, false),
            b.directory(Path::new("/library"), true, false)
        );
        b.style = "薄荷绿款".into();
        assert_ne!(
            a.directory(Path::new("/library"), true, false),
            b.directory(Path::new("/library"), true, false)
        );
        assert_eq!(
            a.directory(Path::new("/library"), true, true),
            PathBuf::from("/library/成品/生成视频/水杯/奶油白款/厨房场景")
        );
        assert!(Organization::legacy("../wrong", "", "x")
            .validate()
            .is_err());
        assert_ne!(component("a b"), component("a_b"));
    }
    #[test]
    fn content_identity_prevents_overwrite_and_optional_levels_are_omitted() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("source");
        let dest = d.path().join("dest");
        std::fs::write(&src, b"first").unwrap();
        copy_immutable(&src, &dest).unwrap();
        copy_immutable(&src, &dest).unwrap();
        std::fs::write(&src, b"second").unwrap();
        assert!(copy_immutable(&src, &dest).is_err());
        assert_eq!(std::fs::read(&dest).unwrap(), b"first");
        assert_eq!(
            Organization::legacy("", "", "活动海报").directory(d.path(), true, false),
            d.path().join("成品/生成图片/活动海报")
        );
    }
}
