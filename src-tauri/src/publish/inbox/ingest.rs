//! 商品级文案收件箱的幂等全量扫描。

use std::path::{Path, PathBuf};

use sqlx::SqlitePool;
use walkdir::WalkDir;

use crate::db::repo::copy as copy_repo;
use crate::error::AppResult;
use crate::publish::{copy_ingest, paths};

#[derive(Debug, Clone)]
pub struct IngestResult {
    pub file_name: String,
    pub state: String,
    pub product_code: Option<String>,
    pub titles: i64,
    pub bodies: i64,
    pub message: String,
    pub changed: bool,
}

fn relative(root: &Path, path: &Path) -> paths::RelPath {
    let inbox = paths::RelPath::new(paths::INBOX).to_local(root);
    paths::RelPath::from_parts([
        paths::INBOX,
        path.strip_prefix(inbox)
            .unwrap_or(path)
            .to_string_lossy()
            .as_ref(),
    ])
}

fn is_archived(path: &Path, inbox: &Path) -> bool {
    path.strip_prefix(inbox)
        .ok()
        .and_then(|rel| rel.components().next())
        .and_then(|part| part.as_os_str().to_str())
        .is_some_and(|part| paths::INBOX_ARCHIVES.contains(&part))
}

fn product_hint(path: &Path, inbox: &Path) -> Option<String> {
    path.strip_prefix(inbox)
        .ok()
        .and_then(|rel| rel.components().next())
        .and_then(|part| part.as_os_str().to_str())
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_uppercase())
}

fn archive(root: &Path, source: &Path) -> AppResult<PathBuf> {
    let date = chrono::Local::now().format("%Y%m%d").to_string();
    let dir = paths::RelPath::from_parts([paths::INBOX, paths::INGESTED, &date]).to_local(root);
    std::fs::create_dir_all(&dir)?;
    let name = source
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("文案.txt");
    let mut target = dir.join(name);
    let mut suffix = 2;
    while target.exists() {
        let stem = source
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("文案");
        target = dir.join(format!("{stem}_{suffix}.txt"));
        suffix += 1;
    }
    std::fs::rename(source, &target)?;
    Ok(target)
}

async fn record_pending(
    pool: &SqlitePool,
    rel: &paths::RelPath,
    state: &str,
    code: Option<&str>,
    message: &str,
) -> AppResult<bool> {
    let existing: Option<(i64, String, Option<String>)> = sqlx::query_as(
        "SELECT id,state,detail_json FROM inbox_items WHERE file_rel=?1 ORDER BY id DESC LIMIT 1",
    )
    .bind(rel.as_str())
    .fetch_optional(pool)
    .await?;
    let detail = serde_json::json!({ "message": message }).to_string();
    match existing {
        Some((id, old_state, old_detail)) => {
            let changed = old_state != state || old_detail.as_deref() != Some(&detail);
            sqlx::query("UPDATE inbox_items SET state=?2,sku_code=?3,detail_json=?4 WHERE id=?1")
                .bind(id)
                .bind(state)
                .bind(code)
                .bind(detail)
                .execute(pool)
                .await?;
            Ok(changed)
        }
        None => {
            sqlx::query(
                "INSERT INTO inbox_items(file_rel,kind,sku_code,state,detail_json,created_at)
                 VALUES(?1,'copy',?2,?3,?4,?5)",
            )
            .bind(rel.as_str())
            .bind(code)
            .bind(state)
            .bind(detail)
            .bind(crate::db::now_unix())
            .execute(pool)
            .await?;
            Ok(true)
        }
    }
}

async fn ingest_one(pool: &SqlitePool, root: &Path, path: &Path) -> AppResult<IngestResult> {
    let inbox = paths::RelPath::new(paths::INBOX).to_local(root);
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_string();
    let rel = relative(root, path);
    let bytes = std::fs::read(path)?;
    let text = match String::from_utf8(bytes.clone()) {
        Ok(value) => value,
        Err(_) => {
            let message = "文案文件必须是 UTF-8";
            let changed = record_pending(pool, &rel, "failed", None, message).await?;
            return Ok(IngestResult {
                file_name,
                state: "failed".into(),
                product_code: None,
                titles: 0,
                bodies: 0,
                message: message.into(),
                changed,
            });
        }
    };
    let parsed = match copy_ingest::parse(&text, &file_name) {
        Ok(value) => value,
        Err(message) => {
            let changed = record_pending(pool, &rel, "failed", None, &message).await?;
            return Ok(IngestResult {
                file_name,
                state: "failed".into(),
                product_code: None,
                titles: 0,
                bodies: 0,
                message,
                changed,
            });
        }
    };
    let code = parsed
        .product_code
        .clone()
        .or_else(|| product_hint(path, &inbox));
    let product: Option<(i64, String)> = match code.as_deref() {
        Some(code) => {
            sqlx::query_as("SELECT id,code FROM products WHERE code=?1 COLLATE NOCASE")
                .bind(code)
                .fetch_optional(pool)
                .await?
        }
        None => None,
    };
    let Some((product_id, product_code)) = product else {
        let message = "识别不到商品，文件保留原位等待认领";
        let changed = record_pending(pool, &rel, "unclaimed", code.as_deref(), message).await?;
        return Ok(IngestResult {
            file_name,
            state: "unclaimed".into(),
            product_code: code,
            titles: 0,
            bodies: 0,
            message: message.into(),
            changed,
        });
    };
    let hash = copy_ingest::content_hash(&bytes);
    let mut tx = pool.begin().await?;
    let inserted = sqlx::query(
        "INSERT OR IGNORE INTO copy_ingest_hashes(content_hash,file_rel,created_at) VALUES(?1,?2,?3)",
    )
    .bind(&hash)
    .bind(rel.as_str())
    .bind(crate::db::now_unix())
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if inserted == 0 {
        tx.rollback().await?;
        let archived = archive(root, path)?;
        return Ok(IngestResult {
            file_name,
            state: "duplicate".into(),
            product_code: Some(product_code),
            titles: 0,
            bodies: 0,
            message: format!("内容已收录，重复文件已移入 {}", archived.display()),
            changed: false,
        });
    }
    for title in &parsed.titles {
        copy_repo::insert_copy(&mut tx, product_id, "title", title, "inbox").await?;
    }
    for body in &parsed.bodies {
        copy_repo::insert_copy(&mut tx, product_id, "body", body, "inbox").await?;
    }
    if !parsed.topics.is_empty() {
        let exists: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM topic_groups WHERE product_id=?1 AND scope='product'",
        )
        .bind(product_id)
        .fetch_one(&mut *tx)
        .await?;
        if exists == 0 {
            let now = crate::db::now_unix();
            sqlx::query(
                "INSERT INTO topic_groups(product_id,scope,tags_json,created_at,updated_at)
                 VALUES(?1,'product',?2,?3,?3)",
            )
            .bind(product_id)
            .bind(serde_json::to_string(&parsed.topics)?)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }
    }
    sqlx::query(
        "INSERT INTO inbox_items(file_rel,kind,sku_code,state,detail_json,created_at)
         VALUES(?1,?2,?3,'ingested',?4,?5)",
    )
    .bind(rel.as_str())
    .bind(&parsed.kind)
    .bind(&product_code)
    .bind(
        serde_json::json!({ "titles": parsed.titles.len(), "bodies": parsed.bodies.len() })
            .to_string(),
    )
    .bind(crate::db::now_unix())
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    let archived = archive(root, path)?;
    sqlx::query("UPDATE inbox_items SET file_rel=?2 WHERE file_rel=?1 AND state='ingested'")
        .bind(rel.as_str())
        .bind(relative(root, &archived).as_str())
        .execute(pool)
        .await?;
    Ok(IngestResult {
        file_name,
        state: "ingested".into(),
        product_code: Some(product_code),
        titles: parsed.titles.len() as i64,
        bodies: parsed.bodies.len() as i64,
        message: "已自动收录并移入已收录".into(),
        changed: true,
    })
}

pub async fn rescan(pool: &SqlitePool, root: &Path) -> AppResult<Vec<IngestResult>> {
    let inbox = paths::RelPath::new(paths::INBOX).to_local(root);
    std::fs::create_dir_all(&inbox)?;
    let files: Vec<PathBuf> = WalkDir::new(&inbox)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| entry.into_path())
        .filter(|path| {
            path.extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case("txt"))
        })
        .filter(|path| !is_archived(path, &inbox))
        .collect();
    let mut out = Vec::new();
    for file in files {
        match ingest_one(pool, root, &file).await {
            Ok(result) => out.push(result),
            Err(err) => out.push(IngestResult {
                file_name: file
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or("")
                    .into(),
                state: "failed".into(),
                product_code: None,
                titles: 0,
                bodies: 0,
                message: err.to_string(),
                changed: true,
            }),
        }
    }
    Ok(out)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // 测试断言失败即测试失败
mod tests {
    use super::*;
    use crate::db::test_support::test_pool;

    async fn seed_product(pool: &SqlitePool) {
        sqlx::query(
            "INSERT INTO products(id,code,name,created_at,updated_at) VALUES(1,'A','商品 A',0,0)",
        )
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn scan_ingests_archives_deduplicates_and_keeps_unclaimed_file() {
        let (pool, _db_dir) = test_pool().await;
        seed_product(&pool).await;
        let root_dir = tempfile::tempdir().unwrap();
        let root = root_dir.path();
        let product_dir = paths::RelPath::from_parts([paths::INBOX, "A"]).to_local(root);
        std::fs::create_dir_all(&product_dir).unwrap();
        let source = "【商品】A\n【类型】正文\n\n第一条\n====\n第二条";
        let first = product_dir.join("正文.txt");
        std::fs::write(&first, source).unwrap();

        let first_results = rescan(&pool, root).await.unwrap();
        assert_eq!(first_results.len(), 1);
        assert_eq!(first_results[0].state, "ingested");
        assert_eq!(first_results[0].bodies, 2);
        assert!(!first.exists(), "成功后原文件必须移入已收录");
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM text_items")
                .fetch_one(&pool)
                .await
                .unwrap(),
            2
        );

        let duplicate = product_dir.join("正文-重复.txt");
        std::fs::write(&duplicate, source).unwrap();
        let duplicate_results = rescan(&pool, root).await.unwrap();
        assert_eq!(duplicate_results[0].state, "duplicate");
        assert!(
            !duplicate.exists(),
            "重复文件也应归档，避免 watcher 一直重扫"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM text_items")
                .fetch_one(&pool)
                .await
                .unwrap(),
            2,
            "同一内容只能入池一次"
        );

        let unclaimed = paths::RelPath::new(paths::INBOX)
            .to_local(root)
            .join("未知商品-标题.txt");
        std::fs::write(&unclaimed, "【商品】NOPE\n【类型】标题\n\n待认领标题").unwrap();
        let unclaimed_results = rescan(&pool, root).await.unwrap();
        assert_eq!(unclaimed_results[0].state, "unclaimed");
        assert!(unclaimed.exists(), "识别不到商品时不得移动或删除原文件");
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM inbox_items WHERE state='unclaimed'"
            )
            .fetch_one(&pool)
            .await
            .unwrap(),
            1
        );
    }
}
