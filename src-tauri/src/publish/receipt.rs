//! RPA 回执 jsonl 解析纯函数。

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

pub const FAIL_KINDS: [&str; 6] = [
    "登录失效",
    "风控拦截",
    "素材不合规",
    "页面变更",
    "网络超时",
    "其他",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReceiptLine {
    pub task_id: String,
    pub status: String,
    pub fail_kind: Option<String>,
    pub message: String,
    pub finished_at: String,
}

pub fn parse_jsonl(source: &str) -> Result<Vec<ReceiptLine>, String> {
    let lines: Vec<(usize, &str)> = source
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .collect();
    let mut out = Vec::new();
    let mut task_ids = HashSet::new();
    for (index, (line_no, line)) in lines.iter().enumerate() {
        match serde_json::from_str::<ReceiptLine>(line) {
            Ok(receipt) => {
                if receipt.status != "已完成" && receipt.status != "失败" {
                    return Err(format!("回执第 {} 行 status 非法", line_no + 1));
                }
                if receipt.task_id.trim().is_empty() {
                    return Err(format!("回执第 {} 行 taskId 为空", line_no + 1));
                }
                if !task_ids.insert(receipt.task_id.clone()) {
                    return Err(format!(
                        "回执第 {} 行 taskId 重复：{}",
                        line_no + 1,
                        receipt.task_id
                    ));
                }
                match (receipt.status.as_str(), receipt.fail_kind.as_deref()) {
                    ("已完成", None) => {}
                    ("已完成", Some(_)) => {
                        return Err(format!(
                            "回执第 {} 行：已完成时 failKind 必须为 null",
                            line_no + 1
                        ));
                    }
                    ("失败", Some(kind)) if FAIL_KINDS.contains(&kind) => {}
                    ("失败", _) => {
                        return Err(format!(
                            "回执第 {} 行：失败时 failKind 必须是约定的失败类型",
                            line_no + 1
                        ));
                    }
                    _ => unreachable!(),
                }
                if chrono::NaiveDateTime::parse_from_str(&receipt.finished_at, "%Y-%m-%d %H:%M")
                    .is_err()
                {
                    return Err(format!("回执第 {} 行 finishedAt 格式非法", line_no + 1));
                }
                out.push(receipt);
            }
            Err(_) if index + 1 == lines.len() && !line.trim_end().ends_with('}') => break,
            Err(err) => return Err(format!("回执第 {} 行解析失败：{err}", line_no + 1)),
        }
    }
    Ok(out)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // 测试断言失败即测试失败
mod tests {
    use super::*;

    #[test]
    fn incomplete_last_line_is_ignored() {
        let src = "{\"taskId\":\"T1\",\"status\":\"已完成\",\"failKind\":null,\"message\":\"草稿已存\",\"finishedAt\":\"2026-08-02 09:16\"}\n{\"taskId\":\"T2\"";
        let rows = parse_jsonl(src).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].task_id, "T1");
    }

    #[test]
    fn corrupt_middle_line_is_reported() {
        let src = "bad\n{\"taskId\":\"T1\",\"status\":\"已完成\",\"failKind\":null,\"message\":\"ok\",\"finishedAt\":\"2026-08-02 09:16\"}";
        assert!(parse_jsonl(src).unwrap_err().contains("第 1 行"));
    }

    #[test]
    fn complete_but_invalid_last_line_is_not_mistaken_for_a_crash_fragment() {
        let err = parse_jsonl("{\"taskId\":1}").unwrap_err();
        assert!(err.contains("第 1 行解析失败"));
    }

    #[test]
    fn validates_fail_kind_semantics_and_unique_task_ids() {
        let completed_with_reason = "{\"taskId\":\"T1\",\"status\":\"已完成\",\"failKind\":\"其他\",\"message\":\"ok\",\"finishedAt\":\"2026-08-02 09:16\"}";
        assert!(parse_jsonl(completed_with_reason)
            .unwrap_err()
            .contains("必须为 null"));
        let bad_failure = "{\"taskId\":\"T1\",\"status\":\"失败\",\"failKind\":\"随便写\",\"message\":\"bad\",\"finishedAt\":\"2026-08-02 09:16\"}";
        assert!(parse_jsonl(bad_failure)
            .unwrap_err()
            .contains("约定的失败类型"));
        let duplicate = "{\"taskId\":\"T1\",\"status\":\"已完成\",\"failKind\":null,\"message\":\"ok\",\"finishedAt\":\"2026-08-02 09:16\"}\n{\"taskId\":\"T1\",\"status\":\"已完成\",\"failKind\":null,\"message\":\"ok\",\"finishedAt\":\"2026-08-02 09:16\"}";
        assert!(parse_jsonl(duplicate).unwrap_err().contains("taskId 重复"));
    }
}
