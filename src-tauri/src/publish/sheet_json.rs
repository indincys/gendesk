//! `gendesk.tasksheet/1` 契约的单一序列化定义。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProductRef {
    pub code: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DouyinOptions {
    pub product_url: String,
    pub short_title: String,
    pub visibility: String,
    pub allow_save: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct XhsOptions {
    pub original: bool,
    pub allow_co_create: bool,
    pub allow_copy: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShipinhaoOptions {
    pub location: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportTask {
    pub task_id: String,
    pub content_id: String,
    pub platform: String,
    pub mode: String,
    pub scheduled_at: String,
    pub title: Option<String>,
    pub description: String,
    pub topics: Vec<String>,
    pub image_paths: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cover_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub music_keyword: Option<String>,
    pub cart: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub douyin: Option<DouyinOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub xhs: Option<XhsOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shipinhao: Option<ShipinhaoOptions>,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSheetJson {
    pub schema: String,
    pub sheet_id: String,
    pub product: ProductRef,
    pub generated_at: String,
    pub tasks: Vec<ExportTask>,
}

pub fn to_pretty_json(sheet: &TaskSheetJson) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(sheet)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // 测试断言失败即测试失败
mod tests {
    use super::*;

    #[test]
    fn kuaishou_title_is_null_and_absent_options_stay_absent() {
        let sheet = TaskSheetJson {
            schema: "gendesk.tasksheet/1".into(),
            sheet_id: "A-20260802".into(),
            product: ProductRef {
                code: "A".into(),
                name: "商品".into(),
            },
            generated_at: "2026-08-01 20:00".into(),
            tasks: vec![ExportTask {
                task_id: "T1".into(),
                content_id: "C1".into(),
                platform: "快手".into(),
                mode: "图文".into(),
                scheduled_at: "2026-08-02 09:00".into(),
                title: None,
                description: "正文".into(),
                topics: vec![],
                image_paths: vec!["/a.jpg".into()],
                cover_path: None,
                music_keyword: None,
                cart: false,
                douyin: None,
                xhs: None,
                shipinhao: None,
                note: String::new(),
            }],
        };
        let value: serde_json::Value =
            serde_json::from_str(&to_pretty_json(&sheet).unwrap()).unwrap();
        assert!(value["tasks"][0]["title"].is_null());
        assert!(value["tasks"][0].get("coverPath").is_none());
        assert!(value["tasks"][0].get("douyin").is_none());
    }
}
