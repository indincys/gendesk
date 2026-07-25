//! works 域命令（作品库，执行计划 2.1 / 需求 14.4）。

use serde::{Deserialize, Serialize};
use specta::Type;
use sqlx::FromRow;
use tauri::State;

use crate::db::now_unix;
use crate::db::repo::{trash as trash_repo, works as work_repo};
use crate::error::{AppError, AppResult};
use crate::files;
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
    /// 按分组标签（含受控「用途」）筛选。作品自身不带标签——标签绑在它的提示词组上。
    pub tag: Option<String>,
    /// 隐藏已导出到图生视频包的作品（跨包去重，读 work_exports 台账）。
    pub hide_exported: bool,
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

const WORK_SELECT: &str = "SELECT w.id, COALESCE(p.code,'') AS prompt_code,
        COALESCE(g.name,'') AS group_name, COALESCE(r.name,'') AS ref_name,
        w.batch_id, w.favorite, w.accepted_at, w.image_path, w.thumb_path, w.prompt_text,
        w.ref_image_id, w.group_id, w.task_id,
        EXISTS (SELECT 1 FROM v2v_clips c WHERE c.work_id = w.id) AS in_pipeline,
        EXISTS (SELECT 1 FROM tag_bindings tb JOIN tags tg ON tg.id = tb.tag_id
                WHERE tb.entity_type = 'prompt_group' AND tb.entity_id = w.group_id
                  AND tg.name = '图生视频') AS is_i2v
    FROM accepted_works w
    LEFT JOIN prompts p ON p.id = w.prompt_id
    LEFT JOIN prompt_groups g ON g.id = w.group_id
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
        conds.push(
            "EXISTS (SELECT 1 FROM tag_bindings tb JOIN tags tg ON tg.id = tb.tag_id
                     WHERE tb.entity_type = 'prompt_group' AND tb.entity_id = w.group_id
                       AND tg.name = ?)"
                .into(),
        );
    }
    if filter.hide_exported {
        conds.push(
            "NOT EXISTS (SELECT 1 FROM work_exports we
                         WHERE we.work_id = w.id AND we.channel = ?)"
                .into(),
        );
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
            "(p.code LIKE ? OR g.name LIKE ? OR r.name LIKE ? OR w.prompt_text LIKE ?)".into(),
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
    if filter.hide_exported {
        q = q.bind(crate::v2v::CHANNEL_I2V);
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
pub async fn get_work(state: State<'_, AppState>, id: i64) -> AppResult<WorkView> {
    let sql = format!("{WORK_SELECT} WHERE w.id = ?");
    sqlx::query_as::<_, WorkView>(&sql)
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::InvalidInput("作品不存在".into()))
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
    let Some(row) = work_repo::delete(&state.db, id).await? else {
        return Ok(());
    };
    let mut tx = state.db.begin().await?;
    trash_repo::insert(
        &mut tx,
        &trash_repo::NewTrashItem {
            entity_type: "work".into(),
            ref_id: Some(row.id),
            thumb_path: Some(row.thumb_path.clone()),
            prompt_text: Some(row.prompt_text.clone()),
            code: None,
            title: None,
            source_label: "手动删除".into(),
            // E21 决策：默认**不**物理删除外部输出文件（用户可能已发布/引用）；仅清缩略图。
            file_paths: vec![row.thumb_path],
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
        let Some(row) = work_repo::delete(&state.db, id).await? else {
            continue;
        };
        let mut tx = state.db.begin().await?;
        trash_repo::insert(
            &mut tx,
            &trash_repo::NewTrashItem {
                entity_type: "work".into(),
                ref_id: Some(row.id),
                thumb_path: Some(row.thumb_path.clone()),
                prompt_text: Some(row.prompt_text.clone()),
                code: None,
                title: None,
                source_label: "批量删除".into(),
                file_paths: vec![row.thumb_path],
            },
        )
        .await?;
        tx.commit().await?;
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
    std::fs::create_dir_all(&dest).map_err(|e| AppError::Io(e.to_string()))?;

    let ph = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!("SELECT image_path FROM accepted_works WHERE id IN ({ph})");
    let mut q = sqlx::query_scalar::<_, String>(&sql);
    for id in &ids {
        q = q.bind(id);
    }
    let paths = q.fetch_all(&state.db).await?;

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
}

/// 导出用的作品行（含组前缀，供包目录命名）。
#[derive(Debug, Clone, FromRow)]
struct V2vRow {
    id: i64,
    group_id: Option<i64>,
    group_name: String,
    group_prefix: String,
    prompt_code: String,
    ref_name: String,
    batch_id: Option<i64>,
    accepted_at: i64,
    image_path: String,
    thumb_path: String,
    prompt_text: String,
}

const V2V_SELECT: &str = "SELECT w.id, w.group_id,
        COALESCE(g.name,'未分组') AS group_name, COALESCE(g.prefix,'x') AS group_prefix,
        COALESCE(p.code,'') AS prompt_code, COALESCE(r.name,'') AS ref_name,
        w.batch_id, w.accepted_at, w.image_path, w.thumb_path, w.prompt_text
    FROM accepted_works w
    LEFT JOIN prompts p ON p.id = w.prompt_id
    LEFT JOIN prompt_groups g ON g.id = w.group_id
    LEFT JOIN ref_images r ON r.id = w.ref_image_id";

/// 导出图生视频包（一包一组）。
///
/// 一包一组不是为了目录整齐：同组的分镜图最后要剪进同一条成片，运镜语言与时长必须统一，
/// 跨组混一个包改写出来的风格会飘。所选作品按 group_id 分堆，每堆一个包。
#[tauri::command]
#[specta::specta]
pub async fn export_works_v2v(
    state: State<'_, AppState>,
    ids: Vec<i64>,
    dest_dir: String,
) -> AppResult<Vec<crate::v2v::PackSummary>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let dest = std::path::PathBuf::from(&dest_dir);
    std::fs::create_dir_all(&dest).map_err(|e| AppError::Io(e.to_string()))?;

    let ph = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!("{V2V_SELECT} WHERE w.id IN ({ph}) ORDER BY w.group_id, w.id");
    let mut q = sqlx::query_as::<_, V2vRow>(&sql);
    for id in &ids {
        q = q.bind(id);
    }
    let rows = q.fetch_all(&state.db).await?;
    if rows.is_empty() {
        return Err(AppError::InvalidInput("所选作品不存在".into()));
    }

    // 按组分堆（SQL 已按 group_id 排序，相邻即同组）。
    let mut buckets: Vec<Vec<V2vRow>> = Vec::new();
    for row in rows {
        match buckets.last_mut() {
            Some(b) if b[0].group_id == row.group_id => b.push(row),
            _ => buckets.push(vec![row]),
        }
    }

    let date = files::date_yymmdd(now_unix());
    let mut summaries = Vec::with_capacity(buckets.len());
    for bucket in buckets {
        summaries.push(export_one_pack(&state, &dest, &date, bucket).await?);
    }
    Ok(summaries)
}

async fn export_one_pack(
    state: &State<'_, AppState>,
    dest: &std::path::Path,
    date: &str,
    bucket: Vec<V2vRow>,
) -> AppResult<crate::v2v::PackSummary> {
    let head = &bucket[0];
    let group_name = head.group_name.clone();

    // 公共前后缀取自**该组全部**验收作品，而不只是本次所选的几条：超集的公共缀必然是
    // 子集公共缀的前缀，取超集更保守——只选 2 条时不会把恰好雷同的一大段场景描写当模板剥走。
    let group_texts: Vec<String> = match head.group_id {
        Some(gid) => {
            sqlx::query_scalar::<_, String>(
                "SELECT prompt_text FROM accepted_works WHERE group_id = ?1 ORDER BY id",
            )
            .bind(gid)
            .fetch_all(&state.db)
            .await?
        }
        None => bucket.iter().map(|r| r.prompt_text.clone()).collect(),
    };
    let (pre, suf) = crate::v2v::common_affixes(&group_texts);

    let pack_id = crate::v2v::dedupe_dir(
        dest,
        &crate::v2v::pack_dir_name(date, &head.group_prefix, &group_name),
    );
    let pack_dir = dest.join(&pack_id);

    let mut items = Vec::with_capacity(bucket.len());
    let mut skipped = 0i64;
    for row in &bucket {
        let src = std::path::Path::new(&row.image_path);
        if !src.is_file() {
            // E21 懒检测同源：源文件可能已被外部移走/删除。跳过并计数，绝不写一条
            // 指向不存在文件的 manifest——skill 拿到那种条目只会在提交时报错。
            skipped += 1;
            continue;
        }
        let item_id = format!("W{}", row.id);
        let ext = files::output_ext_from_path(&row.image_path);
        let image_rel = format!("images/{item_id}.{ext}");
        std::fs::create_dir_all(pack_dir.join("images"))
            .map_err(|e| AppError::Io(e.to_string()))?;
        std::fs::copy(src, pack_dir.join(&image_rel)).map_err(|e| AppError::Io(e.to_string()))?;

        // 缩略图缺失不致命（只影响读图成本），原图仍在，照常出条目。
        let thumb_src = std::path::Path::new(&row.thumb_path);
        let thumb_rel = if thumb_src.is_file() {
            let t = format!("thumbs/{item_id}.jpg");
            std::fs::create_dir_all(pack_dir.join("thumbs"))
                .map_err(|e| AppError::Io(e.to_string()))?;
            std::fs::copy(thumb_src, pack_dir.join(&t)).map_err(|e| AppError::Io(e.to_string()))?;
            t
        } else {
            image_rel.clone()
        };

        items.push(crate::v2v::PackItem {
            id: item_id,
            work_id: row.id,
            prompt_code: row.prompt_code.clone(),
            group_name: group_name.clone(),
            ref_name: row.ref_name.clone(),
            batch_id: row.batch_id,
            accepted_at: row.accepted_at,
            image: image_rel,
            thumb: thumb_rel,
            display_name: src
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default(),
            source_prompt: row.prompt_text.clone(),
            variable_part: crate::v2v::variable_part(&row.prompt_text, pre, suf),
            stripped_prefix_chars: pre,
            stripped_suffix_chars: suf,
        });
    }

    crate::v2v::write_pack(&pack_dir, &items, &group_name)?;

    // 台账：包写成之后才记，否则写包失败会留下「记了没导出」的假记录，
    // 而「隐藏已导出」正是靠它筛——假记录会让那张图从候选里永久消失。
    let now = now_unix();
    let mut tx = state.db.begin().await?;
    for item in &items {
        sqlx::query(
            "INSERT OR IGNORE INTO work_exports (work_id, channel, pack_id, item_id, exported_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .bind(item.work_id)
        .bind(crate::v2v::CHANNEL_I2V)
        .bind(&pack_id)
        .bind(&item.id)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;

    Ok(crate::v2v::PackSummary {
        pack_dir: pack_dir.to_string_lossy().to_string(),
        pack_id,
        exported: items.len() as i64,
        skipped,
        stripped_prefix_chars: pre,
        stripped_suffix_chars: suf,
    })
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
    let row: Option<(Option<i64>, String)> =
        sqlx::query_as("SELECT task_id, image_path FROM accepted_works WHERE id = ?1")
            .bind(id)
            .fetch_optional(&state.db)
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
    let src = state.dirs.results().join(format!("{task_id}.{ext}"));
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
    use super::{WorkView, WORK_SELECT};
    use crate::db::test_support::test_pool;
    use sqlx::SqlitePool;

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
            sqlx::query("INSERT INTO accepted_works (id,image_path,thumb_path,prompt_id,prompt_text,group_id,batch_id,accepted_at) VALUES (?1,'/i','/t',?1,'x',1,2,-999)")
                .bind(wid).execute(&pool).await.unwrap();
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
            "{WORK_SELECT} WHERE (p.code LIKE ?1 OR g.name LIKE ?1 OR r.name LIKE ?1 OR w.prompt_text LIKE ?1) ORDER BY w.id"
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
        bind_tag(&pool, 1, "图生视频").await;
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
            sqlx::query("INSERT INTO accepted_works (id,task_id,image_path,thumb_path,prompt_id,prompt_text,group_id,ref_image_id,batch_id,accepted_at) VALUES (?1,NULL,?2,'/t',?1,?3,?4,1,1,0)")
                .bind(wid).bind(format!("/img{wid}.jpg")).bind(*text).bind(group_id)
                .execute(pool).await.unwrap();
        }
    }

    async fn bind_tag(pool: &SqlitePool, group_id: i64, tag: &str) {
        sqlx::query("INSERT INTO tags (name) VALUES (?1) ON CONFLICT(name) DO NOTHING")
            .bind(tag)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO tag_bindings (tag_id, entity_type, entity_id) SELECT id,'prompt_group',?2 FROM tags WHERE name=?1")
            .bind(tag).bind(group_id).execute(pool).await.unwrap();
    }

    /// 复刻 list_works 的 tag 条件（作品自身无标签，须经其提示词组的绑定过滤）。
    const TAG_COND: &str = "SELECT w.id FROM accepted_works w WHERE EXISTS (
        SELECT 1 FROM tag_bindings tb JOIN tags tg ON tg.id = tb.tag_id
        WHERE tb.entity_type = 'prompt_group' AND tb.entity_id = w.group_id AND tg.name = ?1)
        ORDER BY w.id";

    // 用途筛选是整条链路的入口：批次会混组，只有沿「作品 → 组 → 标签」这条边才筛得对。
    #[tokio::test]
    async fn tag_filter_selects_only_works_of_tagged_groups() {
        let (pool, _d) = test_pool().await;
        seed(&pool, 1, "AA", &[(1, "甲"), (2, "乙")]).await;
        seed(&pool, 2, "BB", &[(3, "丙")]).await;
        bind_tag(&pool, 1, "图生视频").await;

        let ids: Vec<i64> = sqlx::query_scalar(TAG_COND)
            .bind("图生视频")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(ids, vec![1, 2], "只应命中已打用途标签的组下的作品");

        let none: Vec<i64> = sqlx::query_scalar(TAG_COND)
            .bind("不存在的用途")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert!(none.is_empty(), "未绑定的标签不应命中任何作品");
    }

    /// 复刻 list_works 的 hide_exported 条件。
    const HIDE_COND: &str = "SELECT w.id FROM accepted_works w WHERE NOT EXISTS (
        SELECT 1 FROM work_exports we WHERE we.work_id = w.id AND we.channel = ?1)
        ORDER BY w.id";

    // 跨包去重：台账里已有该渠道记录的作品不再出现在候选里，
    // 但**别的渠道**的记录不得误伤——否则将来第二个下游一上线，第一个下游就全被挡住。
    #[tokio::test]
    async fn hide_exported_is_scoped_to_channel() {
        let (pool, _d) = test_pool().await;
        seed(&pool, 1, "AA", &[(1, "甲"), (2, "乙"), (3, "丙")]).await;
        sqlx::query("INSERT INTO work_exports (work_id,channel,pack_id,item_id,exported_at) VALUES (1,'i2v','p1','W1',0)")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO work_exports (work_id,channel,pack_id,item_id,exported_at) VALUES (2,'other','p2','W2',0)")
            .execute(&pool).await.unwrap();

        let ids: Vec<i64> = sqlx::query_scalar(HIDE_COND)
            .bind(crate::v2v::CHANNEL_I2V)
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(
            ids,
            vec![2, 3],
            "只有本渠道导出过的 1 应被隐藏；2 是别的渠道，不得误伤"
        );
    }

    // 同一包内同一条目重复写台账须幂等（INSERT OR IGNORE + UNIQUE(pack_id,item_id)），
    // 否则重导一次就把台账写成重影，「已导出几次」的口径当场失真。
    #[tokio::test]
    async fn ledger_insert_is_idempotent_per_pack_item() {
        let (pool, _d) = test_pool().await;
        seed(&pool, 1, "AA", &[(1, "甲")]).await;
        for _ in 0..3 {
            sqlx::query("INSERT OR IGNORE INTO work_exports (work_id,channel,pack_id,item_id,exported_at) VALUES (1,'i2v','p1','W1',0)")
                .execute(&pool).await.unwrap();
        }
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM work_exports")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 1, "同包同条目只应留一条台账");

        // 换个包重导同一张图：这是新的一次导出，必须记得下。
        sqlx::query("INSERT OR IGNORE INTO work_exports (work_id,channel,pack_id,item_id,exported_at) VALUES (1,'i2v','p2','W1',0)")
            .execute(&pool).await.unwrap();
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM work_exports")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 2, "不同包的导出各记一条");
    }

    // work_exports 不设 FK：作品被删（进废纸篓）后台账仍在，
    // 才答得出「这张图当时导出过」。若被级联抹掉，历史就无从追溯。
    #[tokio::test]
    async fn ledger_survives_work_deletion() {
        let (pool, _d) = test_pool().await;
        seed(&pool, 1, "AA", &[(1, "甲")]).await;
        sqlx::query("INSERT INTO work_exports (work_id,channel,pack_id,item_id,exported_at) VALUES (1,'i2v','p1','W1',0)")
            .execute(&pool).await.unwrap();
        sqlx::query("DELETE FROM accepted_works WHERE id=1")
            .execute(&pool)
            .await
            .unwrap();
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM work_exports")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 1, "作品删除不得连带抹掉导出台账");
    }
}
