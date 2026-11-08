//! Read-only input validation. No database, app startup, thumbnails or provider calls.
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{OrderSpec, Plan};
use crate::error::{AppError, AppResult};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExternalInput {
    pub version: i64,
    pub order: OrderSpec,
    pub prompts: HashMap<String, String>,
    pub references: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceCheck {
    pub source: PathBuf,
    pub sha256: String,
    #[serde(skip)]
    pub content_hash: String,
    pub width: u32,
    pub height: u32,
}

pub struct CheckedPlan {
    pub fingerprint: String,
    pub references: Vec<ReferenceCheck>,
}

pub async fn check(plan: &Plan) -> AppResult<CheckedPlan> {
    let mut references = Vec::new();
    for source in plan.distinct_refs() {
        let path = source.clone();
        let checked = crate::files::decode::bounded(move |permit| {
            let image = crate::files::decode::inspect_image(&path, permit)?;
            Ok(ReferenceCheck {
                source: path,
                sha256: format!("{:x}", Sha256::digest(&image.bytes)),
                content_hash: crate::files::content_hash_bytes(&image.bytes),
                width: image.width,
                height: image.height,
            })
        })
        .await
        .map_err(|error| {
            AppError::InvalidInput(format!("参考图解码失败 {}：{error}", source.display()))
        })?;
        references.push(checked);
    }
    let items: Vec<_> = plan
        .items
        .iter()
        .map(|item| {
            let hashes: Vec<_> = item
                .ref_paths
                .iter()
                .map(|path| {
                    references
                        .iter()
                        .find(|reference| &reference.source == path)
                        .map(|r| r.sha256.as_str())
                })
                .collect();
            serde_json::json!({"name":item.name,"sku":item.sku,"style":item.style,
            "prompt":item.prompt_text,"params":item.params_json,"refs":hashes})
        })
        .collect();
    let bytes = serde_json::to_vec(&serde_json::json!({"items":items,"note":plan.note,
        "purpose":plan.purpose,"skill":plan.skill,"refGroup":plan.ref_group,"ephemeral":plan.ephemeral}))?;
    Ok(CheckedPlan {
        fingerprint: format!("{:x}", Sha256::digest(bytes)),
        references,
    })
}

pub async fn external(input: ExternalInput) -> AppResult<serde_json::Value> {
    if input.version != 1 {
        return Err(AppError::InvalidInput("预检协议 version 只支持 1".into()));
    }
    let name = input.order.job_id.as_deref().unwrap_or("preflight");
    let plan = super::plan_with_inputs(Path::new("."), name, &input.order, Some(&input))
        .map_err(AppError::InvalidInput)?;
    let checked = check(&plan).await?;
    Ok(
        serde_json::json!({"status":"validated","validation":"full","protocolVersion":1,
        "appVersion":env!("CARGO_PKG_VERSION"),"taskCount":plan.task_count(),
        "refCount":checked.references.len(),"fingerprint":checked.fingerprint,
        "references":checked.references,
        "items":plan.items.iter().map(|item| serde_json::json!({"name":item.name,
            "paramsJson":item.params_json,"wireJson":item.wire_json})).collect::<Vec<_>>() }),
    )
}
