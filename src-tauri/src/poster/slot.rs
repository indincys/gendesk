//! 审稿通过的图进 Poster 商品的哪个库位：封面库、某个图片类型，或图片库里的未分类。
//!
//! 依据是作品归属里的 `posterType`（Agent 投单时显式指定）或需求名（缺省）：
//! - `封面` / `封面图` / `cover` → 封面库；
//! - 与 Poster 某个图片类型同名 → 该类型（先比原名，再比去掉末尾「图」字后的名字，须唯一）；
//! - `未分类`、空或对不上 → 未分类。
//!
//! 需求名是自由文字（`桌面场景图` 这类本来就不对应任何类型），所以按需求名对不上只是未分类；
//! 显式写了 `posterType` 却对不上，是 Agent 的明确意图落空，报错而不是悄悄降成未分类。
//! 旧版 Poster 没有类型目录：按需求名的照旧进未分类（不因升级前的版本卡住全部导入），显式类型报错。
use serde::Serialize;

use super::client::{PosterImageType, PURPOSE_COVER};
use crate::library::Organization;

/// 显式指定「不分类」的写法。
pub const UNTYPED: &str = "未分类";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Slot {
    /// 封面库（`purpose: cover`）。
    Cover,
    /// 图片库里的某个图片类型。
    Type { id: String, name: String },
    /// 图片库，未分类。
    #[default]
    Untyped,
}

impl Slot {
    /// 写进 Poster 请求的 `(purpose, typeId)`。
    pub fn wire(&self) -> (Option<String>, Option<String>) {
        match self {
            Slot::Cover => (Some(PURPOSE_COVER.into()), None),
            Slot::Type { id, .. } => (None, Some(id.clone())),
            Slot::Untyped => (None, None),
        }
    }

    #[cfg(test)]
    pub fn label(&self) -> &str {
        match self {
            Slot::Cover => "封面",
            Slot::Type { name, .. } => name,
            Slot::Untyped => UNTYPED,
        }
    }
}

/// 作品想进的库位名，以及它是不是 Agent 显式指定的。
pub fn wanted(organization: &Organization) -> (&str, bool) {
    match organization.poster_type() {
        "" => (organization.requirement.as_str(), false),
        explicit => (explicit, true),
    }
}

pub fn is_cover(label: &str) -> bool {
    matches!(label, "封面" | "封面图") || label.eq_ignore_ascii_case(PURPOSE_COVER)
}

/// 解析库位。`types` 为 `None` 表示 Poster 版本过旧、没有图片类型目录。
/// `Err` 是给人看的原因（显式类型对不上，或需要类型而 Poster 太旧）。
pub fn resolve(
    label: &str,
    explicit: bool,
    types: Option<&[PosterImageType]>,
) -> Result<Slot, String> {
    let label = label.trim();
    if label.is_empty() || label == UNTYPED {
        return Ok(Slot::Untyped);
    }
    if is_cover(label) {
        return Ok(Slot::Cover);
    }
    let Some(types) = types else {
        if !explicit {
            tracing::warn!(label, "Poster 版本过旧，没有图片类型目录，本张进未分类");
            return Ok(Slot::Untyped);
        }
        return Err(format!(
            "当前 Poster 不支持按图片类型导入，无法归入「{label}」；请更新 Poster 后重试"
        ));
    };
    let pick = |found: Vec<&PosterImageType>| match found.as_slice() {
        [only] => Some(Slot::Type {
            id: only.id.clone(),
            name: only.name.clone(),
        }),
        _ => None,
    };
    let stem = |name: &str| name.strip_suffix('图').unwrap_or(name).to_string();
    let found = pick(types.iter().filter(|t| t.name == label).collect()).or_else(|| {
        pick(
            types
                .iter()
                .filter(|t| stem(&t.name) == stem(label))
                .collect(),
        )
    });
    match found {
        Some(slot) => Ok(slot),
        None if explicit => Err(format!(
            "Poster 里没有图片类型「{label}」，可用：{}、封面、未分类",
            types
                .iter()
                .map(|t| t.name.as_str())
                .collect::<Vec<_>>()
                .join("、")
        )),
        None => Ok(Slot::Untyped),
    }
}

/// 作品的库位：归属快照读不出时按未分类处理（旧作品没有需求名）。
pub fn of_work(
    organization_json: Option<&str>,
    types: Option<&[PosterImageType]>,
) -> Result<Slot, String> {
    let organization: Organization = organization_json
        .and_then(|json| serde_json::from_str(json).ok())
        .unwrap_or_default();
    let (label, explicit) = wanted(&organization);
    resolve(label, explicit, types)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // 测试断言失败即失败
mod tests {
    use super::*;

    fn types() -> Vec<PosterImageType> {
        ["正视图", "45度角", "礼盒图", "侧视图"]
            .iter()
            .enumerate()
            .map(|(i, name)| PosterImageType {
                id: format!("t{i}"),
                name: (*name).into(),
            })
            .collect()
    }

    #[test]
    fn requirement_names_map_to_cover_types_or_untyped() {
        let types = types();
        let all = Some(types.as_slice());
        assert_eq!(resolve("封面", false, all), Ok(Slot::Cover));
        assert_eq!(resolve("封面图", false, all), Ok(Slot::Cover));
        assert_eq!(
            resolve("正视图", false, all),
            Ok(Slot::Type {
                id: "t0".into(),
                name: "正视图".into()
            })
        );
        // 末尾「图」字可有可无：「礼盒」对「礼盒图」、「45度角图」对「45度角」。
        assert_eq!(
            resolve("礼盒", false, all).map(|s| s.label().to_string()),
            Ok("礼盒图".into())
        );
        assert_eq!(
            resolve("45度角图", false, all).map(|s| s.label().to_string()),
            Ok("45度角".into())
        );
        // 场景类需求本来就不对应类型：未分类，不报错。
        assert_eq!(resolve("桌面场景图", false, all), Ok(Slot::Untyped));
        assert_eq!(resolve("", false, all), Ok(Slot::Untyped));
        // 不按包含或前缀猜：「正视图特写」不是「正视图」。
        assert_eq!(resolve("正视图特写", false, all), Ok(Slot::Untyped));
    }

    #[test]
    fn explicit_type_must_exist_and_can_force_untyped() {
        let types = types();
        let all = Some(types.as_slice());
        let error = resolve("正面图", true, all).unwrap_err();
        assert!(
            error.contains("正视图") && error.contains("封面"),
            "{error}"
        );
        assert_eq!(resolve(UNTYPED, true, all), Ok(Slot::Untyped));
        assert_eq!(resolve("cover", true, all), Ok(Slot::Cover));
        let organization = Organization {
            requirement: "正面场景图".into(),
            poster_type: Some("正视图".into()),
            ..Organization::default()
        };
        assert_eq!(wanted(&organization), ("正视图", true));
    }

    #[test]
    fn old_poster_without_types_still_takes_cover_and_untyped() {
        assert_eq!(resolve("封面", false, None), Ok(Slot::Cover));
        assert_eq!(resolve("", false, None), Ok(Slot::Untyped));
        assert_eq!(resolve("正视图", false, None), Ok(Slot::Untyped));
        assert!(resolve("正视图", true, None).is_err());
        assert_eq!(of_work(None, None), Ok(Slot::Untyped));
        assert_eq!(
            of_work(Some(r#"{"requirement":"封面"}"#), None),
            Ok(Slot::Cover)
        );
    }

    #[test]
    fn wire_fields_follow_the_slot() {
        assert_eq!(Slot::Cover.wire(), (Some("cover".into()), None));
        assert_eq!(
            Slot::Type {
                id: "t".into(),
                name: "n".into()
            }
            .wire(),
            (None, Some("t".into()))
        );
        assert_eq!(Slot::Untyped.wire(), (None, None));
    }
}
