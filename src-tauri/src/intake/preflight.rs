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
    /// 外部成图（`mode=import`）：清单内相对路径 → 本地文件。生图单没有这一项。
    #[serde(default)]
    pub results: HashMap<String, String>,
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
    /// 外部成图逐条核对结果（与 `plan.items` 同序）；生图单为空。
    pub results: Vec<ReferenceCheck>,
}

/// 实际解码一张图并取内容摘要。参考图与外部成图共用同一口径。
async fn inspect(source: PathBuf, what: &str) -> AppResult<ReferenceCheck> {
    let path = source.clone();
    crate::files::decode::bounded(move |permit| {
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
        AppError::InvalidInput(format!("{what}解码失败 {}：{error}", source.display()))
    })
}

pub async fn check(plan: &Plan) -> AppResult<CheckedPlan> {
    let mut references = Vec::new();
    for source in plan.distinct_refs() {
        references.push(inspect(source, "参考图").await?);
    }
    let mut results = Vec::new();
    for item in &plan.items {
        if let Some(source) = &item.result_path {
            results.push(inspect(source.clone(), "成图").await?);
        }
    }
    if plan.import && results.len() != plan.items.len() {
        return Err(AppError::InvalidInput(
            "外部成图导入单每条都要有成图".into(),
        ));
    }
    let items: Vec<_> = plan
        .items
        .iter()
        .map(|item| {
            // 只有包内文件时与旧版逐字节相同，旧检查点的内容身份不变。
            let hashes: Vec<Option<String>> = item
                .slots
                .iter()
                .map(|slot| match slot {
                    super::RefSlot::File(path) => references
                        .iter()
                        .find(|reference| &reference.source == path)
                        .map(|r| r.sha256.clone()),
                    super::RefSlot::Library(spec) => Some(spec.fingerprint()),
                })
                .collect();
            serde_json::json!({"name":item.name,"sku":item.sku,"style":item.style,
            "prompt":item.prompt_text,"params":item.params_json,"refs":hashes})
        })
        .collect();
    // refGroup / ephemeral 已随参考图库下线；保留固定值，使旧检查点的内容身份不变。
    let mut identity = serde_json::json!({"items":items,"note":plan.note,
        "purpose":plan.purpose,"skill":plan.skill,"refGroup":null,"ephemeral":false});
    // 只有导入单才多这两项：生图单的内容身份与旧版逐字节相同。
    if plan.import {
        identity["mode"] = serde_json::json!(super::MODE_IMPORT);
        identity["results"] =
            serde_json::json!(results.iter().map(|r| &r.sha256).collect::<Vec<_>>());
    }
    let bytes = serde_json::to_vec(&identity)?;
    Ok(CheckedPlan {
        fingerprint: format!("{:x}", Sha256::digest(bytes)),
        references,
        results,
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
        "mode": if plan.import { super::MODE_IMPORT } else { "generate" },
        "results":checked.results,
        "refCount":checked.references.len(),"fingerprint":checked.fingerprint,
        "references":checked.references,
        "librarySlots":plan.library_slots().into_iter().map(|(item, slot, spec)| serde_json::json!({
            "item": plan.items[item].name, "slot": slot + 1, "reference": spec.describe(), "spec": spec,
        })).collect::<Vec<_>>(),
        "items":plan.items.iter().map(|item| serde_json::json!({"name":item.name,
            "paramsJson":item.params_json,"wireJson":item.wire_json})).collect::<Vec<_>>() }),
    )
}
