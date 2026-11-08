//! works 域命令（作品库，执行计划 2.1 / 需求 14.4）。

use serde::{Deserialize, Serialize};
use specta::Type;
use sqlx::FromRow;
use tauri::State;

use crate::db::repo::{trash as trash_repo, works as work_repo};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

#[derive(Debug, Clone, Serialize, Type, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct WorkView {
    pub id: i64,
    pub prompt_code: String,
    pub group_name: String,
    pub ref_name: String,
    pub batch_id: Option<i64>,
    pub favorite: i64,
    pub accepted_at: i64,
    pub image_path: String,
    pub thumb_path: String,
    pub prompt_text: String,
    /// 复刻/再生成所需的原始关联（E33）；批次删除后 task_id 可能为空。
    pub ref_image_id: Option<i64>,
    pub group_id: Option<i64>,
    pub task_id: Option<i64>,
    /// 已在视频流水线里（任一阶段）。卡片角标用，避免把同一张图重复入队。
    pub in_pipeline: bool,
    /// 其提示词组用途 = 图生视频。卡片角标 + 「本批全选」的默认取样。
    pub is_i2v: bool,
}

#[derive(Debug, Clone, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct WorkFilter {
    pub group_id: Option<i64>,
    pub favorite_only: bool,
    /// 按**用途**筛选（受控取值，当前只有「图生视频」）。
    ///
    /// 读的是作品自己的 `purpose` 列（0045），不再 JOIN `tag_bindings`：
    /// 那是张无外键的多态表，分组一删、id 一复用，旧绑定就会把用途安到无关的组头上。
    /// 自由标签（txt 里写的 `标签: 白底,3C`）仍然绑在分组上，但它不是这条筛选的事。
    pub tag: Option<String>,
    /// 全文搜索：编号 / 分组名 / 参考图名 / 提示词正文。
    ///
    /// 分组是「一份 txt = 一个组」的产物，会长到几十上百个——它天然是**出货单位**而不是
    /// 分类法，永远不会是好的浏览轴。搜索 + 批次分节才是找回一张历史图的实际路径。
    #[serde(default)]
    pub query: Option<String>,
    /// 只看某一批次。
    #[serde(default)]
    pub batch_id: Option<i64>,
}

/// 编号与组名读**作品自己的快照列**（0027），不再 JOIN prompts/prompt_groups。
///
/// 提示词成了消耗品：批次跑完即随批次一起删掉。若继续现读，作品库的编号、分组分节、
/// 全文搜索会在上游被清理的那一刻集体变空，而作品是长期资产，不该跟着上游一起失忆。
/// （同 0020 给 v2v_clips 冗余 group_name 的理由——下游存快照。）
const WORK_SELECT: &str = "SELECT w.id, w.prompt_code, w.group_name,
        COALESCE(r.name,'') AS ref_name,
        w.batch_id, w.favorite, w.accepted_at, w.image_path, w.thumb_path, w.prompt_text,
        w.ref_image_id, w.group_id, w.task_id,
        EXISTS (SELECT 1 FROM v2v_clips c WHERE c.work_id = w.id) AS in_pipeline,
        (w.purpose = '图生视频') AS is_i2v
    FROM accepted_works w
    LEFT JOIN ref_images r ON r.id = w.ref_image_id";

#[tauri::command]
#[specta::specta]
pub async fn list_works(
    state: State<'_, AppState>,
    filter: WorkFilter,
    page: Option<i64>,
) -> AppResult<Vec<WorkView>> {
    let mut sql = String::from(WORK_SELECT);
    let mut conds: Vec<String> = Vec::new();
    if filter.group_id.is_some() {
        conds.push("w.group_id = ?".into());
    }
    if filter.favorite_only {
        conds.push("w.favorite = 1".into());
    }
    if filter.tag.is_some() {
        conds.push("w.purpose = ?".into());
    }
    if filter.batch_id.is_some() {
        conds.push("w.batch_id = ?".into());
    }
    // 全文搜索一次覆盖四处：编号 / 分组名 / 参考图名 / 提示词正文。
    // 分开成四个筛选器毫无意义——人搜的时候并不知道自己记住的是哪一处。
    let query = filter
        .query
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| format!("%{s}%"));
    if query.is_some() {
        conds.push(
            "(w.prompt_code LIKE ? OR w.group_name LIKE ? OR r.name LIKE ? OR w.prompt_text LIKE ?)".into(),
        );
    }
    if !conds.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&conds.join(" AND "));
    }
    // 批次倒序 + 批次内生成序：与验收页（v0.11.0）同一排序，两页读起来对得上。
    // 批次是一阵工作的天然单元，也是「近期这批」唯一稳定的锚点；accepted_at 会因为
    // 隔天补验收而把同一批切散到两个日期里。NULL batch 在 DESC 下自然排到最后。
    sql.push_str(" ORDER BY w.batch_id DESC, w.id ASC LIMIT ? OFFSET ?");

    let limit = 300i64;
    let offset = page.unwrap_or(0).max(0) * limit;
    let mut q = sqlx::query_as::<_, WorkView>(&sql);
    // 绑定序必须与上面 push 条件的先后严格一致。
    if let Some(gid) = filter.group_id {
        q = q.bind(gid);
    }
    if let Some(tag) = &filter.tag {
        q = q.bind(tag.clone());
    }
    if let Some(b) = filter.batch_id {
        q = q.bind(b);
    }
    if let Some(pat) = &query {
        q = q
            .bind(pat.clone())
            .bind(pat.clone())
            .bind(pat.clone())
            .bind(pat.clone());
    }
    Ok(q.bind(limit).bind(offset).fetch_all(&state.db).await?)
}

#[tauri::command]
#[specta::specta]
pub async fn toggle_work_favorite(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    work_repo::toggle_favorite(&state.db, id).await?;
    Ok(())
}

/// 删除作品 → 进废纸篓（记录删除，文件待清理时物理删）。
#[tauri::command]
#[specta::specta]
pub async fn trash_work(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    trash_one_work(&state.db, id, "手动删除").await
}

/// 一条作品进废纸篓。整行序列化进 `payload_json`（0027）——作品是唯一「删除即真删行」
/// 的实体，不留快照就还原不回来，而误删一张已经验收通过的图恰恰是最该能撤回的一种。
async fn trash_one_work(pool: &sqlx::SqlitePool, id: i64, label: &str) -> AppResult<()> {
    let Some(row) = work_repo::delete(pool, id).await? else {
        return Ok(());
    };
    let payload = work_repo::to_payload(&row);
    let mut tx = pool.begin().await?;
    trash_repo::insert(
        &mut tx,
        &trash_repo::NewTrashItem {
            entity_type: "work".into(),
            ref_id: Some(row.id),
            thumb_path: Some(row.thumb_path.clone()),
            prompt_text: Some(row.prompt_text.clone()),
            code: (!row.prompt_code.is_empty()).then(|| row.prompt_code.clone()),
            title: (!row.group_name.is_empty()).then(|| row.group_name.clone()),
            source_label: label.into(),
            // E21 决策：默认**不**物理删除外部输出文件（用户可能已发布/引用）；仅清缩略图。
            file_paths: vec![row.thumb_path],
            payload_json: payload,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

/// 批量收藏（E15）。favorite=true 收藏，false 取消。
#[tauri::command]
#[specta::specta]
pub async fn set_works_favorite(
    state: State<'_, AppState>,
    ids: Vec<i64>,
    favorite: bool,
) -> AppResult<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let ph = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!("UPDATE accepted_works SET favorite = ? WHERE id IN ({ph})");
    let mut q = sqlx::query(&sql).bind(favorite as i64);
    for id in &ids {
        q = q.bind(id);
    }
    q.execute(&state.db).await?;
    Ok(())
}

/// 批量删除作品 → 进废纸篓（E15）。默认不物理删除外部输出文件（同 trash_work 决策）。
#[tauri::command]
#[specta::specta]
pub async fn trash_works(state: State<'_, AppState>, ids: Vec<i64>) -> AppResult<()> {
    for id in ids {
        trash_one_work(&state.db, id, "批量删除").await?;
    }
    Ok(())
}

/// 批量导出作品到指定文件夹（E15）：复制各作品输出文件（image_path）到目标目录。
/// 返回成功导出数；源文件缺失的项跳过（不计入）。
#[tauri::command]
#[specta::specta]
pub async fn export_works(
    state: State<'_, AppState>,
    ids: Vec<i64>,
    dest_dir: String,
) -> AppResult<i64> {
    if ids.is_empty() {
        return Ok(0);
    }
    let dest = std::path::PathBuf::from(&dest_dir);

    let ph = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!("SELECT image_path FROM accepted_works WHERE id IN ({ph})");
    let mut q = sqlx::query_scalar::<_, String>(&sql);
    for id in &ids {
        q = q.bind(id);
    }
    let paths = q.fetch_all(&state.db).await?;

    // 拷贝几十上百张图是纯阻塞 IO —— 留在异步执行器上会把整个 IPC 卡住，
    // 而这条命令恰恰是「人选了一整页作品点导出」时跑的（同 v0.14.0 那次的教训）。
    tokio::task::spawn_blocking(move || -> AppResult<i64> {
        std::fs::create_dir_all(&dest).map_err(|e| AppError::Io(e.to_string()))?;
        let mut exported = 0i64;
        for p in paths {
            let src = std::path::PathBuf::from(&p);
            if !src.is_file() {
                continue;
            }
            let Some(name) = src.file_name() else {
                continue;
            };
            // 目标同名冲突时追加序号，避免覆盖。
            let mut out = dest.join(name);
            let mut n = 1;
            while out.exists() {
                let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("work");
                let ext = src.extension().and_then(|s| s.to_str()).unwrap_or("jpg");
                out = dest.join(format!("{stem}_{n}.{ext}"));
                n += 1;
            }
            if std::fs::copy(&src, &out).is_ok() {
                exported += 1;
            }
        }
        Ok(exported)
    })
    .await
    .map_err(|e| AppError::Io(format!("导出任务失败：{e}")))?
}

/// 文件是否存在（E21 作品源文件缺失懒检测）。
#[tauri::command]
#[specta::specta]
pub async fn file_exists(path: String) -> AppResult<bool> {
    Ok(std::path::Path::new(&path).is_file())
}

/// 从资产区快照重新导出作品输出文件（E21）：源为 `results/{task_id}.{ext}`。
/// 批次已删除（task_id 为空）或快照已随清理消失时报可读错误。
#[tauri::command]
#[specta::specta]
pub async fn reexport_work(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    reexport_from_snapshot(&state.db, state.dirs.results(), id).await
}

/// [`reexport_work`] 的本体。抽出来只为一件事：R2 —— **重新导出必须复用原 uid**
/// —— 是可测的，而 `State<AppResult>` 在单测里造不出来。
///
/// 复用不是「再发一次相同的号」，而是**根本不发号**：文件名早就写在
/// `accepted_works.image_path` 里了，这里原样拷回去。uid 是作品的属性，
/// 不是导出这个动作的属性；重发会让已经交付出去的那个文件从此认不出主人。
pub(crate) async fn reexport_from_snapshot(
    pool: &sqlx::SqlitePool,
    results_dir: std::path::PathBuf,
    id: i64,
) -> AppResult<()> {
    let row: Option<(Option<i64>, String)> =
        sqlx::query_as("SELECT task_id, image_path FROM accepted_works WHERE id = ?1")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    let Some((task_id, image_path)) = row else {
        return Err(AppError::InvalidInput("作品不存在".into()));
    };
    let Some(task_id) = task_id else {
        return Err(AppError::InvalidInput(
            "该作品所属批次已清理，源快照不存在，无法重新导出".into(),
        ));
    };
    // 任务1：结果快照扩展名跟随输出文件（默认 jpg；保留原格式时可能 png）。
    let ext = std::path::Path::new(&image_path)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("jpg")
        .to_lowercase();
    let stored: Option<String> =
        sqlx::query_scalar("SELECT result_image_path FROM tasks WHERE id=?")
            .bind(task_id)
            .fetch_optional(pool)
            .await?
            .flatten();
    let src = stored
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_file())
        .unwrap_or_else(|| results_dir.join(format!("{task_id}.{ext}")));
    if !src.is_file() {
        return Err(AppError::InvalidInput(
            "资产区源快照已不存在（可能随批次清理删除），无法重新导出".into(),
        ));
    }
    let dst = std::path::PathBuf::from(&image_path);
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).map_err(|e| AppError::Io(e.to_string()))?;
    }
    std::fs::copy(&src, &dst).map_err(|e| AppError::Io(e.to_string()))?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即失败
mod tests {
    use super::{reexport_from_snapshot, WorkView, WORK_SELECT};
    use crate::db::test_support::test_pool;
    use sqlx::SqlitePool;

    /// R2：重新导出**复用原 uid**，绝不重摇。
    ///
    /// uid 是作品的属性，不是导出这个动作的属性。重发一个号意味着同一张图在
    /// `自动发布` 那边有了两个主键，而先送出去的那份从此认不出主人 —— 而这条路径
    /// 恰恰是「文件被搬走之后救回来」的那条（D3），最不该在这里换名字。
    #[tokio::test]
    async fn reexport_reuses_the_original_uid_and_issues_nothing() {
        let (pool, _d) = test_pool().await;
        let data = tempfile::tempdir().unwrap();
        let results = data.path().join("results");
        std::fs::create_dir_all(&results).unwrap();
        std::fs::write(results.join("1.jpg"), b"snapshot").unwrap();
        let out = data
            .path()
            .join("outputs")
            .join("W-王一博-55-正面-hu72.jpg");

        sqlx::query("INSERT INTO batches (id,created_at,output_dir,params_json,status) VALUES (1,0,'/out','{}','running')").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO prompt_groups (id,name,prefix,scene,is_temp,created_at) VALUES (1,'g','GG','',0,0)").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO prompts (id,group_id,code,text,status,source,created_at,updated_at) VALUES (1,1,'GG-0001','t','active','library',0,0)").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO ref_images (id,name,file_path,thumb_path,width,height,file_size,created_at) VALUES (1,'r','/a','/t',1,1,1,0)").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO tasks (id,batch_id,ref_image_id,prompt_id,prompt_text_snapshot,status,created_at,updated_at) VALUES (1,1,1,1,'t','pass',0,0)").execute(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO accepted_works (id,task_id,image_path,thumb_path,prompt_text,accepted_at,
                item_name,uid,sku_code,style)
             VALUES (1,1,?1,'/t','t',0,'W-王一博-55-正面','hu72','55','王一博')",
        )
        .bind(out.to_string_lossy().to_string())
        .execute(&pool)
        .await
        .unwrap();
        let mut tx = pool.begin().await.unwrap();
        crate::db::repo::names::issue_with(&mut tx, "W-王一博-55-正面", Some(1), || {
            "hu72".into()
        })
        .await
        .unwrap();
        tx.commit().await.unwrap();

        reexport_from_snapshot(&pool, results, 1).await.unwrap();

        assert!(out.is_file(), "应当把快照拷回原来那个文件名");
        let (uid, n): (String, i64) = sqlx::query_as(
            "SELECT (SELECT uid FROM accepted_works WHERE id=1),
                    (SELECT COUNT(*) FROM issued_names)",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(uid, "hu72", "uid 不得因为重新导出而改变");
        assert_eq!(n, 1, "重新导出不得新发一个号");
    }

    /// 批次倒序 + 批次内 id 升序：与验收页同一排序，两页读起来对得上。
    /// 这条排序是「按批次分节」这个 UI 决定的地基——换成 accepted_at 排序，
    /// 隔天补验收的同一批就会被切散到两个日期里，分节当场失效。
    #[tokio::test]
    async fn lists_newest_batch_first_then_generation_order() {
        let (pool, _d) = test_pool().await;
        seed(&pool, 1, "AA", &[(1, "甲"), (2, "乙")]).await;
        // 第二个批次（id 更大 = 更近），且 accepted_at **更早**——模拟隔天补验收：
        // 若按 accepted_at 排序，这两条会跑到旧批次后面去。
        sqlx::query("INSERT INTO batches (id,created_at,output_dir,params_json,status) VALUES (2,0,'/out','{}','running')")
            .execute(&pool).await.unwrap();
        for wid in [3i64, 4] {
            sqlx::query("INSERT INTO prompts (id,group_id,code,text,status,source,created_at,updated_at) VALUES (?1,1,?2,'x','active','library',0,0)")
                .bind(wid).bind(format!("AA-{wid:04}")).execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO accepted_works (id,image_path,thumb_path,prompt_id,prompt_text,group_id,batch_id,accepted_at,prompt_code,group_name) VALUES (?1,'/i','/t',?1,'x',1,2,-999,?2,'组')")
                .bind(wid).bind(format!("AA-{wid:04}")).execute(&pool).await.unwrap();
        }
        let sql = format!("{WORK_SELECT} ORDER BY w.batch_id DESC, w.id ASC");
        let rows = sqlx::query_as::<_, WorkView>(&sql)
            .fetch_all(&pool)
            .await
            .unwrap();
        let order: Vec<(Option<i64>, i64)> = rows.iter().map(|r| (r.batch_id, r.id)).collect();
        assert_eq!(
            order,
            vec![(Some(2), 3), (Some(2), 4), (Some(1), 1), (Some(1), 2)],
            "近批整体在前，批内按生成序；不受 accepted_at 干扰"
        );
    }

    // 全文搜索一次覆盖编号/组名/参考图名/正文：人搜的时候并不知道自己记住的是哪一处。
    #[tokio::test]
    async fn search_matches_code_group_ref_and_body() {
        let (pool, _d) = test_pool().await;
        seed(&pool, 1, "AA", &[(1, "屋顶花园的木地台"), (2, "宠物餐吧")]).await;
        let sql = format!(
            "{WORK_SELECT} WHERE (w.prompt_code LIKE ?1 OR w.group_name LIKE ?1 OR r.name LIKE ?1 OR w.prompt_text LIKE ?1) ORDER BY w.id"
        );
        let hit = |pat: &str| {
            let sql = sql.clone();
            let pool = pool.clone();
            let pat = format!("%{pat}%");
            async move {
                sqlx::query_as::<_, WorkView>(&sql)
                    .bind(pat)
                    .fetch_all(&pool)
                    .await
                    .unwrap()
                    .into_iter()
                    .map(|r| r.id)
                    .collect::<Vec<_>>()
            }
        };
        assert_eq!(hit("屋顶花园").await, vec![1], "命中正文");
        assert_eq!(hit("AA-0002").await, vec![2], "命中编号");
        assert_eq!(hit("组1").await, vec![1, 2], "命中分组名");
        assert!(hit("查无此物").await.is_empty());
    }

    // 卡片角标：已在流水线 / 用途是图生视频。前者防重复入队，后者是「本批全选」的取样依据。
    #[tokio::test]
    async fn view_flags_reflect_pipeline_and_purpose() {
        let (pool, _d) = test_pool().await;
        seed(&pool, 1, "AA", &[(1, "甲"), (2, "乙")]).await;
        set_purpose(&pool, &[1, 2], "图生视频").await;
        sqlx::query(
            "INSERT INTO v2v_clips (work_id,group_id,group_name,stage,source_prompt,created_at,updated_at)
             VALUES (1,1,'组1','rewrite','甲',0,0)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let sql = format!("{WORK_SELECT} ORDER BY w.id");
        let rows = sqlx::query_as::<_, WorkView>(&sql)
            .fetch_all(&pool)
            .await
            .unwrap();
        assert!(rows[0].in_pipeline, "已入队的图须带角标");
        assert!(!rows[1].in_pipeline, "未入队的图不得带角标");
        assert!(rows[0].is_i2v && rows[1].is_i2v, "同组两张的用途一致");
    }

    /// 种一个组 + 一条提示词 + 一张参考图 + 一个批次 + N 条已验收作品。
    async fn seed(pool: &SqlitePool, group_id: i64, prefix: &str, works: &[(i64, &str)]) {
        sqlx::query("INSERT INTO prompt_groups (id,name,prefix,scene,is_temp,created_at) VALUES (?1,?2,?3,'',0,0)")
            .bind(group_id).bind(format!("组{group_id}")).bind(prefix)
            .execute(pool).await.unwrap();
        sqlx::query("INSERT OR IGNORE INTO ref_images (id,name,file_path,thumb_path,width,height,file_size,created_at) VALUES (1,'r','/f','/t',1,1,1,0)")
            .execute(pool).await.unwrap();
        sqlx::query("INSERT OR IGNORE INTO batches (id,created_at,output_dir,params_json,status) VALUES (1,0,'/out','{}','running')")
            .execute(pool).await.unwrap();
        for (wid, text) in works {
            sqlx::query("INSERT INTO prompts (id,group_id,code,text,status,source,created_at,updated_at) VALUES (?1,?2,?3,?4,'active','library',0,0)")
                .bind(wid).bind(group_id).bind(format!("{prefix}-{wid:04}")).bind(*text)
                .execute(pool).await.unwrap();
            // task_id 留空：0008 起可空，且真实数据里就有 5 条这样的行（批次已清理）。
            sqlx::query("INSERT INTO accepted_works (id,task_id,image_path,thumb_path,prompt_id,prompt_text,group_id,ref_image_id,batch_id,accepted_at,prompt_code,group_name) VALUES (?1,NULL,?2,'/t',?1,?3,?4,1,1,0,?5,?6)")
                .bind(wid).bind(format!("/img{wid}.jpg")).bind(*text).bind(group_id)
                .bind(format!("{prefix}-{wid:04}")).bind(format!("组{group_id}"))
                .execute(pool).await.unwrap();
        }
    }

    async fn set_purpose(pool: &SqlitePool, work_ids: &[i64], purpose: &str) {
        for id in work_ids {
            sqlx::query("UPDATE accepted_works SET purpose = ?2 WHERE id = ?1")
                .bind(id)
                .bind(purpose)
                .execute(pool)
                .await
                .unwrap();
        }
    }

    /// 复刻 list_works 的用途条件（0045 起读作品自己的列）。
    const TAG_COND: &str = "SELECT w.id FROM accepted_works w WHERE w.purpose = ?1 ORDER BY w.id";

    /// 用途筛选读**作品自己的那一列**，不再沿「作品 → 组 → tag_bindings」那条边。
    ///
    /// 这条替换了原来的 `tag_filter_selects_only_works_of_tagged_groups`：它的前提
    /// （用途绑在提示词组上）已被 0045 整体移除，不是为了让 CI 变绿而放宽。
    /// 新形态**更严**：多加了一条「同组但用途不同的两张图必须分得开」——
    /// 那正是旧载体表达不了的东西（标签绑在组上，同组必然同用途）。
    #[tokio::test]
    async fn purpose_filter_reads_the_work_s_own_column() {
        let (pool, _d) = test_pool().await;
        seed(&pool, 1, "AA", &[(1, "甲"), (2, "乙")]).await;
        seed(&pool, 2, "BB", &[(3, "丙")]).await;
        set_purpose(&pool, &[1, 2], "图生视频").await;

        let ids: Vec<i64> = sqlx::query_scalar(TAG_COND)
            .bind("图生视频")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(ids, vec![1, 2], "只应命中用途为图生视频的作品");

        // 同一个组里两张图用途不同 —— 旧载体（标签绑在组上）表达不了这件事。
        set_purpose(&pool, &[2], "").await;
        let ids: Vec<i64> = sqlx::query_scalar(TAG_COND)
            .bind("图生视频")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(ids, vec![1], "同组不同用途必须分得开");
        set_purpose(&pool, &[2], "图生视频").await;

        let none: Vec<i64> = sqlx::query_scalar(TAG_COND)
            .bind("不存在的用途")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert!(none.is_empty(), "未绑定的标签不应命中任何作品");
    }
}
