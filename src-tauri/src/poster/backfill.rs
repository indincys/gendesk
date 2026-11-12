//! 补导历史成品：自动导入关闭期间（或开关上线之前）通过的作品从未登记 Poster 批次。
//!
//! 只补「从未登记过」的作品：已导入、失败待处理、用户选了「不导入」的都已有条目，
//! 不会被再次登记。默认只预演；`apply` 时只为能对上商品的款式建批，对不上的款式
//! 原样列出，用户绑定后再跑一次即可补上（已登记的不会重复）。
//! 只补静态图：实况配对完成后队列行即删除，事后无法可靠找回原配对。
use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;
use sqlx::{FromRow, SqlitePool};

use super::{bindings, check_with, client, fallback_root, load_settings, new_id, TargetCheck};
use crate::db::now_unix;
use crate::error::{AppError, AppResult};
use crate::library::Organization;

/// 一批最多多少张：Poster 单次导入有超时，历史成品可能上百张。
const CHUNK: usize = 50;

#[derive(Debug, Default, Clone)]
pub struct Filter {
    pub product: Option<String>,
    pub style: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackfillGroup {
    #[serde(flatten)]
    pub target: TargetCheck,
    /// 可补导的张数（成品文件仍在）。
    pub count: usize,
    /// 成品文件已不在、无法补导的张数。
    pub missing_files: usize,
    /// 本次新建的批次数（预演时为 0）。
    pub batches: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackfillReport {
    pub applied: bool,
    pub auto_import: bool,
    pub groups: Vec<BackfillGroup>,
    /// 本次新登记的张数与批次数。
    pub images: usize,
    pub batches: usize,
}

#[derive(Debug, FromRow)]
struct Work {
    id: i64,
    uid: String,
    item_name: String,
    sku_code: String,
    style: String,
    organization_json: String,
    image_path: String,
}

/// 与验收时 `Organization::poster_keys` 同一口径：作品归属快照优先，旧作品沿用 SKU / 款式。
fn key_of(work: &Work) -> (String, String) {
    let organization: Organization =
        serde_json::from_str(&work.organization_json).unwrap_or_default();
    organization.poster_keys(&work.sku_code, &work.style)
}

fn wanted(filter: &Filter, (sku, style): &(String, String)) -> bool {
    filter.product.as_deref().map_or(true, |p| p == sku)
        && filter.style.as_deref().map_or(true, |s| s == style)
}

pub async fn backfill(
    pool: &SqlitePool,
    data_root: &Path,
    filter: &Filter,
    apply: bool,
) -> AppResult<BackfillReport> {
    let settings = load_settings(pool).await?;
    if apply && !settings.auto_import {
        return Err(AppError::InvalidInput(
            "请先在设置 › 审稿后自动化里打开「自动导入 Poster」再补导".into(),
        ));
    }
    let works: Vec<Work> = sqlx::query_as(
        "SELECT w.id,w.uid,w.item_name,w.sku_code,w.style,w.organization_json,w.image_path
        FROM accepted_works w
        WHERE w.uid<>'' AND NOT EXISTS
            (SELECT 1 FROM poster_images i WHERE i.work_id=w.id AND i.uid=w.uid)
        ORDER BY w.id",
    )
    .fetch_all(pool)
    .await?;
    let mut grouped: BTreeMap<(String, String), (Vec<Work>, usize)> = BTreeMap::new();
    for work in works {
        let key = key_of(&work);
        if !wanted(filter, &key) {
            continue;
        }
        let entry = grouped.entry(key).or_default();
        if Path::new(&work.image_path).is_file() {
            entry.0.push(work);
        } else {
            entry.1 += 1;
        }
    }
    let keys = grouped.keys().cloned().collect::<Vec<_>>();
    let products = if keys.is_empty() {
        Vec::new()
    } else {
        client::products(Path::new(&settings.executable)).await?
    };
    let checks = check_with(&keys, &products, &bindings(pool).await?);
    let mut report = BackfillReport {
        applied: apply,
        auto_import: settings.auto_import,
        groups: Vec::new(),
        images: 0,
        batches: 0,
    };
    let library_root = fallback_root(data_root).to_string_lossy().into_owned();
    for (target, ((_, _), (works, missing))) in checks.into_iter().zip(grouped) {
        let mut batches = 0;
        if apply && target.target_id.is_some() {
            for chunk in works.chunks(CHUNK) {
                let id = new_id();
                let mut tx = pool.begin().await?;
                sqlx::query(
                    "INSERT INTO poster_batches(id,created_at,state,library_root)
                    VALUES(?,?,'queued',?)",
                )
                .bind(&id)
                .bind(now_unix())
                .bind(&library_root)
                .execute(&mut *tx)
                .await?;
                for work in chunk {
                    sqlx::query(
                        "INSERT INTO poster_images(batch_id,work_id,uid,item_name,sku,style,image_path)
                        VALUES(?,?,?,?,?,?,?)",
                    )
                    .bind(&id)
                    .bind(work.id)
                    .bind(&work.uid)
                    .bind(&work.item_name)
                    .bind(&target.product)
                    .bind(&target.style)
                    .bind(&work.image_path)
                    .execute(&mut *tx)
                    .await?;
                }
                tx.commit().await?;
                batches += 1;
            }
            report.images += works.len();
            report.batches += batches;
        }
        report.groups.push(BackfillGroup {
            target,
            count: works.len(),
            missing_files: missing,
            batches,
        });
    }
    Ok(report)
}
