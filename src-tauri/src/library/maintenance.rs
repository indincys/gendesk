//! 显式运行的离线整理工具。先预览 / 备份，再复制校验、重绑外部引用、提交库，最后归档旧出口。
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};

use super::{copy_immutable, filename, Organization};
use crate::error::{AppError, AppResult};

#[derive(Default, Deserialize)]
#[serde(default)]
struct Assignments {
    tasks: BTreeMap<i64, Organization>,
    works: BTreeMap<i64, Organization>,
}

#[derive(Serialize)]
struct Entry {
    kind: String,
    id: i64,
    source: String,
    target: String,
    organization: Organization,
}

fn parse_org(raw: &str) -> AppResult<Organization> {
    if raw.is_empty() {
        Ok(Organization::default())
    } else {
        Ok(serde_json::from_str(raw)?)
    }
}

/// 维护入口只在显式的 CLI 中调用；不在启动时自动移动用户文件。
pub async fn organize(
    database: &Path,
    root: &Path,
    assignment_file: &Path,
    poster: Option<&Path>,
    report: &Path,
    apply: bool,
) -> AppResult<()> {
    if !database.is_file() || !root.is_absolute() {
        return Err(AppError::InvalidInput(
            "需要现有数据库与绝对作品根目录".into(),
        ));
    }
    let assignments: Assignments = serde_json::from_slice(&std::fs::read(assignment_file)?)?;
    let data_root = database
        .parent()
        .ok_or_else(|| AppError::InvalidInput("数据根目录不存在".into()))?;
    let raw = SqlitePool::connect(&format!("sqlite:{}", database.display())).await?;
    let busy: i64 = sqlx::query_scalar("SELECT (SELECT COUNT(*) FROM tasks WHERE status IN ('q','run','retry'))+(SELECT COUNT(*) FROM v2v_clips WHERE stage IN ('run','ready'))")
        .fetch_one(&raw).await?;
    if busy > 0 {
        return Err(AppError::InvalidInput(
            "请先完成或暂停在途任务，并退出 GenDesk 后整理".into(),
        ));
    }
    let backup = data_root.join("database-backups").join(format!(
        "{}_内容整理.db",
        chrono::Local::now().format("%Y-%m-%d_%H%M%S%f")
    ));
    if apply {
        std::fs::create_dir_all(
            backup
                .parent()
                .ok_or_else(|| AppError::InvalidInput("备份路径无效".into()))?,
        )?;
        sqlx::query("VACUUM INTO ?")
            .bind(backup.to_string_lossy().as_ref())
            .execute(&raw)
            .await?;
    }
    let preview_copy = if !apply {
        let parent = report
            .parent()
            .ok_or_else(|| AppError::InvalidInput("报告路径无效".into()))?;
        std::fs::create_dir_all(parent)?;
        let copy = parent.join(format!("{}-preview-copy.db", crate::poster::new_id()));
        sqlx::query("VACUUM INTO ?")
            .bind(copy.to_string_lossy().as_ref())
            .execute(&raw)
            .await?;
        Some(copy)
    } else {
        None
    };
    raw.close().await;
    // 预览只迁移隔离副本，不迁移运行库。
    let pool = if apply {
        crate::db::connect(database).await?
    } else {
        crate::db::connect(
            preview_copy
                .as_deref()
                .ok_or_else(|| AppError::Internal("缺少预览副本".into()))?,
        )
        .await?
    };
    let mut entries = Vec::<Entry>::new();
    let mut task_org = HashMap::<i64, Organization>::new();
    let mut params = HashMap::<i64, serde_json::Value>::new();
    for row in sqlx::query("SELECT t.*,COALESCE(p.sku_code,'') AS sku,COALESCE(p.style,'') AS style,COALESCE(p.item_name,'') AS item_name,COALESCE(g.name,b.note,'') AS requirement FROM tasks t LEFT JOIN prompts p ON p.id=t.prompt_id LEFT JOIN prompt_groups g ON g.id=p.group_id LEFT JOIN batches b ON b.id=t.batch_id").fetch_all(&pool).await? {
        let id: i64 = row.try_get("id")?;
        let raw: String = row.try_get("params_json")?;
        let org = assignments.tasks.get(&id).cloned().or(Organization::from_params(&raw)?)
            .unwrap_or_else(|| Organization::legacy(row.get("sku"),row.get("style"),row.get("requirement")));
        org.validate()?;
        let mut value: serde_json::Value = serde_json::from_str(if raw.is_empty() {"{}"} else {&raw})?;
        value["organization"] = serde_json::to_value(&org)?;
        params.insert(id,value); task_org.insert(id,org.clone());
        let source: Option<String> = row.try_get("result_image_path")?;
        let status: String = row.try_get("status")?;
        if let Some(source) = source.filter(|p| Path::new(p).is_file()) {
            // 已通过的源结果属于恢复快照，保持在应用内部；仅迁移用户的成品出口。
            if status != "pass" && status != "rej" {
                let ext = Path::new(&source).extension().and_then(|e| e.to_str()).unwrap_or("png");
                let at: i64 = row.try_get("created_at")?;
                let dir = org.directory(root,false,false);
                let name: String = row.try_get("item_name")?;
                let target = dir.join(filename(if name.is_empty() {&org.requirement} else {&name},&format!("task{id}"),at,ext));
                entries.push(Entry{kind:"task".into(),id,source,target:target.to_string_lossy().into(),organization:org});
            }
        }
    }
    let mut works = HashMap::<i64, String>::new();
    let mut uids = HashMap::<String, String>::new();
    for row in sqlx::query("SELECT * FROM accepted_works")
        .fetch_all(&pool)
        .await?
    {
        let id: i64 = row.try_get("id")?;
        let task_id: Option<i64> = row.try_get("task_id")?;
        let saved: String = row.try_get("organization_json").unwrap_or_default();
        let org = assignments
            .works
            .get(&id)
            .cloned()
            .or(task_id.and_then(|t| task_org.get(&t).cloned()))
            .unwrap_or(parse_org(&saved)?);
        let org = if org == Organization::default() {
            Organization::legacy(row.get("sku_code"), row.get("style"), row.get("group_name"))
        } else {
            org
        };
        org.validate()?;
        let source: String = row.try_get("image_path")?;
        if !Path::new(&source).is_file() {
            return Err(AppError::InvalidInput(format!(
                "成品源文件不存在，未整理：{source}"
            )));
        }
        let uid: String = row.try_get("uid")?;
        let identity = if uid.is_empty() {
            format!("work{id}")
        } else {
            uid.clone()
        };
        let ext = Path::new(&source)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("png");
        let name: String = row.try_get("item_name")?;
        let target = org
            .directory(root, true, false)
            .join(filename(
                if name.is_empty() {
                    &org.requirement
                } else {
                    &name
                },
                &identity,
                row.try_get("accepted_at")?,
                ext,
            ))
            .to_string_lossy()
            .to_string();
        works.insert(id, target.clone());
        uids.insert(uid, target.clone());
        entries.push(Entry {
            kind: "work".into(),
            id,
            source,
            target,
            organization: org,
        });
    }
    for clip in crate::db::repo::v2v::list_by_stages(&pool, &["pass"]).await? {
        if clip.stage != "pass" {
            continue;
        }
        let Some(source) = clip
            .export_path
            .clone()
            .or(clip.video_path.clone())
            .filter(|p| Path::new(p).is_file())
        else {
            continue;
        };
        let org = entries
            .iter()
            .find(|e| e.kind == "work" && e.id == clip.work_id)
            .map(|e| e.organization.clone())
            .unwrap_or(parse_org(&clip.organization_json)?);
        let org = if org == Organization::default() {
            Organization::legacy("", "", &clip.group_name)
        } else {
            org
        };
        let ext = Path::new(&source)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("mp4");
        let attempt = if clip.attempt > 1 {
            format!("_{}", clip.attempt)
        } else {
            String::new()
        };
        let target = org.directory(root, true, true).join(filename(
            &org.requirement,
            &format!("clip{}{attempt}", clip.id),
            clip.reviewed_at.unwrap_or(clip.created_at),
            ext,
        ));
        entries.push(Entry {
            kind: "clip".into(),
            id: clip.id,
            source,
            target: target.to_string_lossy().into(),
            organization: org,
        });
    }
    let plan = serde_json::json!({"applied":false,"backup":backup,"root":root,"entries":entries});
    crate::poster::storage::atomic_write(report, &serde_json::to_vec_pretty(&plan)?)?;
    if !apply {
        pool.close().await;
        if let Some(copy) = preview_copy {
            std::fs::remove_file(copy)?;
        }
        return Ok(());
    }
    let mut targets = HashMap::<String, String>::new();
    for e in &entries {
        // 在大小写不敏感文件系统上也验证实际内容；同名不同文件不能悄悄覆盖。
        if let Some(previous) = targets.insert(e.target.to_lowercase(), e.source.clone()) {
            if std::fs::read(previous)? != std::fs::read(&e.source)? {
                return Err(AppError::InvalidInput(format!(
                    "归属文件名冲突：{}",
                    e.target
                )));
            }
        }
        copy_immutable(Path::new(&e.source), Path::new(&e.target))?;
    }
    let mut requests = Vec::new();
    for row in sqlx::query(
        "SELECT id,state,request_json FROM poster_batches WHERE request_json IS NOT NULL",
    )
    .fetch_all(&pool)
    .await?
    {
        let id: String = row.try_get("id")?;
        let state: String = row.try_get("state")?;
        let mut request: crate::poster::client::Request =
            serde_json::from_str(row.try_get("request_json")?)?;
        let mut relocated = request.clone();
        relocated.items.clear();
        for item in &mut request.items {
            // Poster 身份独立于 live work：作品已删除时保留原引用，不能因此中止其他整理。
            if let Some(path) = uids.get(&item.external_id) {
                if &item.path != path {
                    item.path = path.clone();
                    relocated.items.push(item.clone());
                }
            }
        }
        if matches!(state.as_str(), "done" | "queued" | "failed") && !relocated.items.is_empty() {
            let executable = poster.ok_or_else(|| {
                AppError::InvalidInput("已导入过 Poster，需要提供支持同图重绑的 Poster CLI".into())
            })?;
            let manifest = data_root.join("作品记录").join(format!("重绑_{id}.json"));
            crate::poster::storage::atomic_write(
                &manifest,
                &serde_json::to_vec_pretty(&relocated)?,
            )?;
            let preview =
                crate::poster::client::preview_relocation(executable, &manifest, &relocated)
                    .await?;
            if state == "done" && preview.items.iter().any(|i| i.status == "ready") {
                return Err(AppError::InvalidInput(
                    "已完成导入的图片在 Poster 中缺失，保留原文件待核对".into(),
                ));
            }
            // 回执丢失的 queued/failed 也可能已在 Poster 提交；只重绑已存在身份，不借整理创建新图。
            relocated.items.retain(|i| {
                preview.items.iter().any(|r| {
                    r.external_id == i.external_id
                        && matches!(r.status.as_str(), "duplicate" | "deleted")
                })
            });
            if !relocated.items.is_empty() {
                crate::poster::storage::atomic_write(
                    &manifest,
                    &serde_json::to_vec_pretty(&relocated)?,
                )?;
                let receipt =
                    crate::poster::client::import(executable, &manifest, &relocated).await?;
                if receipt.items.iter().any(|i| i.status == "imported") {
                    return Err(AppError::Internal(
                        "整理时出现意外新图片，保留全部原文件待核对".into(),
                    ));
                }
            }
        }
        requests.push((id, request));
    }
    let mut tx = pool.begin().await?;
    for (id, value) in params {
        sqlx::query("UPDATE tasks SET params_json=? WHERE id=?")
            .bind(value.to_string())
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    for e in &entries {
        match e.kind.as_str() {
            "work" => {
                sqlx::query(
                    "UPDATE accepted_works SET image_path=?,organization_json=? WHERE id=?",
                )
                .bind(&e.target)
                .bind(serde_json::to_string(&e.organization)?)
                .bind(e.id)
                .execute(&mut *tx)
                .await?;
                sqlx::query("UPDATE v2v_clips SET organization_json=? WHERE work_id=?")
                    .bind(serde_json::to_string(&e.organization)?)
                    .bind(e.id)
                    .execute(&mut *tx)
                    .await?;
                sqlx::query("UPDATE poster_images SET image_path=? WHERE uid=(SELECT uid FROM accepted_works WHERE id=?)")
                    .bind(&e.target)
                    .bind(e.id)
                    .execute(&mut *tx)
                    .await?;
            }
            "task" => {
                sqlx::query("UPDATE tasks SET result_image_path=? WHERE id=?")
                    .bind(&e.target)
                    .bind(e.id)
                    .execute(&mut *tx)
                    .await?;
            }
            "clip" => {
                sqlx::query("UPDATE v2v_clips SET export_path=?,organization_json=? WHERE id=?")
                    .bind(&e.target)
                    .bind(serde_json::to_string(&e.organization)?)
                    .bind(e.id)
                    .execute(&mut *tx)
                    .await?;
            }
            _ => {}
        }
    }
    for (id, request) in requests {
        sqlx::query("UPDATE poster_batches SET request_json=? WHERE id=?")
            .bind(serde_json::to_string(&request)?)
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    let mut app_settings =
        crate::commands::settings::load_settings(&pool, &root.to_string_lossy()).await?;
    app_settings.output_dir = root.to_string_lossy().into();
    sqlx::query("INSERT INTO settings(key,value_json) VALUES('app',?) ON CONFLICT(key) DO UPDATE SET value_json=excluded.value_json")
        .bind(serde_json::to_string(&app_settings)?).execute(&mut *tx).await?;
    // 废纸篓载荷里的路径也保持可还原。只替换完整路径字符串，不模糊替换文本。
    let relocation: HashMap<&str, &str> = entries
        .iter()
        .map(|e| (e.source.as_str(), e.target.as_str()))
        .collect();
    for row in sqlx::query("SELECT id,file_paths_json,payload_json FROM trash_items")
        .fetch_all(&mut *tx)
        .await?
    {
        let id: i64 = row.try_get("id")?;
        for field in ["file_paths_json", "payload_json"] {
            let raw: Option<String> = row.try_get(field)?;
            if let Some(raw) = raw {
                if let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&raw) {
                    replace_paths(&mut value, &relocation);
                    sqlx::query(&format!("UPDATE trash_items SET {field}=? WHERE id=?"))
                        .bind(value.to_string())
                        .bind(id)
                        .execute(&mut *tx)
                        .await?;
                }
            }
        }
    }
    tx.commit().await?;
    crate::poster::storage::atomic_write(
        report,
        &serde_json::to_vec_pretty(
            &serde_json::json!({"applied":true,"backup":backup,"root":root,"entries":entries}),
        )?,
    )?;
    pool.close().await;
    Ok(())
}

fn replace_paths(value: &mut serde_json::Value, paths: &HashMap<&str, &str>) {
    match value {
        serde_json::Value::String(s) => {
            if let Some(new) = paths.get(s.as_str()) {
                *s = (*new).into();
            }
        }
        serde_json::Value::Array(a) => {
            for v in a {
                replace_paths(v, paths);
            }
        }
        serde_json::Value::Object(o) => {
            for v in o.values_mut() {
                replace_paths(v, paths);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 临时运行库和文件的迁移行为验证。
mod tests {
    use super::*;
    #[cfg(unix)]
    #[tokio::test]
    async fn uncertain_poster_receipts_relink_existing_images_without_importing_unreceived_images()
    {
        use std::os::unix::fs::PermissionsExt;
        let (pool, dir) = crate::db::test_support::test_pool().await;
        let source = dir.path().join("source.png");
        std::fs::write(&source, b"image").unwrap();
        sqlx::query("INSERT INTO accepted_works(id,image_path,thumb_path,prompt_text,accepted_at,uid,item_name) VALUES(1,?,'/thumb','prompt',1,'abcd','场景展示')")
            .bind(source.to_string_lossy().as_ref()).execute(&pool).await.unwrap();
        let assignments = dir.path().join("assignments.json");
        std::fs::write(
            &assignments,
            r#"{"works":{"1":{"product":"水杯","style":"白色款","requirement":"厨房场景"}}}"#,
        )
        .unwrap();
        let executable = dir.path().join("Poster");
        std::fs::write(&executable,r##"#!/usr/bin/env python3
import sys,json,pathlib
root=pathlib.Path(__file__).parent
if sys.argv[1]=='image-import-spec':
 print(json.dumps({'capabilities':['same-identity-path-relink']}))
else:
 data=json.loads(pathlib.Path(sys.argv[2]).read_text())
 status=(root/'preview-status').read_text()
 apply='--apply' in sys.argv
 if apply:
  assert status=='duplicate', '整理不能创建尚未导入的图片'
  (root/'relinked').write_text(data['items'][0]['path'])
 data['applied']=apply
 data['items']=[{'externalId':i['externalId'],'productId':i['productId'],'imageId':'existing-image','status':status} for i in data['items']]
 print(json.dumps(data))
"##).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let root = dir.path().join("作品");
        for (state, status) in [("failed", "duplicate"), ("queued", "ready")] {
            let _ = std::fs::remove_file(dir.path().join("relinked"));
            std::fs::write(dir.path().join("preview-status"), status).unwrap();
            sqlx::query("DELETE FROM poster_batches")
                .execute(&pool)
                .await
                .unwrap();
            let request=serde_json::json!({"schema":"poster.images/1","sourceId":"source","batchId":"batch","items":[{"externalId":"abcd","productId":"product","path":source,"sha256":"hash","byteSize":5,"source":"ai"}]}).to_string();
            sqlx::query("INSERT INTO poster_batches(id,created_at,state,request_json) VALUES('batch',1,?,?)")
                .bind(state).bind(request).execute(&pool).await.unwrap();
            organize(
                &dir.path().join("test.db"),
                &root,
                &assignments,
                Some(&executable),
                &dir.path().join("report.json"),
                true,
            )
            .await
            .unwrap();
            assert_eq!(dir.path().join("relinked").exists(), status == "duplicate");
            let saved: String =
                sqlx::query_scalar("SELECT request_json FROM poster_batches WHERE id='batch'")
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            let saved: crate::poster::client::Request = serde_json::from_str(&saved).unwrap();
            assert!(Path::new(&saved.items[0].path).starts_with(&root));
            let preserved: String =
                sqlx::query_scalar("SELECT state FROM poster_batches WHERE id='batch'")
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(preserved, state);
        }
    }

    #[tokio::test]
    async fn organization_preview_is_read_only_and_apply_preserves_files_identity_and_replay() {
        let (pool, dir) = crate::db::test_support::test_pool().await;
        let source = dir.path().join("old.png");
        std::fs::write(&source, b"original-image").unwrap();
        sqlx::query("INSERT INTO accepted_works(id,image_path,thumb_path,prompt_text,accepted_at,uid,item_name) VALUES(1,?,'/thumb','prompt',1,'abcd','杯子场景')")
            .bind(source.to_string_lossy().as_ref()).execute(&pool).await.unwrap();
        let assignments = dir.path().join("assignments.json");
        std::fs::write(&assignments,r#"{"works":{"1":{"product":"陶瓷水杯","style":"奶油白款","requirement":"厨房场景图"}}}"#).unwrap();
        let root = dir.path().join("作品");
        let report = dir.path().join("report.json");
        let db = dir.path().join("test.db");
        organize(&db, &root, &assignments, None, &report, false)
            .await
            .unwrap();
        assert!(!root.exists());
        let before: String = sqlx::query_scalar("SELECT image_path FROM accepted_works WHERE id=1")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(before, source.to_string_lossy());
        // 已删除的历史作品仍可被 Poster 引用，不能要求它有 live work 或重新导入它。
        let detached=serde_json::json!({"schema":"poster.images/1","sourceId":"source","batchId":"retired","items":[{"externalId":"retired-image","productId":"product","path":"/retained/old.png","sha256":"hash","byteSize":1,"source":"ai"}]}).to_string();
        sqlx::query("INSERT INTO poster_batches(id,created_at,state,request_json) VALUES('retired',1,'done',?)").bind(&detached).execute(&pool).await.unwrap();
        organize(&db, &root, &assignments, None, &report, true)
            .await
            .unwrap();
        let first: String = sqlx::query_scalar("SELECT image_path FROM accepted_works WHERE id=1")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(
            Path::new(&first).starts_with(root.join("成品/生成图片/陶瓷水杯/奶油白款/厨房场景图"))
        );
        assert_eq!(std::fs::read(&first).unwrap(), b"original-image");
        assert!(source.is_file(), "旧文件在重绑与核对前不能清理");
        organize(&db, &root, &assignments, None, &report, true)
            .await
            .unwrap();
        let second: String = sqlx::query_scalar("SELECT image_path FROM accepted_works WHERE id=1")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(first, second);
        let uid: String = sqlx::query_scalar("SELECT uid FROM accepted_works WHERE id=1")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(uid, "abcd");
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM accepted_works")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
        let kept: String =
            sqlx::query_scalar("SELECT request_json FROM poster_batches WHERE id='retired'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&kept).unwrap(),
            serde_json::from_str::<serde_json::Value>(&detached).unwrap()
        );
        assert_eq!(
            crate::library::root(&pool).await.unwrap(),
            root,
            "缺少旧app设置时也必须保存完整设置，重启仍用同一根目录"
        );
        let result: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&report).unwrap()).unwrap();
        assert!(Path::new(result["backup"].as_str().unwrap()).is_file());
    }
}
