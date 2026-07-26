//! 图生视频流水线薄 SQL 封装（业务规则不在此层）。
//!
//! 一条 `v2v_clips` = 一张验收通过的图的一次视频尝试。七态与图片侧 `tasks.status` 同构：
//! rewrite（待改写）→ ready（待提交）→ run（已提交）→ rev（待验收）→ pass / rej / fail。

use sqlx::{FromRow, Sqlite, SqlitePool, Transaction};

/// 看板/详情视图（含来自 accepted_works 的父图信息）。
#[derive(Debug, Clone, FromRow)]
pub struct ClipRow {
    pub id: i64,
    pub work_id: i64,
    pub group_id: Option<i64>,
    pub group_name: String,
    pub batch_id: Option<i64>,
    pub stage: String,
    pub source_prompt: String,
    pub variable_part: String,
    pub video_prompt: Option<String>,
    pub model_version: Option<String>,
    pub duration: Option<i64>,
    pub video_resolution: Option<String>,
    pub submit_id: Option<String>,
    pub credit_count: Option<i64>,
    pub video_path: Option<String>,
    pub poster_path: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub fps: Option<f64>,
    pub duration_sec: Option<f64>,
    pub attempt: i64,
    pub error_type: Option<String>,
    pub error_message: Option<String>,
    /// 提交时刻。轮询器的超时兜底读它（「未知态判 Running」必须有个尽头）。
    pub submitted_at: Option<i64>,
    /// 即梦返回的 `gen_status` 原文（0021）。不翻译成自造中文态：它加新态时翻译层只会
    /// 显示「未知」，而原文至少还能被搜索、被拿去问客服。
    pub gen_status: Option<String>,
    /// 排队位次。即梦当前**不回传**（见 `dreamina::QueryResult::queue_idx`），恒为 None；
    /// 留着是因为字段在它的数据形状里，哪天开始给就自动生效。
    pub queue_idx: Option<i64>,
    /// 最后一次**发起查询**的时刻（成功与否都记）。退避轮询据此决定这一条到点没有。
    pub polled_at: Option<i64>,
    /// 实际计费型号（0022，来自回执）。回答「到底走的哪个模型」——
    /// 用我们自己发出去的 `model_version` 回答等于自问自答。
    pub benefit_type: Option<String>,
    pub updated_at: i64,
    /// 父图编号（`accepted_works` → `prompts.code`）。
    pub prompt_code: String,
    /// 父图（首帧）绝对路径 —— 提交即梦 `--image` 用的就是它。
    pub image_path: String,
    /// 父图缩略图 —— 喂 skill 看图、看板渲染。
    pub thumb_path: String,
    pub accepted_at: i64,
}

const SELECT: &str = "SELECT c.id, c.work_id, c.group_id, c.group_name, c.batch_id, c.stage,
        c.source_prompt, c.variable_part, c.video_prompt, c.model_version, c.duration,
        c.video_resolution, c.submit_id, c.credit_count, c.video_path, c.poster_path,
        c.width, c.height, c.fps, c.duration_sec, c.attempt, c.error_type, c.error_message,
        c.submitted_at, c.gen_status, c.queue_idx, c.polled_at, c.benefit_type, c.updated_at,
        COALESCE(p.code,'') AS prompt_code,
        COALESCE(w.image_path,'') AS image_path,
        COALESCE(w.thumb_path,'') AS thumb_path,
        COALESCE(w.accepted_at,0) AS accepted_at
    FROM v2v_clips c
    LEFT JOIN accepted_works w ON w.id = c.work_id
    LEFT JOIN prompts p ON p.id = w.prompt_id";

/// 入队一条（幂等）。已存在同 work 的条目 → 返回 false 不改动。
///
/// 幂等是硬要求：验收命令会被长按 ⏎ / 连点重复触发，而 `UNIQUE(work_id)` 只能拦住
/// 重复插入、拦不住「把一条已经在跑的重置回待改写」。故用 `INSERT OR IGNORE`。
pub async fn enqueue(
    tx: &mut Transaction<'_, Sqlite>,
    work_id: i64,
    group_id: Option<i64>,
    group_name: &str,
    batch_id: Option<i64>,
    source_prompt: &str,
    now: i64,
) -> Result<bool, sqlx::Error> {
    let res = sqlx::query(
        "INSERT OR IGNORE INTO v2v_clips
           (work_id, group_id, group_name, batch_id, stage, source_prompt, variable_part,
            created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, 'rewrite', ?5, '', ?6, ?6)",
    )
    .bind(work_id)
    .bind(group_id)
    .bind(group_name)
    .bind(batch_id)
    .bind(source_prompt)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// 按阶段列出（可选按组过滤）。组内按 id 升序 = 验收序，读起来与验收页连贯。
pub async fn list_by_stages(
    pool: &SqlitePool,
    stages: &[&str],
) -> Result<Vec<ClipRow>, sqlx::Error> {
    if stages.is_empty() {
        return Ok(Vec::new());
    }
    let holes = vec!["?"; stages.len()].join(",");
    let sql = format!("{SELECT} WHERE c.stage IN ({holes}) ORDER BY c.group_id, c.id");
    let mut q = sqlx::query_as::<_, ClipRow>(&sql);
    for s in stages {
        q = q.bind(*s);
    }
    q.fetch_all(pool).await
}

pub async fn get(pool: &SqlitePool, id: i64) -> Result<Option<ClipRow>, sqlx::Error> {
    let sql = format!("{SELECT} WHERE c.id = ?1");
    sqlx::query_as::<_, ClipRow>(&sql)
        .bind(id)
        .fetch_optional(pool)
        .await
}

/// 各阶段计数（看板列头 + 侧栏徽章）。
pub async fn stage_counts(pool: &SqlitePool) -> Result<Vec<(String, i64)>, sqlx::Error> {
    sqlx::query_as::<_, (String, i64)>("SELECT stage, COUNT(*) FROM v2v_clips GROUP BY stage")
        .fetch_all(pool)
        .await
}

/// 写入组内公共前后缀剥离结果（工单物化时按组批量算一次）。
pub async fn set_variable_part(
    pool: &SqlitePool,
    id: i64,
    variable_part: &str,
    now: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE v2v_clips SET variable_part = ?2, updated_at = ?3 WHERE id = ?1")
        .bind(id)
        .bind(variable_part)
        .bind(now)
        .execute(pool)
        .await?;
    Ok(())
}

/// 收录改写结果：rewrite → ready。
///
/// **只对 rewrite/ready 生效**：已提交（run）或已出片（rev/pass）的条目不该被一份迟到的
/// rewrite.jsonl 打回去重跑——那会白烧额度，而且看板上那条视频会凭空消失。
pub async fn apply_rewrite(
    tx: &mut Transaction<'_, Sqlite>,
    id: i64,
    video_prompt: &str,
    model_version: Option<&str>,
    duration: Option<i64>,
    video_resolution: Option<&str>,
    now: i64,
) -> Result<bool, sqlx::Error> {
    let res = sqlx::query(
        "UPDATE v2v_clips
            SET stage='ready', video_prompt=?2, model_version=?3, duration=?4,
                video_resolution=?5, rewrote_at=?6, updated_at=?6,
                error_type=NULL, error_message=NULL
          WHERE id=?1 AND stage IN ('rewrite','ready')",
    )
    .bind(id)
    .bind(video_prompt)
    .bind(model_version)
    .bind(duration)
    .bind(video_resolution)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// 人工编辑视频提示词/参数，并**置为待提交**。
///
/// 置 ready 不是顺手：人在「待改写」列手写完提示词，那一步本身就是改写完成了。若只改文字
/// 不动阶段，这条会留在待改写队列里 → 下一次物化仍把它写进工单 → skill 把人写的覆盖掉。
pub async fn update_ready(
    pool: &SqlitePool,
    id: i64,
    video_prompt: &str,
    model_version: Option<&str>,
    duration: Option<i64>,
    video_resolution: Option<&str>,
    now: i64,
) -> Result<bool, sqlx::Error> {
    let res = sqlx::query(
        "UPDATE v2v_clips
            SET stage='ready', video_prompt=?2, model_version=?3, duration=?4,
                video_resolution=?5, rewrote_at=COALESCE(rewrote_at, ?6), updated_at=?6
          WHERE id=?1 AND stage IN ('ready','rewrite')",
    )
    .bind(id)
    .bind(video_prompt)
    .bind(model_version)
    .bind(duration)
    .bind(video_resolution)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// 取待提交（ready）条目，供提交命令批量处理。
pub async fn take_ready(pool: &SqlitePool, ids: &[i64]) -> Result<Vec<ClipRow>, sqlx::Error> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let holes = vec!["?"; ids.len()].join(",");
    let sql = format!("{SELECT} WHERE c.stage='ready' AND c.id IN ({holes}) ORDER BY c.id");
    let mut q = sqlx::query_as::<_, ClipRow>(&sql);
    for i in ids {
        q = q.bind(*i);
    }
    q.fetch_all(pool).await
}

/// 只改生成参数（批量覆盖用），不动提示词也不动阶段。
///
/// 与 [`update_ready`] 分开是因为它们回答不同的问题：那条是「人改完了这一条」（顺带把
/// 它推进待提交），这条是「这十条都换成 seedance2.0_vip」——后者不该把还在待改写的条目
/// 悄悄推进下一列，否则 skill 还没写提示词，它就已经躺在待提交列里等着被提交了。
pub async fn set_params(
    pool: &SqlitePool,
    ids: &[i64],
    model_version: Option<&str>,
    duration: Option<i64>,
    video_resolution: Option<&str>,
    now: i64,
) -> Result<i64, sqlx::Error> {
    if ids.is_empty() {
        return Ok(0);
    }
    let holes = vec!["?"; ids.len()].join(",");
    // 只允许改还没花钱的两列：已提交/已出片的参数改了也不会重新生效，
    // 却会让详情页显示的参数与那条视频实际用的参数对不上。
    // 全用匿名 `?`：SQLite 给匿名占位符编号是「已用最大编号 + 1」，与 `?N` 混用时
    // 那个规则要靠读文档才敢确定，而这里没有任何理由去依赖它。
    let sql = format!(
        "UPDATE v2v_clips SET model_version=?, duration=?, video_resolution=?, updated_at=?
          WHERE stage IN ('rewrite','ready') AND id IN ({holes})"
    );
    let mut q = sqlx::query(&sql)
        .bind(model_version)
        .bind(duration)
        .bind(video_resolution)
        .bind(now);
    for i in ids {
        q = q.bind(*i);
    }
    Ok(q.execute(pool).await?.rows_affected() as i64)
}

/// 记一次轮询结果（0021）：即梦的状态原文 + 队列位次 + 本次查询时刻。
///
/// **不动 `updated_at`**：它是业务变更时间，看板按它排序也按它显示「几分钟前」。
/// 每轮把在跑的条目全刷一遍会让整块看板永远显示「刚刚」，那等于把这个信息删掉。
pub async fn mark_polled(
    pool: &SqlitePool,
    id: i64,
    gen_status: &str,
    queue_idx: Option<i64>,
    now: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE v2v_clips SET gen_status=?2, queue_idx=?3, polled_at=?4 WHERE id=?1")
        .bind(id)
        .bind(gen_status)
        .bind(queue_idx)
        .bind(now)
        .execute(pool)
        .await?;
    Ok(())
}

/// 只记「问过了」，不动状态（查询失败时用）。
///
/// 失败也要落 `polled_at`，否则退避完全失效：CLI 一旦不可用，19 条会在每个 tick 上
/// 各起一个必然失败的进程 —— 正是最该省着点的那种时候反而最费。
pub async fn mark_poll_attempt(pool: &SqlitePool, id: i64, now: i64) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE v2v_clips SET polled_at=?2 WHERE id=?1")
        .bind(id)
        .bind(now)
        .execute(pool)
        .await?;
    Ok(())
}

/// ready → run（记下 submit_id）。
pub async fn mark_submitted(
    pool: &SqlitePool,
    id: i64,
    submit_id: &str,
    now: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE v2v_clips SET stage='run', submit_id=?2, submitted_at=?3, updated_at=?3,
             attempt = attempt + 1, error_type=NULL, error_message=NULL,
             gen_status=NULL, queue_idx=NULL, polled_at=NULL
         WHERE id=?1",
    )
    .bind(id)
    .bind(submit_id)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

/// 轮询器待办：全部 run 态且有 submit_id 的条目。
pub async fn list_running(pool: &SqlitePool) -> Result<Vec<ClipRow>, sqlx::Error> {
    let sql = format!("{SELECT} WHERE c.stage='run' AND c.submit_id IS NOT NULL ORDER BY c.id");
    sqlx::query_as::<_, ClipRow>(&sql).fetch_all(pool).await
}

/// 成片落盘：run → rev（待验收）。
#[allow(clippy::too_many_arguments)] // 视频元数据是一整组，拆结构体只会多一层无信息的包装
pub async fn mark_ready_for_review(
    pool: &SqlitePool,
    id: i64,
    video_path: &str,
    poster_path: Option<&str>,
    width: Option<i64>,
    height: Option<i64>,
    fps: Option<f64>,
    duration_sec: Option<f64>,
    credit_count: Option<i64>,
    benefit_type: Option<&str>,
    now: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE v2v_clips SET stage='rev', video_path=?2, poster_path=?3, width=?4, height=?5,
             fps=?6, duration_sec=?7, credit_count=?8, benefit_type=?10, finished_at=?9,
             updated_at=?9, error_type=NULL, error_message=NULL
         WHERE id=?1",
    )
    .bind(id)
    .bind(video_path)
    .bind(poster_path)
    .bind(width)
    .bind(height)
    .bind(fps)
    .bind(duration_sec)
    .bind(credit_count)
    .bind(now)
    .bind(benefit_type)
    .execute(pool)
    .await?;
    Ok(())
}

/// 失败：→ fail，留错误分类与原文供人判断是重跑还是退回改写。
pub async fn mark_failed(
    pool: &SqlitePool,
    id: i64,
    error_type: &str,
    error_message: &str,
    now: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE v2v_clips SET stage='fail', error_type=?2, error_message=?3,
             finished_at=?4, updated_at=?4 WHERE id=?1",
    )
    .bind(id)
    .bind(error_type)
    .bind(error_message)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

/// 验收定态：rev → pass / rej。
pub async fn set_reviewed(
    pool: &SqlitePool,
    id: i64,
    stage: &str,
    now: i64,
) -> Result<bool, sqlx::Error> {
    let res = sqlx::query(
        "UPDATE v2v_clips SET stage=?2, reviewed_at=?3, updated_at=?3 WHERE id=?1 AND stage='rev'",
    )
    .bind(id)
    .bind(stage)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// 重跑同提示词：rev/rej/fail → ready，清掉上一轮的成片引用。
///
/// 视频不通过多半是**没抽中**而不是提示词不对，故这是不通过后的默认动作。
/// 清 video_path/poster_path 是必须的：旧文件由调用方搬进废纸篓，这里不能再指着它。
pub async fn requeue_for_run(pool: &SqlitePool, id: i64, now: i64) -> Result<bool, sqlx::Error> {
    let res = sqlx::query(
        "UPDATE v2v_clips
            SET stage='ready', submit_id=NULL, video_path=NULL, poster_path=NULL,
                width=NULL, height=NULL, fps=NULL, duration_sec=NULL, credit_count=NULL,
                error_type=NULL, error_message=NULL, finished_at=NULL, reviewed_at=NULL,
                gen_status=NULL, queue_idx=NULL, polled_at=NULL, updated_at=?2
          WHERE id=?1 AND stage IN ('rev','rej','fail','run')",
    )
    .bind(id)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// 继续等待：把判了超时、但**提交单还在**的条目放回 run，让轮询器重新认领。
///
/// 这条路径的存在理由是钱：超时判定只是我们这边不等了，即梦那边任务还在跑、额度已经扣了。
/// 而 [`requeue_for_run`] 会清掉 `submit_id` —— 那意味着重提一次、再花一份钱买同一条视频。
/// 实测 19 条在 45 分钟被判超时时，`dreamina list_task` 里它们全都还是 `querying`。
///
/// 重置 `submitted_at` 是必须的：不重置的话下一轮立刻又判超时，按钮点了等于没点。
pub async fn resume_timed_out(pool: &SqlitePool, id: i64, now: i64) -> Result<bool, sqlx::Error> {
    let res = sqlx::query(
        "UPDATE v2v_clips
            SET stage='run', submitted_at=?2, updated_at=?2,
                error_type=NULL, error_message=NULL, polled_at=NULL
          WHERE id=?1 AND stage='fail' AND submit_id IS NOT NULL AND submit_id <> ''",
    )
    .bind(id)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// 退回改写：任何非 pass 态 → rewrite，清掉旧的视频提示词让 skill 重写。
pub async fn requeue_for_rewrite(
    pool: &SqlitePool,
    id: i64,
    now: i64,
) -> Result<bool, sqlx::Error> {
    let res = sqlx::query(
        "UPDATE v2v_clips
            SET stage='rewrite', video_prompt=NULL, submit_id=NULL, video_path=NULL,
                poster_path=NULL, width=NULL, height=NULL, fps=NULL, duration_sec=NULL,
                credit_count=NULL, error_type=NULL, error_message=NULL,
                finished_at=NULL, reviewed_at=NULL,
                gen_status=NULL, queue_idx=NULL, polled_at=NULL, updated_at=?2
          WHERE id=?1 AND stage <> 'pass'",
    )
    .bind(id)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// 在跑条目的等待时长分布（最久 / 最新 / 条数）。
///
/// 「最久那条等了多久」是过夜跑批时唯一真正要看的数字之一：即梦不回传排队位次，
/// 所以「前面还有几个人」问不出来；但「我这条已经等了多久」我们自己就知道。
pub async fn running_waits(
    pool: &SqlitePool,
) -> Result<(i64, Option<i64>, Option<i64>), sqlx::Error> {
    let row: (i64, Option<i64>, Option<i64>) = sqlx::query_as(
        "SELECT COUNT(*), MIN(submitted_at), MAX(submitted_at)
           FROM v2v_clips WHERE stage='run'",
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

/// 最近一次**出片**的时刻。
///
/// 判「还在动还是卡住了」的主信号：睡前提交 19 条，早上起来看到「上次出片 20 分钟前」
/// 就知道队列在推进，看到「上次出片 9 小时前」就知道该去查了。
/// 只认真出了片的（rev/pass/rej 都经过了 rev），fail 的 `finished_at` 不算 —— 那是判死的时刻。
pub async fn last_finished_at(pool: &SqlitePool) -> Result<Option<i64>, sqlx::Error> {
    let (t,): (Option<i64>,) = sqlx::query_as(
        "SELECT MAX(finished_at) FROM v2v_clips
          WHERE finished_at IS NOT NULL AND stage IN ('rev','pass','rej')",
    )
    .fetch_one(pool)
    .await?;
    Ok(t)
}

/// 最近 `hours` 小时内**逐小时**的出片条数（`[0]` = 最近一小时，越往后越旧）。
///
/// 趋势比总数有用：总数只说「一共出了多少」，逐小时才说得出「速度在涨还是在停」。
pub async fn finish_histogram(
    pool: &SqlitePool,
    now: i64,
    hours: i64,
) -> Result<Vec<i64>, sqlx::Error> {
    let rows: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT (?1 - finished_at) / 3600 AS bucket, COUNT(*)
           FROM v2v_clips
          WHERE finished_at IS NOT NULL AND stage IN ('rev','pass','rej')
            AND finished_at > ?1 - ?2 * 3600
          GROUP BY bucket",
    )
    .bind(now)
    .bind(hours)
    .fetch_all(pool)
    .await?;
    let mut out = vec![0i64; hours.max(1) as usize];
    for (bucket, n) in rows {
        // 边界：finished_at 恰等于 now 时 bucket=0；未来时间戳（时钟回拨）夹到 0。
        let idx = bucket.clamp(0, hours - 1) as usize;
        out[idx] += n;
    }
    Ok(out)
}

/// 额度消耗台账（一行一阶段）。
///
/// **消耗只认 `credit_count`**，那是即梦在出片时随结果回的实际扣费；提交时我们并不知道
/// 这一条会花多少（同一模型不同时长不同价），所以「预估」在这里是编的。
/// 没出片的条目自然没有这一列 —— 那也是真相：钱花没花，只有出片那一刻才有回执。
#[derive(Debug, Clone, FromRow)]
pub struct CreditRow {
    pub stage: String,
    pub spent: i64,
    pub clips: i64,
}

/// 按阶段汇总消耗；`since` 之后完成的另算一份（近 7 天 / 今日）。
pub async fn credit_by_stage(pool: &SqlitePool) -> Result<Vec<CreditRow>, sqlx::Error> {
    sqlx::query_as::<_, CreditRow>(
        "SELECT stage, COALESCE(SUM(credit_count),0) AS spent, COUNT(credit_count) AS clips
           FROM v2v_clips WHERE credit_count IS NOT NULL GROUP BY stage",
    )
    .fetch_all(pool)
    .await
}

/// `since` 之后出片的消耗合计（按 `finished_at`，即扣费回执到手的时刻）。
pub async fn credit_since(pool: &SqlitePool, since: i64) -> Result<i64, sqlx::Error> {
    let (n,): (i64,) = sqlx::query_as(
        "SELECT COALESCE(SUM(credit_count),0) FROM v2v_clips
          WHERE credit_count IS NOT NULL AND finished_at >= ?1",
    )
    .bind(since)
    .fetch_one(pool)
    .await?;
    Ok(n)
}

/// 从流水线移除（不想给这张图做视频了）。
pub async fn remove(pool: &SqlitePool, ids: &[i64]) -> Result<i64, sqlx::Error> {
    if ids.is_empty() {
        return Ok(0);
    }
    let holes = vec!["?"; ids.len()].join(",");
    let sql = format!("DELETE FROM v2v_clips WHERE id IN ({holes})");
    let mut q = sqlx::query(&sql);
    for i in ids {
        q = q.bind(*i);
    }
    Ok(q.execute(pool).await?.rows_affected() as i64)
}

/// 启动恢复：run 态但没有 submit_id 的条目 = 提交过程中被杀进程，退回 ready 让人重提。
///
/// 反过来（有 submit_id 的）**不能**退回：任务已经在即梦那边跑了，额度已扣，
/// 退回重提等于花两份钱买同一条视频。它们由轮询器继续认领。
pub async fn recover_orphan_submits(pool: &SqlitePool, now: i64) -> Result<i64, sqlx::Error> {
    let res = sqlx::query(
        "UPDATE v2v_clips SET stage='ready', updated_at=?1
          WHERE stage='run' AND (submit_id IS NULL OR submit_id='')",
    )
    .bind(now)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() as i64)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即失败
mod tests {
    use super::*;
    use crate::db::test_support::test_pool;

    async fn seed_work(pool: &SqlitePool, work_id: i64) {
        sqlx::query("INSERT OR IGNORE INTO prompt_groups (id,name,prefix,scene,is_temp,created_at) VALUES (1,'g','GG','',0,0)").execute(pool).await.unwrap();
        sqlx::query("INSERT OR IGNORE INTO prompts (id,group_id,code,text,status,source,created_at,updated_at) VALUES (1,1,'GG-0001','t','active','library',0,0)").execute(pool).await.unwrap();
        sqlx::query(
            "INSERT INTO accepted_works (id,image_path,thumb_path,prompt_id,prompt_text,group_id,batch_id,accepted_at)
             VALUES (?1,'/img.jpg','/thumb.jpg',1,'原文',1,7,100)",
        )
        .bind(work_id)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn enqueue_one(pool: &SqlitePool, work_id: i64) -> bool {
        let mut tx = pool.begin().await.unwrap();
        let ok = enqueue(&mut tx, work_id, Some(1), "测试组", Some(7), "原文", 100)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        ok
    }

    // 入队幂等：连点验收/重复验收不得产生同一张图的多条重影，
    // 否则「这张图做到哪了」当场失去唯一答案。
    #[tokio::test]
    async fn enqueue_is_idempotent_per_work() {
        let (pool, _d) = test_pool().await;
        seed_work(&pool, 1).await;
        assert!(enqueue_one(&pool, 1).await, "首次应入队");
        assert!(!enqueue_one(&pool, 1).await, "同一张图不得重复入队");
        let rows = list_by_stages(&pool, &["rewrite"]).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].prompt_code, "GG-0001", "须带出父图编号");
        assert_eq!(rows[0].image_path, "/img.jpg", "须带出首帧原图路径");
    }

    // 重复入队不得把已在跑的条目打回待改写：额度已花，看板上那条会凭空消失。
    #[tokio::test]
    async fn enqueue_does_not_reset_in_flight_clip() {
        let (pool, _d) = test_pool().await;
        seed_work(&pool, 1).await;
        enqueue_one(&pool, 1).await;
        let id = list_by_stages(&pool, &["rewrite"]).await.unwrap()[0].id;
        let mut tx = pool.begin().await.unwrap();
        apply_rewrite(&mut tx, id, "视频提示词", None, None, None, 200)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        mark_submitted(&pool, id, "sub-1", 300).await.unwrap();

        assert!(!enqueue_one(&pool, 1).await);
        let row = get(&pool, id).await.unwrap().unwrap();
        assert_eq!(row.stage, "run", "已提交的条目不得被重复入队打回");
        assert_eq!(row.submit_id.as_deref(), Some("sub-1"));
    }

    // 迟到的 rewrite.jsonl 不得把已提交/已出片的条目打回 ready（白烧额度）。
    #[tokio::test]
    async fn apply_rewrite_only_touches_pre_submit_stages() {
        let (pool, _d) = test_pool().await;
        seed_work(&pool, 1).await;
        enqueue_one(&pool, 1).await;
        let id = list_by_stages(&pool, &["rewrite"]).await.unwrap()[0].id;

        let mut tx = pool.begin().await.unwrap();
        assert!(
            apply_rewrite(&mut tx, id, "第一版", None, None, None, 200)
                .await
                .unwrap(),
            "rewrite 态应可收录"
        );
        tx.commit().await.unwrap();
        mark_submitted(&pool, id, "sub-1", 300).await.unwrap();

        let mut tx = pool.begin().await.unwrap();
        assert!(
            !apply_rewrite(&mut tx, id, "第二版", None, None, None, 400)
                .await
                .unwrap(),
            "已提交的条目不得被改写结果打回"
        );
        tx.commit().await.unwrap();
        let row = get(&pool, id).await.unwrap().unwrap();
        assert_eq!(row.stage, "run");
        assert_eq!(row.video_prompt.as_deref(), Some("第一版"));
    }

    // 重跑清干净上一轮成片引用，但保留视频提示词（重跑 = 同提示词再抽一次）。
    #[tokio::test]
    async fn requeue_for_run_keeps_prompt_clears_media() {
        let (pool, _d) = test_pool().await;
        seed_work(&pool, 1).await;
        enqueue_one(&pool, 1).await;
        let id = list_by_stages(&pool, &["rewrite"]).await.unwrap()[0].id;
        let mut tx = pool.begin().await.unwrap();
        apply_rewrite(
            &mut tx,
            id,
            "视频提示词",
            Some("seedance2.0fast"),
            Some(5),
            None,
            200,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        mark_submitted(&pool, id, "sub-1", 300).await.unwrap();
        mark_ready_for_review(
            &pool,
            id,
            "/clips/1.mp4",
            Some("/clips/1.jpg"),
            Some(960),
            Some(960),
            Some(24.0),
            Some(4.0),
            Some(44),
            None,
            400,
        )
        .await
        .unwrap();
        assert!(set_reviewed(&pool, id, "rej", 500).await.unwrap());

        assert!(requeue_for_run(&pool, id, 600).await.unwrap());
        let row = get(&pool, id).await.unwrap().unwrap();
        assert_eq!(row.stage, "ready");
        assert_eq!(
            row.video_prompt.as_deref(),
            Some("视频提示词"),
            "重跑必须保留提示词（同提示词再抽一次）"
        );
        assert_eq!(row.model_version.as_deref(), Some("seedance2.0fast"));
        assert!(row.video_path.is_none(), "旧成片引用须清空");
        assert!(row.poster_path.is_none(), "旧封面引用须清空");
        assert!(row.submit_id.is_none(), "旧 submit_id 须清空");
        assert_eq!(row.attempt, 1, "attempt 只在真正提交时递增");
    }

    // 退回改写清掉视频提示词，让 skill 重写。
    #[tokio::test]
    async fn requeue_for_rewrite_clears_prompt_but_never_touches_passed() {
        let (pool, _d) = test_pool().await;
        seed_work(&pool, 1).await;
        seed_work(&pool, 2).await;
        enqueue_one(&pool, 1).await;
        enqueue_one(&pool, 2).await;
        let rows = list_by_stages(&pool, &["rewrite"]).await.unwrap();
        let (a, b) = (rows[0].id, rows[1].id);

        for id in [a, b] {
            let mut tx = pool.begin().await.unwrap();
            apply_rewrite(&mut tx, id, "视频提示词", None, None, None, 200)
                .await
                .unwrap();
            tx.commit().await.unwrap();
            mark_submitted(&pool, id, "s", 300).await.unwrap();
            mark_ready_for_review(
                &pool, id, "/v.mp4", None, None, None, None, None, None, None, 400,
            )
            .await
            .unwrap();
        }
        set_reviewed(&pool, a, "rej", 500).await.unwrap();
        set_reviewed(&pool, b, "pass", 500).await.unwrap();

        assert!(requeue_for_rewrite(&pool, a, 600).await.unwrap());
        let row = get(&pool, a).await.unwrap().unwrap();
        assert_eq!(row.stage, "rewrite");
        assert!(row.video_prompt.is_none(), "退回改写须清掉旧视频提示词");

        assert!(
            !requeue_for_rewrite(&pool, b, 600).await.unwrap(),
            "已通过的成片不得被退回（它可能已入资产库/已发布）"
        );
    }

    // 人在「待改写」列手写提示词 = 改写完成 → 必须离开待改写队列。
    // 若只改文字不动阶段，下一次物化仍把它写进工单，skill 会把人写的覆盖掉。
    #[tokio::test]
    async fn hand_written_prompt_leaves_the_rewrite_queue() {
        let (pool, _d) = test_pool().await;
        seed_work(&pool, 1).await;
        enqueue_one(&pool, 1).await;
        let id = list_by_stages(&pool, &["rewrite"]).await.unwrap()[0].id;

        assert!(update_ready(&pool, id, "我自己写的", None, None, None, 200)
            .await
            .unwrap());
        let row = get(&pool, id).await.unwrap().unwrap();
        assert_eq!(row.stage, "ready", "手写完须进待提交，不能留在待改写");
        assert_eq!(row.video_prompt.as_deref(), Some("我自己写的"));
        assert!(
            list_by_stages(&pool, &["rewrite"])
                .await
                .unwrap()
                .is_empty(),
            "待改写队列须已清空（否则工单会再把它写出去）"
        );
    }

    // 已提交/已出片的条目不得被编辑命令改动（额度已花，参数改了也没用，只会误导人）。
    #[tokio::test]
    async fn update_ready_does_not_touch_submitted_clips() {
        let (pool, _d) = test_pool().await;
        seed_work(&pool, 1).await;
        enqueue_one(&pool, 1).await;
        let id = list_by_stages(&pool, &["rewrite"]).await.unwrap()[0].id;
        let mut tx = pool.begin().await.unwrap();
        apply_rewrite(&mut tx, id, "第一版", None, None, None, 200)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        mark_submitted(&pool, id, "sub-1", 300).await.unwrap();

        assert!(!update_ready(&pool, id, "想改", None, None, None, 400)
            .await
            .unwrap());
        let row = get(&pool, id).await.unwrap().unwrap();
        assert_eq!(row.stage, "run");
        assert_eq!(row.video_prompt.as_deref(), Some("第一版"));
    }

    // 验收只对 rev 生效：连点「通过」不得把已定态的条目再改一次。
    #[tokio::test]
    async fn set_reviewed_only_from_rev() {
        let (pool, _d) = test_pool().await;
        seed_work(&pool, 1).await;
        enqueue_one(&pool, 1).await;
        let id = list_by_stages(&pool, &["rewrite"]).await.unwrap()[0].id;
        assert!(
            !set_reviewed(&pool, id, "pass", 100).await.unwrap(),
            "待改写态不可直接验收通过"
        );
        let mut tx = pool.begin().await.unwrap();
        apply_rewrite(&mut tx, id, "p", None, None, None, 200)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        mark_submitted(&pool, id, "s", 300).await.unwrap();
        mark_ready_for_review(
            &pool, id, "/v.mp4", None, None, None, None, None, None, None, 400,
        )
        .await
        .unwrap();
        assert!(set_reviewed(&pool, id, "pass", 500).await.unwrap());
        assert!(
            !set_reviewed(&pool, id, "rej", 600).await.unwrap(),
            "已定态不可再次验收"
        );
    }

    // 中断恢复：**只**把「提交过程中被杀」的（无 submit_id）退回 ready。
    // 有 submit_id 的额度已扣，退回重提等于花两份钱买同一条视频。
    #[tokio::test]
    async fn recovery_never_resubmits_paid_tasks() {
        let (pool, _d) = test_pool().await;
        seed_work(&pool, 1).await;
        seed_work(&pool, 2).await;
        enqueue_one(&pool, 1).await;
        enqueue_one(&pool, 2).await;
        let rows = list_by_stages(&pool, &["rewrite"]).await.unwrap();
        let (orphan, paid) = (rows[0].id, rows[1].id);
        for id in [orphan, paid] {
            let mut tx = pool.begin().await.unwrap();
            apply_rewrite(&mut tx, id, "p", None, None, None, 200)
                .await
                .unwrap();
            tx.commit().await.unwrap();
        }
        // orphan：进了 run 但 submit_id 没写上（进程被杀在两句之间）。
        sqlx::query("UPDATE v2v_clips SET stage='run' WHERE id=?1")
            .bind(orphan)
            .execute(&pool)
            .await
            .unwrap();
        mark_submitted(&pool, paid, "sub-paid", 300).await.unwrap();

        assert_eq!(recover_orphan_submits(&pool, 400).await.unwrap(), 1);
        assert_eq!(get(&pool, orphan).await.unwrap().unwrap().stage, "ready");
        let paid_row = get(&pool, paid).await.unwrap().unwrap();
        assert_eq!(paid_row.stage, "run", "已付费提交的任务必须留给轮询器认领");
        assert_eq!(paid_row.submit_id.as_deref(), Some("sub-paid"));
    }

    // 阶段计数驱动看板列头与侧栏徽章。
    #[tokio::test]
    async fn stage_counts_groups_by_stage() {
        let (pool, _d) = test_pool().await;
        for w in 1..=3 {
            seed_work(&pool, w).await;
            enqueue_one(&pool, w).await;
        }
        let id = list_by_stages(&pool, &["rewrite"]).await.unwrap()[0].id;
        let mut tx = pool.begin().await.unwrap();
        apply_rewrite(&mut tx, id, "p", None, None, None, 200)
            .await
            .unwrap();
        tx.commit().await.unwrap();

        let counts = stage_counts(&pool).await.unwrap();
        let m: std::collections::HashMap<_, _> = counts.into_iter().collect();
        assert_eq!(m.get("rewrite"), Some(&2));
        assert_eq!(m.get("ready"), Some(&1));
    }

    // 轮询快照落库（0021）：切页/重启后「这条在排队还是在跑」仍要答得出。
    // 关键在**不动 updated_at** —— 每 6 秒把全部在跑的条目刷一遍，
    // 会让看板上所有卡片永远显示「刚刚」，等于把那个信息删掉。
    #[tokio::test]
    async fn poll_snapshot_persists_without_touching_updated_at() {
        let (pool, _d) = test_pool().await;
        seed_work(&pool, 1).await;
        enqueue_one(&pool, 1).await;
        let id = list_by_stages(&pool, &["rewrite"]).await.unwrap()[0].id;
        mark_submitted(&pool, id, "sub-1", 500).await.unwrap();
        let before = get(&pool, id).await.unwrap().unwrap().updated_at;

        mark_polled(&pool, id, "queue", Some(3), 900).await.unwrap();
        let row = get(&pool, id).await.unwrap().unwrap();
        assert_eq!(row.gen_status.as_deref(), Some("queue"));
        assert_eq!(row.queue_idx, Some(3));
        assert_eq!(row.polled_at, Some(900));
        assert_eq!(row.updated_at, before, "轮询快照不得刷新业务变更时间");
    }

    // 重跑/退回改写必须把上一轮的即梦状态一起清掉，否则一条刚退回待提交的条目
    // 会带着上一轮的 `success` 显示，看板上就成了「待提交但已成功」。
    #[tokio::test]
    async fn requeue_clears_stale_provider_status() {
        let (pool, _d) = test_pool().await;
        seed_work(&pool, 1).await;
        enqueue_one(&pool, 1).await;
        let id = list_by_stages(&pool, &["rewrite"]).await.unwrap()[0].id;
        mark_submitted(&pool, id, "sub-1", 500).await.unwrap();
        mark_polled(&pool, id, "success", None, 900).await.unwrap();

        assert!(requeue_for_run(&pool, id, 1000).await.unwrap());
        let row = get(&pool, id).await.unwrap().unwrap();
        assert!(row.gen_status.is_none(), "重跑须清掉上一轮的即梦状态");
        assert!(row.polled_at.is_none());
    }

    // 批量改参数只动还没花钱的两列。已提交/已出片的条目改了不会重新生效，
    // 却会让详情页显示的参数与那条视频实际用的参数对不上。
    #[tokio::test]
    async fn bulk_params_skip_clips_that_already_spent_credit() {
        let (pool, _d) = test_pool().await;
        for w in 1..=2 {
            seed_work(&pool, w).await;
            enqueue_one(&pool, w).await;
        }
        let rows = list_by_stages(&pool, &["rewrite"]).await.unwrap();
        let (waiting, submitted) = (rows[0].id, rows[1].id);
        mark_submitted(&pool, submitted, "sub-1", 500)
            .await
            .unwrap();

        let n = set_params(
            &pool,
            &[waiting, submitted],
            Some("seedance2.0_vip"),
            Some(8),
            Some("1080p"),
            600,
        )
        .await
        .unwrap();
        assert_eq!(n, 1, "只该改到还在待改写/待提交的那条");
        let a = get(&pool, waiting).await.unwrap().unwrap();
        assert_eq!(a.model_version.as_deref(), Some("seedance2.0_vip"));
        assert_eq!(a.duration, Some(8));
        assert_eq!(a.stage, "rewrite", "改参数不得把条目推进下一列");
        let b = get(&pool, submitted).await.unwrap().unwrap();
        assert!(b.model_version.is_none(), "已提交的条目参数不得被改写");
    }

    // 队列观测：即梦不回传排队位次，所以「还在动吗」只能靠我们自己测得准的两件事 ——
    // 最久那条等了多久，以及**上次出片距今多久**。后者是过夜跑批第二天早上的主判据。
    #[tokio::test]
    async fn queue_observability_answers_is_it_still_moving() {
        let (pool, _d) = test_pool().await;
        for w in 1..=3 {
            seed_work(&pool, w).await;
            enqueue_one(&pool, w).await;
        }
        let ids: Vec<i64> = list_by_stages(&pool, &["rewrite"])
            .await
            .unwrap()
            .iter()
            .map(|r| r.id)
            .collect();
        let now = 100_000i64;
        // 两条还在跑（一条等了 3 小时、一条等了 10 分钟），第三条已出片。
        mark_submitted(&pool, ids[0], "s0", now - 3 * 3600)
            .await
            .unwrap();
        mark_submitted(&pool, ids[1], "s1", now - 600)
            .await
            .unwrap();
        mark_submitted(&pool, ids[2], "s2", now - 4 * 3600)
            .await
            .unwrap();
        mark_ready_for_review(
            &pool,
            ids[2],
            "/v.mp4",
            None,
            None,
            None,
            None,
            None,
            Some(44),
            Some("dreamina_seedance_20_fast_5s"),
            now - 5400, // 一个半小时前出的片
        )
        .await
        .unwrap();

        let (running, oldest, newest) = running_waits(&pool).await.unwrap();
        assert_eq!(running, 2);
        assert_eq!(now - oldest.unwrap(), 3 * 3600, "最久那条等了 3 小时");
        assert_eq!(now - newest.unwrap(), 600);

        let last = last_finished_at(&pool).await.unwrap().unwrap();
        assert_eq!(now - last, 5400, "上次出片距今 1.5 小时");

        // 逐小时直方图：1.5 小时前那条落在「1 小时前」这一格。
        let h = finish_histogram(&pool, now, 12).await.unwrap();
        assert_eq!(h.len(), 12);
        assert_eq!(h[1], 1, "1.5 小时前 → bucket 1");
        assert_eq!(h[0], 0, "最近一小时没出片");
        assert_eq!(h.iter().sum::<i64>(), 1);

        // 计费型号来自回执，回答「到底走的哪个模型」。
        let done = get(&pool, ids[2]).await.unwrap().unwrap();
        assert_eq!(
            done.benefit_type.as_deref(),
            Some("dreamina_seedance_20_fast_5s")
        );
    }

    // 判死的条目（fail）**不算出片**：它的 finished_at 是被判死的时刻，
    // 混进来会让「上次出片 20 分钟前」变成一句假话，而人正是拿它决定要不要去查。
    #[tokio::test]
    async fn timeout_failures_do_not_count_as_deliveries() {
        let (pool, _d) = test_pool().await;
        seed_work(&pool, 1).await;
        enqueue_one(&pool, 1).await;
        let id = list_by_stages(&pool, &["rewrite"]).await.unwrap()[0].id;
        mark_submitted(&pool, id, "s", 1_000).await.unwrap();
        mark_failed(&pool, id, "timeout", "45 分钟仍未出片", 99_000)
            .await
            .unwrap();
        assert!(last_finished_at(&pool).await.unwrap().is_none());
        assert_eq!(
            finish_histogram(&pool, 100_000, 12)
                .await
                .unwrap()
                .iter()
                .sum::<i64>(),
            0
        );
    }

    // 超时判死的条目**提交单还在**，所以能原样放回轮询 —— 那 19 条被判超时时，
    // `dreamina list_task` 里它们全都还是 querying。走重跑就是再花一份钱买同一条视频。
    #[tokio::test]
    async fn timed_out_clip_resumes_on_its_original_submit_id() {
        let (pool, _d) = test_pool().await;
        seed_work(&pool, 1).await;
        enqueue_one(&pool, 1).await;
        let id = list_by_stages(&pool, &["rewrite"]).await.unwrap()[0].id;
        mark_submitted(&pool, id, "sub-paid", 500).await.unwrap();
        mark_failed(&pool, id, "timeout", "45 分钟仍未出片", 3_200)
            .await
            .unwrap();

        assert!(resume_timed_out(&pool, id, 9_000).await.unwrap());
        let row = get(&pool, id).await.unwrap().unwrap();
        assert_eq!(row.stage, "run");
        assert_eq!(
            row.submit_id.as_deref(),
            Some("sub-paid"),
            "必须沿用原提交单，否则等于重新花钱"
        );
        assert_eq!(
            row.submitted_at,
            Some(9_000),
            "不重置提交时刻的话下一轮立刻又判超时，按钮点了等于没点"
        );
        assert!(row.error_type.is_none());
        assert_eq!(row.attempt, 1, "继续等待不是新一次尝试，不该 attempt+1");
    }

    // 没有提交单的失败（提交本身就没成功）**不能**放回轮询：
    // 那会造出一条永远查不到结果的 run，只能等超时再失败一次。
    #[tokio::test]
    async fn failed_without_submit_id_cannot_be_resumed() {
        let (pool, _d) = test_pool().await;
        seed_work(&pool, 1).await;
        enqueue_one(&pool, 1).await;
        let id = list_by_stages(&pool, &["rewrite"]).await.unwrap()[0].id;
        mark_failed(&pool, id, "submit", "找不到即梦 CLI", 600)
            .await
            .unwrap();
        assert!(!resume_timed_out(&pool, id, 900).await.unwrap());
        assert_eq!(get(&pool, id).await.unwrap().unwrap().stage, "fail");
    }

    // 额度台账按阶段分账：成片 / 未通过（白花的）/ 还没定论。
    // 没有 credit_count 的条目不计入 —— 钱花没花，只有出片那一刻才有回执。
    #[tokio::test]
    async fn credit_ledger_splits_by_stage_and_ignores_unbilled() {
        let (pool, _d) = test_pool().await;
        for w in 1..=3 {
            seed_work(&pool, w).await;
            enqueue_one(&pool, w).await;
        }
        let ids: Vec<i64> = list_by_stages(&pool, &["rewrite"])
            .await
            .unwrap()
            .iter()
            .map(|r| r.id)
            .collect();
        // 两条出片（44 与 66 额度），第三条还没提交 → 无回执。
        for (id, credit, at) in [(ids[0], 44, 1_000), (ids[1], 66, 2_000)] {
            mark_submitted(&pool, id, "s", at).await.unwrap();
            mark_ready_for_review(
                &pool,
                id,
                "/v.mp4",
                None,
                Some(960),
                Some(960),
                Some(24.0),
                Some(4.0),
                Some(credit),
                None,
                at,
            )
            .await
            .unwrap();
        }
        set_reviewed(&pool, ids[0], "pass", 3_000).await.unwrap();
        set_reviewed(&pool, ids[1], "rej", 3_000).await.unwrap();

        let m: std::collections::HashMap<String, i64> = credit_by_stage(&pool)
            .await
            .unwrap()
            .into_iter()
            .map(|r| (r.stage, r.spent))
            .collect();
        assert_eq!(m.get("pass"), Some(&44));
        assert_eq!(m.get("rej"), Some(&66), "未通过的额度照样花掉了");
        assert_eq!(m.len(), 2, "没有扣费回执的条目不得计入");
        // 按出片时刻切窗：1_000 那条落在窗外。
        assert_eq!(credit_since(&pool, 1_500).await.unwrap(), 66);
        assert_eq!(credit_since(&pool, 0).await.unwrap(), 110);
    }
}
