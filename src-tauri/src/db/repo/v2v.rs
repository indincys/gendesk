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
    /// 排队位次（见 `dreamina::QueryResult::queue_idx`）。健康的任务从排队第一秒起就有它，
    /// 缺席是幽灵单的征兆之一。
    pub queue_idx: Option<i64>,
    /// 最后一次**发起查询**的时刻（成功与否都记）。退避轮询据此决定这一条到点没有。
    pub polled_at: Option<i64>,
    /// 实际计费型号（0022，来自回执）。回答「到底走的哪个模型」——
    /// 用我们自己发出去的 `model_version` 回答等于自问自答。
    pub benefit_type: Option<String>,
    /// **首次**提交时刻（0024）。与 `submitted_at` 的差别就是「继续等待」：
    /// 那个按钮会把 `submitted_at` 重置成当下（否则下一轮立刻又判超时），
    /// 而这一列不动，于是「这条到底等了多久」还答得出来。
    pub first_submitted_at: Option<i64>,
    /// 提交回执里的 `credit_count`（0024）。健康的提交当场就有它；
    /// 它与 `queue_idx` 双双缺席即幽灵单（`runner::is_phantom`）。
    pub submit_credit: Option<i64>,
    /// 提交回执里的 `gen_status`（0024）。
    pub submit_status: Option<String>,
    /// 入队时刻 —— 详情栏「这一条的历程」第一格（图片验收通过 · 自动入队）。
    pub created_at: i64,
    /// 改写结果收录时刻（skill 写回 / 人手写完）。
    pub rewrote_at: Option<i64>,
    /// 出片落盘（或判死）时刻。
    pub finished_at: Option<i64>,
    /// 人工定态（通过/不通过）时刻。
    pub reviewed_at: Option<i64>,
    /// 这一条是补单器放行的还是人放行的（0026）。
    pub auto_submitted: i64,
    /// 打包进资产库时留下的素材包 id（0025）。
    pub asset_pack_id: Option<i64>,
    /// 那个素材包**现在还在不在**（包被退役删除后应回落成「尚未入库」）。
    /// SQLite 的 EXISTS 回 0/1，故用 i64 承接。
    /// 验收通过后交付到 `outputs/视频/` 的那份拷贝（0027）。有它才答得出「片子在哪」。
    pub export_path: Option<String>,
    pub updated_at: i64,
    /// 父图编号（`accepted_works.prompt_code` 快照）。
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
        c.submitted_at, c.gen_status, c.queue_idx, c.polled_at, c.benefit_type,
        c.first_submitted_at, c.submit_credit, c.submit_status,
        c.created_at, c.rewrote_at, c.finished_at, c.reviewed_at,
        c.auto_submitted, c.asset_pack_id,
        c.export_path, c.updated_at,
        COALESCE(w.prompt_code,'') AS prompt_code,
        COALESCE(w.image_path,'') AS image_path,
        COALESCE(w.thumb_path,'') AS thumb_path,
        COALESCE(w.accepted_at,0) AS accepted_at
    FROM v2v_clips c
    LEFT JOIN accepted_works w ON w.id = c.work_id";

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

/// 只读地列出待提交（ready）条目 —— 给「提交前预览命令行与预估额度」用。
///
/// 预览不能认领：人打开确认卡后可能直接关掉，而认领会把这一批推进 `run`。
/// 真正要提交时走 [`claim_ready`]。
pub async fn list_ready(pool: &SqlitePool, ids: &[i64]) -> Result<Vec<ClipRow>, sqlx::Error> {
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

/// **认领**待提交（ready）条目：`ready` → `run`（此刻还没有 submit_id）。
///
/// 只读不写地「取一批 ready」是不够的：两处入口会同时来拿同一条 —— 人点「确认提交」
/// 与常驻队列补单器（`v2v::autofill`）跑在不同任务里，而两者之间隔着整个 CLI 提交的
/// 网络往返。两个都读到同一条 `ready`，就会为同一张图向即梦下**两次**单、扣两份钱，
/// 而 `UNIQUE(work_id)` 拦不住它（自始至终只有一行，第二次提交只是覆盖 submit_id，
/// 第一张片子当场变成认不出主人的孤儿）。
///
/// 认领必须发生在**提交之前**。这与「额度不可撤回」不冲突：这一步还没花钱，
/// 而进程若恰好被杀在认领与写 submit_id 之间，`recover_orphan_submits` 会把
/// 「run 但没有 submit_id」的条目退回 ready —— 那条恢复路径本来就是为这个窗口写的。
///
/// 认领不成功的条目静静跳过：它要么被别人抢走了，要么已经不是 ready（被退回改写/
/// 被删）。返回的行是**认领到的那些**，调用方对它们负责到底。
pub async fn claim_ready(
    pool: &SqlitePool,
    ids: &[i64],
    now: i64,
) -> Result<Vec<ClipRow>, sqlx::Error> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut claimed: Vec<i64> = Vec::new();
    for id in ids {
        let res = sqlx::query(
            "UPDATE v2v_clips SET stage='run', updated_at=?2 WHERE id=?1 AND stage='ready'",
        )
        .bind(*id)
        .bind(now)
        .execute(pool)
        .await?;
        if res.rows_affected() > 0 {
            claimed.push(*id);
        }
    }
    if claimed.is_empty() {
        return Ok(Vec::new());
    }
    let holes = vec!["?"; claimed.len()].join(",");
    let sql = format!("{SELECT} WHERE c.id IN ({holes}) ORDER BY c.id");
    let mut q = sqlx::query_as::<_, ClipRow>(&sql);
    for i in &claimed {
        q = q.bind(*i);
    }
    q.fetch_all(pool).await
}

/// 放回认领：`run`（无 submit_id）→ `ready`。
///
/// 只在**确定没花钱**的分支上调用（比如这一条根本没有视频提示词，压根没提交）。
/// `submit_id IS NULL` 这个谓词是硬闸门：有 submit_id 就意味着钱已经扣了，
/// 放回 ready 等于让它被重新提交一次 —— 花两份钱买同一条视频。
pub async fn release_claim(pool: &SqlitePool, id: i64, now: i64) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE v2v_clips SET stage='ready', updated_at=?2
          WHERE id=?1 AND stage='run' AND submit_id IS NULL",
    )
    .bind(id)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
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
///
/// `queue_idx` 走 `COALESCE` 与 [`mark_swept`] 同形：位次是幽灵判定的两个信号之一，
/// 而它在任务离开排队、开始生成之后就不再出现在回体里 —— 无条件写回等于在这一刻
/// 亲手抹掉「这条确实进过队列」的证据。那条注释里写的保护意图，要两处都 COALESCE
/// 才真正成立。
pub async fn mark_polled(
    pool: &SqlitePool,
    id: i64,
    gen_status: &str,
    queue_idx: Option<i64>,
    now: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE v2v_clips
            SET gen_status=?2, queue_idx=COALESCE(?3, queue_idx), polled_at=?4
          WHERE id=?1",
    )
    .bind(id)
    .bind(gen_status)
    .bind(queue_idx)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

/// 记一次**整表扫描**（`list_task`）的观测结果。
///
/// 与 [`mark_polled`] 的差别全在 `COALESCE`：扫描回体里 `queue_info` 恒缺席，
/// 而 `queue_idx` 是幽灵单判定的两个信号之一 —— 把已经问到过的位次覆盖成 NULL，
/// 等于亲手抹掉「这条确实进过队列」的证据，下一轮就可能把它误判成幽灵单。
/// 同理 `credit_count` 与 `benefit_type` 只增不抹。
///
/// **计费在扫描里就落库**（实测：排队中的条目在 `list_task` 里已带 `credit_count`）。
/// 早前那句「只有出片那一刻才有回执」是从 `query_result` 一条路径上归纳的，
/// 而钱在提交那一刻就扣掉了 —— 越早记下来，「这批到底花了多少」越早答得准。
pub async fn mark_swept(
    pool: &SqlitePool,
    id: i64,
    gen_status: &str,
    credit_count: Option<i64>,
    benefit_type: Option<&str>,
    now: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE v2v_clips
            SET gen_status=?2,
                credit_count=COALESCE(credit_count, ?3),
                benefit_type=COALESCE(benefit_type, ?4),
                polled_at=?5
          WHERE id=?1",
    )
    .bind(id)
    .bind(gen_status)
    .bind(credit_count)
    .bind(benefit_type)
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

/// ready → run（记下 submit_id 与整份提交回执）。
///
/// `first_submitted_at` 在这里**无条件**更新：走到这一步就是一个新的 submit_id，
/// 从它开始重新计时才对。会被「继续等待」重置的是 `submitted_at`（那条路径不换单，
/// 故不动 `first_submitted_at`）—— 两列的分工全部体现在这个差别上。
pub async fn mark_submitted(
    pool: &SqlitePool,
    id: i64,
    receipt: &crate::v2v::dreamina::SubmitReceipt,
    now: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE v2v_clips SET stage='run', submit_id=?2, submitted_at=?3, updated_at=?3,
             first_submitted_at=?3, submit_credit=?4, submit_status=?5,
             attempt = attempt + 1, error_type=NULL, error_message=NULL,
             gen_status=NULL, queue_idx=NULL, polled_at=NULL
         WHERE id=?1",
    )
    .bind(id)
    .bind(&receipt.submit_id)
    .bind(now)
    .bind(receipt.credit_count)
    .bind(&receipt.gen_status)
    .execute(pool)
    .await?;
    Ok(())
}

/// 轮询器待办：全部 run 态且有 submit_id 的条目。
pub async fn list_running(pool: &SqlitePool) -> Result<Vec<ClipRow>, sqlx::Error> {
    let sql = format!("{SELECT} WHERE c.stage='run' AND c.submit_id IS NOT NULL ORDER BY c.id");
    sqlx::query_as::<_, ClipRow>(&sql).fetch_all(pool).await
}

/// 在跑条目各自写死的模型（去重）。轮询档位据此决定走 VIP 档还是非 VIP 档。
///
/// 返回 `Option` 而不是折成字符串：`NULL`/空串意味着「跟随设置里的默认」，
/// 而那个默认只有调用方（读得到设置）才知道。在这里替它填上，等于把回落规则
/// 抄成第二份。
pub async fn running_models(pool: &SqlitePool) -> Result<Vec<Option<String>>, sqlx::Error> {
    let rows: Vec<(Option<String>,)> = sqlx::query_as(
        "SELECT DISTINCT model_version FROM v2v_clips
          WHERE stage='run' AND submit_id IS NOT NULL",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(m,)| m).collect())
}

/// 成片落盘：run → rev（待验收）。
///
/// `credit_count` / `benefit_type` 走 `COALESCE`（同 [`mark_swept`]）：出片那一份回体
/// 未必再带计费字段，而钱在提交那一刻就扣了、扫描一路上也可能早就问到过。无条件写回
/// 等于**在出片那一刻把这条的账抹掉** —— 「这一批花了多少」从此再也答不准。
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
             fps=?6, duration_sec=?7,
             credit_count=COALESCE(?8, credit_count),
             benefit_type=COALESCE(?10, benefit_type),
             finished_at=?9,
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

/// 记下（或清掉）交付拷贝的路径（0027）。
///
/// 与 `set_reviewed` 分开而不是塞进同一条 UPDATE：拷贝是**文件操作**，它可能失败
/// （盘满、目标被占用），而那不该让「这一条已经通过验收」这个判定跟着回滚。
/// 通过了但没拷成，是一条可以事后补的记录；判定丢了才是真丢东西。
pub async fn set_export_path(
    pool: &SqlitePool,
    id: i64,
    path: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE v2v_clips SET export_path = ?2 WHERE id = ?1")
        .bind(id)
        .bind(path)
        .execute(pool)
        .await?;
    Ok(())
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
/// 但**不动 `first_submitted_at`** —— 重置的代价原先是原始提交时刻被永久覆盖，
/// 事故当天看板因此显示「最久已等 10 小时 54 分」，而那只是从按下这个按钮算起的。
///
/// 只放 `timeout`：`phantom` 那类没进队列、没扣费，「继续等待」对它毫无意义
/// （再等一万年也不会出片），该走的是重跑。
pub async fn resume_timed_out(pool: &SqlitePool, id: i64, now: i64) -> Result<bool, sqlx::Error> {
    let res = sqlx::query(
        "UPDATE v2v_clips
            SET stage='run', submitted_at=?2, updated_at=?2,
                error_type=NULL, error_message=NULL, polled_at=NULL
          WHERE id=?1 AND stage='fail' AND error_type='timeout'
            AND submit_id IS NOT NULL AND submit_id <> ''",
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
/// **消耗只认 `credit_count`**，那是即梦给的实际扣费回执，不是我们按单价表算出来的估值。
///
/// 0026 起它在**整表扫描**里就落库（实测排队中的条目在 `list_task` 里已带
/// `credit_count`），不必等到出片 —— 钱本来就是在提交那一刻扣的。故在跑（run）的条目
/// 现在也会带着自己的账出现在这份台账里，归入「未定论」那一档：它确实花掉了，
/// 只是还不知道值不值。
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

/// 验收通过了却没交付到输出目录的条数（成片库徽章）。
///
/// 这是成片这条链上**唯一一处会无声断掉的地方**：验收通过时的拷贝失败不回滚验收
/// （判定是人做的，文件可以补），于是「片子做出来了却没落地」在库里是一个完全合法、
/// 界面上又完全看不见的状态。徽章把它变成待办。
///
/// 只看 `export_path` 是否为空，**不去 stat 文件**：这个计数每 6 秒随事件算一次，
/// 而磁盘上的成片可能在网络卷上。「文件后来被人删了」由「重新交付」那条路径兜底。
pub async fn count_pass_undelivered(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
    let (n,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM v2v_clips
          WHERE stage='pass' AND (export_path IS NULL OR TRIM(export_path)='')",
    )
    .fetch_one(pool)
    .await?;
    Ok(n)
}

/// 幽灵疑单的条数：在跑、任何一处计费证据都没有、且过了宽限期。
///
/// 徽章要计入它，因为它是**唯一一类阻在人身上却不在四个待办阶段里**的条目 ——
/// 它躺在 `run`（「机器在跑，人插不上手」），可它恰恰是机器根本没在跑的那些，
/// 而处置是免费重跑。事故那次 18 条这样的单挂了十几个小时，徽章全程是 0。
///
/// 判据与 `runner::clip_looks_phantom` 是同一条，只是这里在 SQL 里数（徽章要的是一个
/// 数字，把整表拉回来过一遍纯属浪费）。**两处必须一起改** —— 有测试守住它们一致。
/// `before` = `now - runner::PHANTOM_GRACE_SECS`，宽限期常量留在 runner 一处定义。
pub async fn count_phantom_suspects(pool: &SqlitePool, before: i64) -> Result<i64, sqlx::Error> {
    let (n,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM v2v_clips
          WHERE stage='run' AND submit_id IS NOT NULL
            AND credit_count IS NULL AND submit_credit IS NULL AND queue_idx IS NULL
            AND submitted_at IS NOT NULL AND submitted_at < ?1",
    )
    .bind(before)
    .fetch_one(pool)
    .await?;
    Ok(n)
}

/// 补单器自己放出去、此刻还在跑的条数（0026）。
///
/// **只数它自己的**：人手动提交的一批不该顶掉补单器的深度配额，否则手动跑 20 条时
/// 常驻队列就静悄悄停摆了，而「常年保持有任务在排队」正是它存在的全部理由。
pub async fn count_auto_running(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
    let (n,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM v2v_clips WHERE stage='run' AND auto_submitted=1")
            .fetch_one(pool)
            .await?;
    Ok(n)
}

/// 补单器能碰的条目：待提交、有视频提示词、且**没人给它指定过模型**。
///
/// 最后一条是硬边界。补单器会把自己的廉价参数写进它挑中的条目 —— 若它捡走一条用户
/// （或 skill）特意设了 `seedance2.0_vip / 1080p` 的片子，那份选择会被静悄悄降级，
/// 而人要到出片时才看得出来。指定过参数 = 一个深思熟虑的决定，常驻队列不碰它。
const AUTOFILL_POOL: &str = "stage='ready'
      AND video_prompt IS NOT NULL AND TRIM(video_prompt) <> ''
      AND (model_version IS NULL OR TRIM(model_version) = '')";

/// 可供补单的存量条数。
///
/// 待改写的不算 —— 那些还等着 skill 写回，补单器碰不到它们。这个数正是「告急」
/// 那条通知要报的东西：它见底就意味着常驻队列即将断流，而补起来要人去写提示词。
pub async fn count_autofill_pool(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
    let (n,): (i64,) = sqlx::query_as(&format!(
        "SELECT COUNT(*) FROM v2v_clips WHERE {AUTOFILL_POOL}"
    ))
    .fetch_one(pool)
    .await?;
    Ok(n)
}

/// 挑 `limit` 条来补单。**先进先出**（按 id）：排最久的先走，
/// 免得后进来的一直插队，让最早那批永远轮不到。
pub async fn pick_autofill(pool: &SqlitePool, limit: i64) -> Result<Vec<i64>, sqlx::Error> {
    let rows: Vec<(i64,)> = sqlx::query_as(&format!(
        "SELECT id FROM v2v_clips WHERE {AUTOFILL_POOL} ORDER BY id LIMIT ?1"
    ))
    .bind(limit.max(0))
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// 打上「补单器放行」的标记。**必须在提交之前**打：提交成功那一刻就已经扣钱了，
/// 事后再标会在进程恰好被杀时留下一条认不出主人的在跑条目。
pub async fn mark_auto(pool: &SqlitePool, ids: &[i64], now: i64) -> Result<i64, sqlx::Error> {
    if ids.is_empty() {
        return Ok(0);
    }
    let holes = vec!["?"; ids.len()].join(",");
    let sql = format!(
        "UPDATE v2v_clips SET auto_submitted=1, updated_at=? WHERE stage='ready' AND id IN ({holes})"
    );
    let mut q = sqlx::query(&sql).bind(now);
    for i in ids {
        q = q.bind(*i);
    }
    Ok(q.execute(pool).await?.rows_affected() as i64)
}

/// `since` 之后**提交**掉的额度（按提交回执与首次提交时刻）。
///
/// 与 [`credit_since`] 是两个问题：那条按 `finished_at` 切，答的是「今天出的片花了多少」；
/// 这条按 `first_submitted_at` 切，答的是「今天已经花出去多少」。
/// 自动补单的日限必须用后者 —— 用前者的话，补单器可以在任何一条出片之前把一整天的
/// 额度提交光，而那个上限从头到尾都不会触发。
pub async fn credit_submitted_since(pool: &SqlitePool, since: i64) -> Result<i64, sqlx::Error> {
    let (n,): (i64,) = sqlx::query_as(
        "SELECT COALESCE(SUM(COALESCE(credit_count, submit_credit)),0) FROM v2v_clips
          WHERE first_submitted_at IS NOT NULL AND first_submitted_at >= ?1",
    )
    .bind(since)
    .fetch_one(pool)
    .await?;
    Ok(n)
}

/// 一条 clip 的**可撤销列**快照。
///
/// 撤销之所以要整份快照而不是「把 stage 改回去」：`requeue_for_run` / `requeue_for_rewrite`
/// 会连带清掉 submit_id、成片路径、尺寸、扣费回执 —— 只把 stage 拨回 `rev` 会留下一条
/// 「待验收但没有片子」的行，比不给撤销更糟。快照在改动**之前**取，撤销即整份写回。
#[derive(Debug, Clone, FromRow)]
pub struct ClipSnapshot {
    pub id: i64,
    pub stage: String,
    pub video_prompt: Option<String>,
    pub submit_id: Option<String>,
    pub video_path: Option<String>,
    pub poster_path: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub fps: Option<f64>,
    pub duration_sec: Option<f64>,
    pub credit_count: Option<i64>,
    pub error_type: Option<String>,
    pub error_message: Option<String>,
    pub gen_status: Option<String>,
    pub queue_idx: Option<i64>,
    pub polled_at: Option<i64>,
    pub submitted_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub reviewed_at: Option<i64>,
    pub attempt: i64,
}

/// 取快照（撤销令牌的原料）。
pub async fn snapshot(pool: &SqlitePool, id: i64) -> Result<Option<ClipSnapshot>, sqlx::Error> {
    sqlx::query_as::<_, ClipSnapshot>(
        "SELECT id, stage, video_prompt, submit_id, video_path, poster_path, width, height,
                fps, duration_sec, credit_count, error_type, error_message, gen_status,
                queue_idx, polled_at, submitted_at, finished_at, reviewed_at, attempt
           FROM v2v_clips WHERE id = ?1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
}

/// 整份写回快照（撤销）。
pub async fn restore(pool: &SqlitePool, s: &ClipSnapshot, now: i64) -> Result<bool, sqlx::Error> {
    let res = sqlx::query(
        "UPDATE v2v_clips
            SET stage=?2, video_prompt=?3, submit_id=?4, video_path=?5, poster_path=?6,
                width=?7, height=?8, fps=?9, duration_sec=?10, credit_count=?11,
                error_type=?12, error_message=?13, gen_status=?14, queue_idx=?15,
                polled_at=?16, submitted_at=?17, finished_at=?18, reviewed_at=?19,
                attempt=?20, updated_at=?21
          WHERE id=?1",
    )
    .bind(s.id)
    .bind(&s.stage)
    .bind(&s.video_prompt)
    .bind(&s.submit_id)
    .bind(&s.video_path)
    .bind(&s.poster_path)
    .bind(s.width)
    .bind(s.height)
    .bind(s.fps)
    .bind(s.duration_sec)
    .bind(s.credit_count)
    .bind(&s.error_type)
    .bind(&s.error_message)
    .bind(&s.gen_status)
    .bind(s.queue_idx)
    .bind(s.polled_at)
    .bind(s.submitted_at)
    .bind(s.finished_at)
    .bind(s.reviewed_at)
    .bind(s.attempt)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// 最近一次收录改写结果的时刻（交接状态那句「N 分钟前收录」）。
///
/// 不另存一个「上次收录时间」设置项：`rewrote_at` 的最大值**就是**这件事，
/// 而多一处真相来源就多一处会与库里对不上的地方。
pub async fn last_rewrote_at(pool: &SqlitePool) -> Result<Option<i64>, sqlx::Error> {
    let (t,): (Option<i64>,) = sqlx::query_as("SELECT MAX(rewrote_at) FROM v2v_clips")
        .fetch_one(pool)
        .await?;
    Ok(t)
}

/// 「你离开的这段时间」发生了什么（开屏横幅）。
#[derive(Debug, Clone, Default, FromRow)]
pub struct AwayRow {
    /// 出片条数（真出了片的，fail 不算）。
    pub finished: i64,
    /// 判死条数。
    pub failed: i64,
    /// 其中的幽灵单（没入队、没计费，重跑不花钱）。
    pub phantom: i64,
    /// 这段时间内实际扣掉的额度（出片回执之和）。
    pub credits: i64,
}

/// `since` 之后的出片/判死/扣费统计。
///
/// 全部按 `finished_at` 切：那是「这件事发生」的时刻。用 `updated_at` 会把用户自己刚做的
/// 验收动作也算进「离开期间发生的事」，横幅就会开始复述人自己刚点过的操作。
pub async fn away_digest(pool: &SqlitePool, since: i64) -> Result<AwayRow, sqlx::Error> {
    sqlx::query_as::<_, AwayRow>(
        "SELECT
           COALESCE(SUM(CASE WHEN stage IN ('rev','pass','rej') THEN 1 ELSE 0 END),0) AS finished,
           COALESCE(SUM(CASE WHEN stage='fail' THEN 1 ELSE 0 END),0) AS failed,
           COALESCE(SUM(CASE WHEN stage='fail' AND error_type='phantom' THEN 1 ELSE 0 END),0)
             AS phantom,
           COALESCE(SUM(CASE WHEN stage IN ('rev','pass','rej') THEN credit_count ELSE 0 END),0)
             AS credits
         FROM v2v_clips WHERE finished_at IS NOT NULL AND finished_at >= ?1",
    )
    .bind(since)
    .fetch_one(pool)
    .await
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
    use crate::v2v::dreamina::SubmitReceipt;

    async fn seed_work(pool: &SqlitePool, work_id: i64) {
        sqlx::query("INSERT OR IGNORE INTO prompt_groups (id,name,prefix,scene,is_temp,created_at) VALUES (1,'g','GG','',0,0)").execute(pool).await.unwrap();
        sqlx::query("INSERT OR IGNORE INTO prompts (id,group_id,code,text,status,source,created_at,updated_at) VALUES (1,1,'GG-0001','t','active','library',0,0)").execute(pool).await.unwrap();
        sqlx::query(
            "INSERT INTO accepted_works (id,image_path,thumb_path,prompt_id,prompt_text,group_id,batch_id,accepted_at,prompt_code,group_name)
             VALUES (?1,'/img.jpg','/thumb.jpg',1,'原文',1,7,100,'GG-0001','g')",
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

    /// 建一条走到 rev（待验收）的 clip，返回它的 id。撤销/入库那几条测试的共同起点。
    async fn seed_reviewable(pool: &SqlitePool, work_id: i64) -> i64 {
        seed_work(pool, work_id).await;
        enqueue_one(pool, work_id).await;
        let id = list_by_stages(pool, &["rewrite"]).await.unwrap()[0].id;
        let mut tx = pool.begin().await.unwrap();
        apply_rewrite(&mut tx, id, "视频提示词", None, None, None, 200)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        mark_submitted(pool, id, &SubmitReceipt::healthy("sub-1", 8), 300)
            .await
            .unwrap();
        mark_ready_for_review(
            pool,
            id,
            "/clips/1.mp4",
            Some("/clips/1.jpg"),
            Some(720),
            Some(1280),
            Some(24.0),
            Some(4.0),
            Some(8),
            Some("dreamina_seedance_20_fast"),
            400,
        )
        .await
        .unwrap();
        id
    }

    // 常驻队列绝不碰「有人指定过模型」的条目。
    //
    // 它会把自己的廉价参数写进挑中的条目 —— 捡走一条特意设了 vip / 1080p 的片子，
    // 那份选择会被静悄悄降级，而人要到出片时才看得出来（那时钱已经花完了）。
    #[tokio::test]
    async fn autofill_never_touches_a_clip_someone_configured() {
        let (pool, _d) = test_pool().await;
        for w in 1..=3 {
            seed_work(&pool, w).await;
            enqueue_one(&pool, w).await;
        }
        let ids: Vec<i64> = list_by_stages(&pool, &["rewrite"])
            .await
            .unwrap()
            .iter()
            .map(|c| c.id)
            .collect();
        for id in &ids {
            let mut tx = pool.begin().await.unwrap();
            apply_rewrite(&mut tx, *id, "视频提示词", None, None, None, 200)
                .await
                .unwrap();
            tx.commit().await.unwrap();
        }
        assert_eq!(count_autofill_pool(&pool).await.unwrap(), 3);

        // 有人给第二条挑了 vip 通道 —— 从此它不在补单器的候选里。
        let second = ids[1];
        set_params(
            &pool,
            &[second],
            Some("seedance2.0_vip"),
            Some(4),
            Some("1080p"),
            300,
        )
        .await
        .unwrap();
        assert_eq!(count_autofill_pool(&pool).await.unwrap(), 2);
        let picked = pick_autofill(&pool, 10).await.unwrap();
        assert!(
            !picked.contains(&second),
            "指定过参数的条目不得被补单器捡走"
        );
        assert_eq!(picked, vec![ids[0], ids[2]], "其余按 id 先进先出");
    }

    // 深度只数补单器自己放出去的：人手动跑 20 条时，常驻队列不该静悄悄停摆。
    #[tokio::test]
    async fn autofill_depth_counts_only_its_own_submissions() {
        let (pool, _d) = test_pool().await;
        for w in 1..=2 {
            seed_work(&pool, w).await;
            enqueue_one(&pool, w).await;
        }
        let ids: Vec<i64> = list_by_stages(&pool, &["rewrite"])
            .await
            .unwrap()
            .iter()
            .map(|c| c.id)
            .collect();
        for id in &ids {
            let mut tx = pool.begin().await.unwrap();
            apply_rewrite(&mut tx, *id, "p", None, None, None, 200)
                .await
                .unwrap();
            tx.commit().await.unwrap();
        }
        // 一条补单器放的、一条人手动放的，都在跑。
        mark_auto(&pool, &[ids[0]], 300).await.unwrap();
        for id in &ids {
            mark_submitted(&pool, *id, &SubmitReceipt::healthy("s", 8), 400)
                .await
                .unwrap();
        }
        assert_eq!(count_auto_running(&pool).await.unwrap(), 1);
    }

    // 撤销的核心不变量：整份写回，成片路径与扣费回执一并复原。
    //
    // 「把 stage 拨回 rev」是不够的 —— `requeue_for_run` 会连带清掉 video_path / credit_count，
    // 只改阶段会留下一条「待验收但没有片子」的行，比不给撤销更糟。
    #[tokio::test]
    async fn snapshot_then_restore_brings_back_media_and_receipt() {
        let (pool, _d) = test_pool().await;
        let id = seed_reviewable(&pool, 1).await;
        let snap = snapshot(&pool, id).await.unwrap().unwrap();
        assert_eq!(snap.stage, "rev");
        assert_eq!(snap.video_path.as_deref(), Some("/clips/1.mp4"));
        assert_eq!(snap.credit_count, Some(8));

        // 误按了「重跑」：成片引用与扣费回执被清空。
        assert!(requeue_for_run(&pool, id, 500).await.unwrap());
        let after = get(&pool, id).await.unwrap().unwrap();
        assert_eq!(after.stage, "ready");
        assert!(after.video_path.is_none());
        assert!(after.credit_count.is_none());

        assert!(restore(&pool, &snap, 600).await.unwrap());
        let back = get(&pool, id).await.unwrap().unwrap();
        assert_eq!(back.stage, "rev", "撤销须回到待验收");
        assert_eq!(
            back.video_path.as_deref(),
            Some("/clips/1.mp4"),
            "成片路径必须一并回来，否则撤销出来的是个空壳"
        );
        assert_eq!(back.credit_count, Some(8), "扣费回执必须一并回来");
        assert_eq!(back.submit_id.as_deref(), Some("sub-1"));
        assert_eq!(back.attempt, snap.attempt);
    }

    // 撤销一次「不通过」要回到待验收，且 reviewed_at 复原为空 —— 否则历程条上会留下
    // 一个「已判定」的时刻，而那一条其实还等着判。
    #[tokio::test]
    async fn restore_clears_reviewed_at_after_undoing_a_rejection() {
        let (pool, _d) = test_pool().await;
        let id = seed_reviewable(&pool, 1).await;
        let snap = snapshot(&pool, id).await.unwrap().unwrap();
        assert!(snap.reviewed_at.is_none());
        assert!(set_reviewed(&pool, id, "rej", 500).await.unwrap());
        assert_eq!(
            get(&pool, id).await.unwrap().unwrap().reviewed_at,
            Some(500)
        );

        assert!(restore(&pool, &snap, 600).await.unwrap());
        let back = get(&pool, id).await.unwrap().unwrap();
        assert_eq!(back.stage, "rev");
        assert!(back.reviewed_at.is_none(), "撤销后不得留下已判定的时刻");
    }

    // 「验收通过了却没落地」是这条链上唯一一处会无声断掉的地方：验收时的拷贝失败
    // **不回滚验收**，于是它在库里是个完全合法、界面上又完全看不见的状态。
    #[tokio::test]
    async fn undelivered_counts_passed_clips_without_export_path() {
        let (pool, _d) = test_pool().await;
        let id = seed_reviewable(&pool, 1).await;

        // 还没验收：不算 —— 它本来就还没到该交付的时候。
        assert_eq!(count_pass_undelivered(&pool).await.unwrap(), 0);

        assert!(set_reviewed(&pool, id, "pass", 500).await.unwrap());
        assert_eq!(
            count_pass_undelivered(&pool).await.unwrap(),
            1,
            "通过了却没有交付路径 —— 正是要催的那一条"
        );

        set_export_path(&pool, id, Some("/out/视频/甲组/BR310140_260727.mp4"))
            .await
            .unwrap();
        assert_eq!(count_pass_undelivered(&pool).await.unwrap(), 0);

        // 空白串等同于没有：交付路径是从文件系统回来的，别指望它只会是 NULL。
        set_export_path(&pool, id, Some("   ")).await.unwrap();
        assert_eq!(count_pass_undelivered(&pool).await.unwrap(), 1);
    }

    // 「你离开的这段时间」按 finished_at 切，且只把真出了片的算进出片数。
    // 用 updated_at 会把用户自己刚做的验收动作也算进「离开期间发生的事」。
    #[tokio::test]
    async fn away_digest_counts_by_finish_time_and_separates_phantoms() {
        let (pool, _d) = test_pool().await;
        let a = seed_reviewable(&pool, 1).await; // finished_at = 400，credit 8
        seed_work(&pool, 2).await;
        enqueue_one(&pool, 2).await;
        let b = list_by_stages(&pool, &["rewrite"]).await.unwrap()[0].id;
        mark_failed(&pool, b, "phantom", "无位次、无计费", 450)
            .await
            .unwrap();

        let all = away_digest(&pool, 0).await.unwrap();
        assert_eq!(all.finished, 1, "只有真出了片的算出片");
        assert_eq!(all.failed, 1);
        assert_eq!(all.phantom, 1, "幽灵单要单列 —— 它没扣费，处置与超时相反");
        assert_eq!(all.credits, 8);

        // 切在两者之间：只该看见后发生的那件事。
        let later = away_digest(&pool, 420).await.unwrap();
        assert_eq!(later.finished, 0);
        assert_eq!(later.failed, 1);
        assert_eq!(later.credits, 0);
        assert_ne!(a, b);
    }

    // 「N 分钟前收录」直接取 rewrote_at 的最大值，不另存一份「上次收录时间」。
    #[tokio::test]
    async fn last_rewrote_at_tracks_the_newest_ingest() {
        let (pool, _d) = test_pool().await;
        assert!(
            last_rewrote_at(&pool).await.unwrap().is_none(),
            "从没收录过"
        );
        seed_work(&pool, 1).await;
        enqueue_one(&pool, 1).await;
        let id = list_by_stages(&pool, &["rewrite"]).await.unwrap()[0].id;
        let mut tx = pool.begin().await.unwrap();
        apply_rewrite(&mut tx, id, "p", None, None, None, 1234)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        assert_eq!(last_rewrote_at(&pool).await.unwrap(), Some(1234));
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
        mark_submitted(&pool, id, &SubmitReceipt::healthy("sub-1", 8), 300)
            .await
            .unwrap();

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
        mark_submitted(&pool, id, &SubmitReceipt::healthy("sub-1", 8), 300)
            .await
            .unwrap();

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
        mark_submitted(&pool, id, &SubmitReceipt::healthy("sub-1", 8), 300)
            .await
            .unwrap();
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
            mark_submitted(&pool, id, &SubmitReceipt::healthy("s", 8), 300)
                .await
                .unwrap();
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
        mark_submitted(&pool, id, &SubmitReceipt::healthy("sub-1", 8), 300)
            .await
            .unwrap();

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
        mark_submitted(&pool, id, &SubmitReceipt::healthy("s", 8), 300)
            .await
            .unwrap();
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
        mark_submitted(&pool, paid, &SubmitReceipt::healthy("sub-paid", 8), 300)
            .await
            .unwrap();

        assert_eq!(recover_orphan_submits(&pool, 400).await.unwrap(), 1);
        assert_eq!(get(&pool, orphan).await.unwrap().unwrap().stage, "ready");
        let paid_row = get(&pool, paid).await.unwrap().unwrap();
        assert_eq!(paid_row.stage, "run", "已付费提交的任务必须留给轮询器认领");
        assert_eq!(paid_row.submit_id.as_deref(), Some("sub-paid"));
    }

    /// **同一条 ready 只能被认领一次**。
    ///
    /// 人点「确认提交」与常驻队列补单器跑在不同任务里，中间隔着整个 CLI 网络往返。
    /// 两边都读到同一条 `ready` 就会为同一张图下两次单、扣两份钱，而 `UNIQUE(work_id)`
    /// 拦不住它：自始至终只有一行，第二次提交只是覆盖 submit_id，
    /// 第一张片子当场变成认不出主人的孤儿。
    #[tokio::test]
    async fn a_ready_clip_can_only_be_claimed_once() {
        let (pool, _d) = test_pool().await;
        seed_work(&pool, 1).await;
        enqueue_one(&pool, 1).await;
        let id = list_by_stages(&pool, &["rewrite"]).await.unwrap()[0].id;
        let mut tx = pool.begin().await.unwrap();
        apply_rewrite(&mut tx, id, "p", None, None, None, 200)
            .await
            .unwrap();
        tx.commit().await.unwrap();

        let first = claim_ready(&pool, &[id], 300).await.unwrap();
        assert_eq!(first.len(), 1, "第一个认领者拿到它");
        assert_eq!(first[0].stage, "run", "认领即迁移，且发生在提交之前");
        let second = claim_ready(&pool, &[id], 301).await.unwrap();
        assert!(second.is_empty(), "第二个认领者必须空手而归");

        // 只读列表照旧看不到它（它已经不是 ready 了），预览也就不会把它算进去。
        assert!(list_ready(&pool, &[id]).await.unwrap().is_empty());
    }

    /// 认领到一半被杀 → 启动恢复认得出来（run 且无 submit_id）。
    /// 这正是「认领可以放在提交之前」的全部依据。
    #[tokio::test]
    async fn a_claim_without_a_submit_id_is_recovered_on_startup() {
        let (pool, _d) = test_pool().await;
        seed_work(&pool, 1).await;
        enqueue_one(&pool, 1).await;
        let id = list_by_stages(&pool, &["rewrite"]).await.unwrap()[0].id;
        let mut tx = pool.begin().await.unwrap();
        apply_rewrite(&mut tx, id, "p", None, None, None, 200)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        claim_ready(&pool, &[id], 300).await.unwrap();

        assert_eq!(recover_orphan_submits(&pool, 400).await.unwrap(), 1);
        assert_eq!(get(&pool, id).await.unwrap().unwrap().stage, "ready");
    }

    /// 放回认领只对「确定没花钱」的条目生效。
    ///
    /// 有 submit_id 就意味着额度已经扣了，把它放回 ready 等于让它被重新提交一次 ——
    /// 花两份钱买同一条视频，正是这套顺序要防的那件事。
    #[tokio::test]
    async fn releasing_a_claim_never_touches_a_paid_submission() {
        let (pool, _d) = test_pool().await;
        seed_work(&pool, 1).await;
        enqueue_one(&pool, 1).await;
        let id = list_by_stages(&pool, &["rewrite"]).await.unwrap()[0].id;
        let mut tx = pool.begin().await.unwrap();
        apply_rewrite(&mut tx, id, "p", None, None, None, 200)
            .await
            .unwrap();
        tx.commit().await.unwrap();

        claim_ready(&pool, &[id], 300).await.unwrap();
        release_claim(&pool, id, 310).await.unwrap();
        assert_eq!(
            get(&pool, id).await.unwrap().unwrap().stage,
            "ready",
            "没提交出去的认领要放回去，不能卡在 run 里等下次启动恢复"
        );

        claim_ready(&pool, &[id], 320).await.unwrap();
        mark_submitted(&pool, id, &SubmitReceipt::healthy("sub-paid", 8), 330)
            .await
            .unwrap();
        release_claim(&pool, id, 340).await.unwrap();
        let row = get(&pool, id).await.unwrap().unwrap();
        assert_eq!(row.stage, "run", "已经扣过费的条目不得被放回待提交");
        assert_eq!(row.submit_id.as_deref(), Some("sub-paid"));
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
        mark_submitted(&pool, id, &SubmitReceipt::healthy("sub-1", 8), 500)
            .await
            .unwrap();
        let before = get(&pool, id).await.unwrap().unwrap().updated_at;

        mark_polled(&pool, id, "queue", Some(3), 900).await.unwrap();
        let row = get(&pool, id).await.unwrap().unwrap();
        assert_eq!(row.gen_status.as_deref(), Some("queue"));
        assert_eq!(row.queue_idx, Some(3));
        assert_eq!(row.polled_at, Some(900));
        assert_eq!(row.updated_at, before, "轮询快照不得刷新业务变更时间");
    }

    /// 队列位次一旦问到就**只增不抹**：它是幽灵判定的两个信号之一。
    ///
    /// 任务离开排队、开始生成之后回体里就不再有 `queue_info`，无条件写回等于在那一刻
    /// 亲手删掉「这条确实进过队列」的证据 —— 而下一轮判定读的正是它。
    #[tokio::test]
    async fn polling_never_erases_a_queue_position_it_already_knew() {
        let (pool, _d) = test_pool().await;
        seed_work(&pool, 1).await;
        enqueue_one(&pool, 1).await;
        let id = list_by_stages(&pool, &["rewrite"]).await.unwrap()[0].id;
        mark_submitted(&pool, id, &SubmitReceipt::healthy("sub-1", 8), 500)
            .await
            .unwrap();

        mark_polled(&pool, id, "queue", Some(4485), 900)
            .await
            .unwrap();
        // 开始生成了 —— 这一份回体不再带 queue_info。
        mark_polled(&pool, id, "generating", None, 1000)
            .await
            .unwrap();
        let row = get(&pool, id).await.unwrap().unwrap();
        assert_eq!(row.queue_idx, Some(4485), "已问到的位次不得被抹成空");
        assert_eq!(
            row.gen_status.as_deref(),
            Some("generating"),
            "状态照常更新"
        );
        assert_eq!(row.polled_at, Some(1000));
    }

    /// 出片那一刻不得把这条的账抹掉。
    ///
    /// 钱在提交那一刻就扣了，扫描一路上也可能早就问到过计费；而出片那份回体未必再带
    /// `credit_count`。无条件写回 = 成片入库的同时把「这一批花了多少」变成永远答不准。
    #[tokio::test]
    async fn finishing_never_erases_the_bill() {
        let (pool, _d) = test_pool().await;
        seed_work(&pool, 1).await;
        enqueue_one(&pool, 1).await;
        let id = list_by_stages(&pool, &["rewrite"]).await.unwrap()[0].id;
        mark_submitted(&pool, id, &SubmitReceipt::healthy("sub-1", 8), 500)
            .await
            .unwrap();
        // 扫描路上问到了计费与计费型号。
        mark_swept(&pool, id, "generating", Some(8), Some("dreamina_x"), 600)
            .await
            .unwrap();
        // 出片回体只有视频元数据，没有 commerce_info。
        mark_ready_for_review(
            &pool,
            id,
            "/clips/1.mp4",
            None,
            Some(720),
            Some(1280),
            None,
            Some(4.0),
            None,
            None,
            700,
        )
        .await
        .unwrap();
        let row = get(&pool, id).await.unwrap().unwrap();
        assert_eq!(row.stage, "rev");
        assert_eq!(row.credit_count, Some(8), "扣费回执不得在出片时被抹掉");
        assert_eq!(row.benefit_type.as_deref(), Some("dreamina_x"));
    }

    /// 徽章里那个幽灵计数（SQL）与逐行判据（Rust）必须给同一个答案。
    ///
    /// 它们是同一条规则的两种写法 —— 一个数数、一个判行，改了一处忘了另一处，
    /// 表现就是「徽章说有 3 条待办，点进去一条高亮的都没有」。
    #[tokio::test]
    async fn phantom_count_agrees_with_the_row_level_predicate() {
        use crate::v2v::runner::{clip_looks_phantom, PHANTOM_GRACE_SECS};
        let (pool, _d) = test_pool().await;
        let now = 100_000i64;
        let submitted = now - PHANTOM_GRACE_SECS - 60;
        for w in 1..=4 {
            seed_work(&pool, w).await;
            enqueue_one(&pool, w).await;
        }
        let ids: Vec<i64> = list_by_stages(&pool, &["rewrite"])
            .await
            .unwrap()
            .iter()
            .map(|c| c.id)
            .collect();
        // 1：幽灵（提交回执什么都没给）。2：提交就带计费。3：扫描路上问到过位次。
        // 4：还在宽限期内。
        mark_submitted(&pool, ids[0], &SubmitReceipt::bare("sub-1"), submitted)
            .await
            .unwrap();
        mark_submitted(
            &pool,
            ids[1],
            &SubmitReceipt::healthy("sub-2", 8),
            submitted,
        )
        .await
        .unwrap();
        mark_submitted(&pool, ids[2], &SubmitReceipt::bare("sub-3"), submitted)
            .await
            .unwrap();
        mark_polled(&pool, ids[2], "queue", Some(4485), submitted + 10)
            .await
            .unwrap();
        mark_submitted(&pool, ids[3], &SubmitReceipt::bare("sub-4"), now - 60)
            .await
            .unwrap();

        let n = count_phantom_suspects(&pool, now - PHANTOM_GRACE_SECS)
            .await
            .unwrap();
        let rows = list_by_stages(&pool, &["run"]).await.unwrap();
        let by_row = rows.iter().filter(|c| clip_looks_phantom(c, now)).count() as i64;
        assert_eq!(n, 1, "只有 sub-1 一条没有任何计费证据且过了宽限期");
        assert_eq!(n, by_row, "SQL 计数与逐行判据必须一致");
    }

    // 重跑/退回改写必须把上一轮的即梦状态一起清掉，否则一条刚退回待提交的条目
    // 会带着上一轮的 `success` 显示，看板上就成了「待提交但已成功」。
    #[tokio::test]
    async fn requeue_clears_stale_provider_status() {
        let (pool, _d) = test_pool().await;
        seed_work(&pool, 1).await;
        enqueue_one(&pool, 1).await;
        let id = list_by_stages(&pool, &["rewrite"]).await.unwrap()[0].id;
        mark_submitted(&pool, id, &SubmitReceipt::healthy("sub-1", 8), 500)
            .await
            .unwrap();
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
        mark_submitted(&pool, submitted, &SubmitReceipt::healthy("sub-1", 8), 500)
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
        mark_submitted(
            &pool,
            ids[0],
            &SubmitReceipt::healthy("s0", 8),
            now - 3 * 3600,
        )
        .await
        .unwrap();
        mark_submitted(&pool, ids[1], &SubmitReceipt::healthy("s1", 8), now - 600)
            .await
            .unwrap();
        mark_submitted(
            &pool,
            ids[2],
            &SubmitReceipt::healthy("s2", 8),
            now - 4 * 3600,
        )
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
        mark_submitted(&pool, id, &SubmitReceipt::healthy("s", 8), 1_000)
            .await
            .unwrap();
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
        mark_submitted(&pool, id, &SubmitReceipt::healthy("sub-paid", 8), 500)
            .await
            .unwrap();
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
        assert_eq!(
            row.first_submitted_at,
            Some(500),
            "首次提交时刻不得被重置抹掉 —— 抹掉了「这条到底等了多久」就再也答不出来"
        );
    }

    // 幽灵单（从未入队、从未计费）**不给「继续等待」**：再等一万年也不会出片，
    // 而那个按钮的全部意义是「钱已经花了，别重复花」。它该走的是重跑。
    #[tokio::test]
    async fn phantom_clip_is_not_resumable() {
        let (pool, _d) = test_pool().await;
        seed_work(&pool, 1).await;
        enqueue_one(&pool, 1).await;
        let id = list_by_stages(&pool, &["rewrite"]).await.unwrap()[0].id;
        mark_submitted(&pool, id, &SubmitReceipt::bare("sub-ghost"), 500)
            .await
            .unwrap();
        mark_failed(&pool, id, "phantom", "即梦接了单但未入队", 3_200)
            .await
            .unwrap();

        assert!(!resume_timed_out(&pool, id, 9_000).await.unwrap());
        assert_eq!(get(&pool, id).await.unwrap().unwrap().stage, "fail");
    }

    // 提交回执落库：`submit_credit` 为空即「即梦没给计费回执」，
    // 界面据此才敢对用户说「这条没扣费，重跑不会重复扣」。
    #[tokio::test]
    async fn submit_receipt_is_persisted() {
        let (pool, _d) = test_pool().await;
        seed_work(&pool, 1).await;
        enqueue_one(&pool, 1).await;
        let id = list_by_stages(&pool, &["rewrite"]).await.unwrap()[0].id;

        mark_submitted(&pool, id, &SubmitReceipt::healthy("sub-ok", 8), 500)
            .await
            .unwrap();
        let row = get(&pool, id).await.unwrap().unwrap();
        assert_eq!(row.submit_credit, Some(8));
        assert_eq!(row.submit_status.as_deref(), Some("querying"));
        assert_eq!(row.first_submitted_at, Some(500));

        // 重投换新单 → 首次提交时刻跟着换（这才是新一次等待的起点）。
        mark_submitted(&pool, id, &SubmitReceipt::bare("sub-ghost"), 7_000)
            .await
            .unwrap();
        let row = get(&pool, id).await.unwrap().unwrap();
        assert_eq!(row.submit_credit, None, "没有计费回执就该是空");
        assert_eq!(row.first_submitted_at, Some(7_000));
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
            mark_submitted(&pool, id, &SubmitReceipt::healthy("s", 8), at)
                .await
                .unwrap();
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
