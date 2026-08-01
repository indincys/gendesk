//! 四平台枚举与中文名映射（图文发布链路重构 §3.3）。
//!
//! 平台集合是**单点定义**：加平台 = 改此文件一处（+ migration 无涉）。
//! 存储与 JSON 键用小写 code（`douyin`…）；UI 与任务包展示一律使用中文（`zh()`）。
//! 文本条目的平台标签 = 四平台 code + `general`（通用），见 [`text_platform_tag`]。

// 部分映射先于 P2 编排消费者落地；未使用项在对应任务接入后收紧。
#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use specta::Type;

/// 固定四平台枚举。序列化为小写 code，与 DB / 平台矩阵键一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Douyin,
    Xhs,
    Kuaishou,
    Shipinhao,
}

/// 文本条目「通用」标签（不属于任何具体平台）。
pub const GENERAL_TAG: &str = "general";

impl Platform {
    /// 全部平台，顺序即 UI/矩阵展示序。
    pub const ALL: [Platform; 4] = [
        Platform::Douyin,
        Platform::Xhs,
        Platform::Kuaishou,
        Platform::Shipinhao,
    ];

    /// 存储 / 平台矩阵键（ASCII 小写）。
    pub fn code(self) -> &'static str {
        match self {
            Platform::Douyin => "douyin",
            Platform::Xhs => "xhs",
            Platform::Kuaishou => "kuaishou",
            Platform::Shipinhao => "shipinhao",
        }
    }

    /// 中文显示名（任务包 + UI）。
    pub fn zh(self) -> &'static str {
        match self {
            Platform::Douyin => "抖音",
            Platform::Xhs => "小红书",
            Platform::Kuaishou => "快手",
            Platform::Shipinhao => "视频号",
        }
    }

    /// 由存储 code 解析。
    pub fn from_code(code: &str) -> Option<Platform> {
        Platform::ALL.into_iter().find(|p| p.code() == code)
    }

    /// 由中文名解析（收件箱文件名等人工输入容错）。
    /// 容忍视频号的常见中文别名。
    pub fn from_zh(name: &str) -> Option<Platform> {
        let n = name.trim();
        if let Some(p) = Platform::ALL.into_iter().find(|p| p.zh() == n) {
            return Some(p);
        }
        match n {
            "视频号" | "微信视频号" => Some(Platform::Shipinhao),
            _ => None,
        }
    }
}

/// 平台标签信息（暴露给前端做矩阵/选择器渲染，避免前端重复中文映射）。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PlatformInfo {
    pub code: String,
    pub zh: String,
}

/// 全部四平台的 `{code, zh}`（前端单点数据来源）。
pub fn platform_infos() -> Vec<PlatformInfo> {
    Platform::ALL
        .into_iter()
        .map(|p| PlatformInfo {
            code: p.code().to_string(),
            zh: p.zh().to_string(),
        })
        .collect()
}

/// 把任意平台名（中文别名或 code，或「通用」）规范为文本条目平台标签：
/// 命中五平台 → 其 code；`通用`/`general`/空/未知 → `general`。
pub fn text_platform_tag(name: &str) -> String {
    let n = name.trim();
    if let Some(p) = Platform::from_code(n).or_else(|| Platform::from_zh(n)) {
        p.code().to_string()
    } else {
        GENERAL_TAG.to_string()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即测试失败，是期望行为
mod tests {
    use super::*;

    #[test]
    fn code_zh_roundtrip_all() {
        for p in Platform::ALL {
            assert_eq!(Platform::from_code(p.code()), Some(p));
            assert_eq!(Platform::from_zh(p.zh()), Some(p), "{} 中文名回读", p.zh());
        }
    }

    #[test]
    fn from_zh_aliases() {
        assert_eq!(Platform::from_zh("微信视频号"), Some(Platform::Shipinhao));
        assert_eq!(Platform::from_zh(" 小红书 "), Some(Platform::Xhs));
        assert_eq!(Platform::from_zh("未知平台"), None);
    }

    #[test]
    fn text_tag_falls_back_to_general() {
        assert_eq!(text_platform_tag("小红书"), "xhs");
        assert_eq!(text_platform_tag("xhs"), "xhs");
        assert_eq!(text_platform_tag("通用"), "general");
        assert_eq!(text_platform_tag("general"), "general");
        assert_eq!(text_platform_tag(""), "general");
        assert_eq!(text_platform_tag("火星"), "general");
    }

    #[test]
    fn infos_cover_four() {
        let infos = platform_infos();
        assert_eq!(infos.len(), 4);
        assert_eq!(infos[0].code, "douyin");
        assert_eq!(infos[0].zh, "抖音");
    }
}
