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
    /// 人已经点过「确认提交」、但被在跑上限挡在本地的时刻（0028）。
    ///
    /// 它与 `stage='ready'` 并存：这一条的一切（改参数、退回改写、删除）都还成立，
    /// 差别只是「有没有人放过行」。轮询循环按它先进先出地往即梦补位。
    pub submit_queued_at: Option<i64>,
    /// 验收通过后交付到 `{交付目录}/` 的那份拷贝（0027）。有它才答得出「片子在哪」。
    ///
    /// 0025 的 `asset_pack_id` 列**不再读取**：v0.22.0 起成片不入资产库
    /// （它们是 B-roll 素材，不适合直接发布）。迁移 forward-only，列留在表里。
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
        c.auto_submitted, c.submit_queued_at,
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

pub async fn get<'e, E>(ex: E, id: i64) -> Result<Option<ClipRow>, sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let sql = format!("{SELECT} WHERE c.id = ?1");
    sqlx::query_as::<_, ClipRow>(&sql)
        .bind(id)
        .fetch_optional(ex)
        .await
}

/// 各阶段计数（看板列头 + 侧栏徽章）。
pub async fn stage_counts(pool: &SqlitePool) -> Result<Vec<(String, i64)>, sqlx::Error> {
    sqlx::query_as::<_, (String, i64)>("SELECT stage, COUNT(*) FROM v2v_clips GROUP BY stage")
        .fetch_all(pool)
        .await
}

/// 写入组内公共前后缀剥离结果（工单物化时按组批量算一次）。
/// 落可变部分（物化工单时顺手算出来的）。
///
/// **值没变就一个字都不写**。工单物化是自动的：队列一变就重写一遍，而 `updated_at`
/// 是业务变更时间，看板照它显示「几分钟前」—— 每次物化都刷一遍，等于让整块看板
/// 永远显示「刚刚」（同 `mark_polled` 那条注释里的理由）。
/// 这也是「`v2v_handoff_status` 可以随处调用」的前提：它会顺手重写工单。
pub async fn set_variable_part(
    pool: &SqlitePool,
    id: i64,
    variable_part: &str,
    now: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE v2v_clips SET variable_part = ?2, updated_at = ?3
          WHERE id = ?1 AND variable_part <> ?2",
    )
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
/// 而进程若恰好被杀在远端接单与写 submit_id 之间，我们无法知道有没有扣费。
/// `recover_orphan_submits` 会把「run 但没有 submit_id」隔离到失败态，绝不自动重提。
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
    let sql = format!("{SELECT} WHERE c.id IN ({holes})");
    let mut q = sqlx::query_as::<_, ClipRow>(&sql);
    for i in &claimed {
        q = q.bind(*i);
    }
    let rows = q.fetch_all(pool).await?;
    let mut by_id: std::collections::HashMap<i64, ClipRow> =
        rows.into_iter().map(|row| (row.id, row)).collect();
    // SQL 的 IN 查询没有顺序保证；必须恢复调用方挑出的 FIFO 顺序。否则撞墙后停批时，
    // 哪条先试、哪几条退回会偷偷变成主键顺序，候补队列就不再是 FIFO。
    Ok(claimed
        .into_iter()
        .filter_map(|id| by_id.remove(&id))
        .collect())
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

/// 批量放回一批**还没调用过远端**的认领。
///
/// 并发探测遇到明确未计费拒收时，把相应 `run` 认领一次 UPDATE 退回。人工放行的保留
/// 原 `submit_queued_at`；常驻补单原本没有排队时刻，就以本次拒收时刻入队。两种来源
/// 此后都由同一个 FIFO 队列在有空位时自动候补，不会变成无人再碰的 ready 条目。
pub async fn release_claims(pool: &SqlitePool, ids: &[i64], now: i64) -> Result<i64, sqlx::Error> {
    if ids.is_empty() {
        return Ok(0);
    }
    let holes = vec!["?"; ids.len()].join(",");
    let sql = format!(
        "UPDATE v2v_clips
            SET stage='ready', submit_queued_at=COALESCE(submit_queued_at, ?), updated_at=?
          WHERE stage='run' AND submit_id IS NULL AND id IN ({holes})"
    );
    let mut q = sqlx::query(&sql).bind(now).bind(now);
    for id in ids {
        q = q.bind(*id);
    }
    Ok(q.execute(pool).await?.rows_affected() as i64)
}

/// 只改生成参数（批量覆盖用），不动提示词也不动阶段。
///
/// 与 [`update_ready`] 分开是因为它们回答不同的问题：那条是「人改完了这一条」（顺带把
/// 它推进待提交），这条是「这十条都换成 seedance2.0_vip」——后者不该把还在待改写的条目
/// 悄悄推进下一列，否则 skill 还没写提示词，它就已经躺在待提交列里等着被提交了。
pub async fn set_params<'e, E>(
    ex: E,
    ids: &[i64],
    model_version: Option<&str>,
    duration: Option<i64>,
    video_resolution: Option<&str>,
    now: i64,
) -> Result<i64, sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
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
    Ok(q.execute(ex).await?.rows_affected() as i64)
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
///
/// **计费也在这里写回**（同 [`mark_swept`]，同样 `COALESCE`、只增不抹）。原先只写状态
/// 与位次，于是逐条 `query_result` 这条路径上问到的 `credit_count` 只在内存里当一次
/// 幽灵证据就扔了：非 VIP 十分钟才扫一轮，一条排队几小时的单可以被逐条问到几十次、
/// 每次都带着计费回执，而库里那一列直到出片才第一次落值 —— 「在跑的这些已经花了多少」
/// 在最需要它的那几个小时里恒为 0，重跑护栏读的五处证据也白少一处。
///
/// `expect_submit`：结算所有权谓词，见 [`OWNED`]。
#[allow(clippy::too_many_arguments)] // 一份回体里能落库的字段就这些，拆结构体只会多一层包装
pub async fn mark_polled(
    pool: &SqlitePool,
    id: i64,
    expect_submit: &str,
    gen_status: &str,
    queue_idx: Option<i64>,
    credit_count: Option<i64>,
    benefit_type: Option<&str>,
    now: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(&format!(
        "UPDATE v2v_clips
            SET gen_status=?2, queue_idx=COALESCE(?3, queue_idx), polled_at=?4,
                credit_count=COALESCE(credit_count, ?5),
                benefit_type=COALESCE(benefit_type, ?6)
          WHERE id=?1 AND {OWNED}?7"
    ))
    .bind(id)
    .bind(gen_status)
    .bind(queue_idx)
    .bind(now)
    .bind(credit_count)
    .bind(benefit_type)
    .bind(expect_submit)
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
    expect_submit: &str,
    gen_status: &str,
    credit_count: Option<i64>,
    benefit_type: Option<&str>,
    now: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(&format!(
        "UPDATE v2v_clips
            SET gen_status=?2,
                credit_count=COALESCE(credit_count, ?3),
                benefit_type=COALESCE(benefit_type, ?4),
                polled_at=?5
          WHERE id=?1 AND {OWNED}?6"
    ))
    .bind(id)
    .bind(gen_status)
    .bind(credit_count)
    .bind(benefit_type)
    .bind(now)
    .bind(expect_submit)
    .execute(pool)
    .await?;
    Ok(())
}

/// 只记「问过了」，不动状态（查询失败时用）。
///
/// 失败也要落 `polled_at`，否则退避完全失效：CLI 一旦不可用，19 条会在每个 tick 上
/// 各起一个必然失败的进程 —— 正是最该省着点的那种时候反而最费。
pub async fn mark_poll_attempt(
    pool: &SqlitePool,
    id: i64,
    expect_submit: &str,
    now: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(&format!(
        "UPDATE v2v_clips SET polled_at=?2 WHERE id=?1 AND {OWNED}?3"
    ))
    .bind(id)
    .bind(now)
    .bind(expect_submit)
    .execute(pool)
    .await?;
    Ok(())
}

/// ready → run（记下 submit_id 与整份提交回执）。
///
/// `first_submitted_at` 在这里**无条件**更新：走到这一步就是一个新的 submit_id，
/// 从它开始重新计时才对。会被「继续等待」重置的是 `submitted_at`（那条路径不换单，
/// 故不动 `first_submitted_at`）—— 两列的分工全部体现在这个差别上。
#[cfg(test)]
pub async fn mark_submitted(
    pool: &SqlitePool,
    id: i64,
    receipt: &crate::v2v::dreamina::SubmitReceipt,
    now: i64,
) -> Result<(), sqlx::Error> {
    mark_submitted_inner(pool, id, receipt, None, now).await
}

/// 生产提交路径：除回执外一并钉住这一次实际使用的模型通道。
///
/// 条目在提交前可以一直“跟随默认”；提交后若仍保留空型号，用户随后修改默认设置会让
/// 一条已经在 A 通道运行的任务看起来像在 B 通道。消费事件触发器也必须在同一条 UPDATE
/// 中看到真实通道，才能把这笔账记到正确的分类下。
pub async fn mark_submitted_on(
    pool: &SqlitePool,
    id: i64,
    receipt: &crate::v2v::dreamina::SubmitReceipt,
    channel: &str,
    now: i64,
) -> Result<(), sqlx::Error> {
    mark_submitted_inner(pool, id, receipt, Some(channel), now).await
}

async fn mark_submitted_inner(
    pool: &SqlitePool,
    id: i64,
    receipt: &crate::v2v::dreamina::SubmitReceipt,
    channel: Option<&str>,
    now: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE v2v_clips SET stage='run', submit_id=?2, submitted_at=?3, updated_at=?3,
             first_submitted_at=?3, submit_credit=?4, submit_status=?5,
             model_version=COALESCE(?6, model_version),
             attempt = attempt + 1, error_type=NULL, error_message=NULL,
             gen_status=NULL, queue_idx=NULL, polled_at=NULL
         WHERE id=?1",
    )
    .bind(id)
    .bind(&receipt.submit_id)
    .bind(now)
    .bind(receipt.credit_count)
    .bind(&receipt.gen_status)
    .bind(channel)
    .execute(pool)
    .await?;
    Ok(())
}

/// 轮询器待办：全部 run 态且有 submit_id 的条目。
pub async fn list_running(pool: &SqlitePool) -> Result<Vec<ClipRow>, sqlx::Error> {
    let sql = format!("{SELECT} WHERE c.stage='run' AND c.submit_id IS NOT NULL ORDER BY c.id");
    sqlx::query_as::<_, ClipRow>(&sql).fetch_all(pool).await
}

/// [`list_running`] 的条数。手动刷新要先告诉界面「这一轮要问几条」，而那时把整行捞
/// 出来只为了数一下是白费 —— 谓词必须与 `list_running` 一字不差，否则进度条的分母
/// 会和实际问的条数对不上。
pub async fn count_running(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
    let (n,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM v2v_clips WHERE stage='run' AND submit_id IS NOT NULL",
    )
    .fetch_one(pool)
    .await?;
    Ok(n)
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

/// 轮询结算的**所有权谓词**：这一行现在还是当初那一单的吗。
///
/// ## 为什么每一处结算写入都要带上它
///
/// 查询一条即梦任务要走网络（实测手动刷新逐条问，一轮可以跑几十秒到几分钟），而这段
/// 时间里人是可以动这一行的 —— 重跑、换通道、放弃改投，都会把 `submit_id` 换成另一单
/// B。此时 A 的回体才姗姗来迟，若结算只按 `id` 写，就会拿 A 的结果去改 B 的行：B 被
/// 写成 `rev`/`fail`，而 `list_running`（要求 `stage='run' AND submit_id IS NOT NULL`）
/// 从此再也捞不到它 —— **一条已经扣过费、即梦那边还在跑的任务当场失联**，且界面上
/// 看不出任何异常（那一行摆着 A 的成片，看起来一切正常）。
///
/// 与 `claim_ready` / `mark_running` 同一种手法（见 CLAUDE.md「并发认领」）：代价都是钱，
/// 所以判据不能是「读一下再写」，必须压进同一条 UPDATE 的 WHERE 里。
const OWNED: &str = "stage='run' AND submit_id=";

/// 成片落盘：run → rev（待验收）。
///
/// `expect_submit`：结算所有权谓词，见 [`OWNED`]。返回 `false` = 这一行已经不是那一单
/// 的了，调用方**必须**把刚落盘的文件收掉（那是 A 的成片，而这一行现在属于 B）。
///
/// `credit_count` / `benefit_type` 走 `COALESCE`（同 [`mark_swept`]）：出片那一份回体
/// 未必再带计费字段，而钱在提交那一刻就扣了、扫描一路上也可能早就问到过。无条件写回
/// 等于**在出片那一刻把这条的账抹掉** —— 「这一批花了多少」从此再也答不准。
#[allow(clippy::too_many_arguments)] // 视频元数据是一整组，拆结构体只会多一层无信息的包装
pub async fn mark_ready_for_review(
    pool: &SqlitePool,
    id: i64,
    expect_submit: &str,
    video_path: &str,
    poster_path: Option<&str>,
    width: Option<i64>,
    height: Option<i64>,
    fps: Option<f64>,
    duration_sec: Option<f64>,
    credit_count: Option<i64>,
    benefit_type: Option<&str>,
    now: i64,
) -> Result<bool, sqlx::Error> {
    let res = sqlx::query(&format!(
        "UPDATE v2v_clips SET stage='rev', video_path=?2, poster_path=?3, width=?4, height=?5,
             fps=?6, duration_sec=?7,
             credit_count=COALESCE(?8, credit_count),
             benefit_type=COALESCE(?10, benefit_type),
             finished_at=?9,
             updated_at=?9, error_type=NULL, error_message=NULL
         WHERE id=?1 AND {OWNED}?11"
    ))
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
    .bind(expect_submit)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// 失败：→ fail，留错误分类与原文供人判断是重跑还是退回改写。
///
/// `expect_submit` 是 [`OWNED`] 谓词，但这里必须可选：**提交失败**那条路径（`submit_batch`
/// 的 `Err` 分支）此时根本还没有 submit_id —— 对方说没做成，所以没花钱。带谓词的是轮询
/// 结算那几处，它们判的是一条已经花过钱的单。
pub async fn mark_failed(
    pool: &SqlitePool,
    id: i64,
    expect_submit: Option<&str>,
    error_type: &str,
    error_message: &str,
    now: i64,
) -> Result<bool, sqlx::Error> {
    let res = sqlx::query(&format!(
        "UPDATE v2v_clips SET stage='fail', error_type=?2, error_message=?3,
             finished_at=?4, updated_at=?4
         WHERE id=?1 AND (?5 IS NULL OR ({OWNED}?5))"
    ))
    .bind(id)
    .bind(error_type)
    .bind(error_message)
    .bind(now)
    .bind(expect_submit)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() > 0)
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
/// 成片文件就位后把路径挪到最终名上（见 `runner::settle` 的两步落盘）。
///
/// 单独一条 UPDATE 而不是并进 [`mark_ready_for_review`]：那一条要带所有权 CAS，而 CAS
/// 通过**之前**最终文件名还不能碰（它按主键命名，那个位置可能属于另一单）。
pub async fn set_video_path(pool: &SqlitePool, id: i64, path: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE v2v_clips SET video_path = ?2 WHERE id = ?1")
        .bind(id)
        .bind(path)
        .execute(pool)
        .await?;
    Ok(())
}

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

/// 无输出恢复/改投：**fail/run** → ready，清掉上一份无成片提交引用。
///
/// `run` 只供 Rust 已判定未计费的幽灵单恢复，或带费用确认的改通道流程；待验收、拒绝和
/// 已通过任务在 SQL 白名单上进不来，不能用同一提示词直接再次提交。
///
/// 但对一条即梦**已经收过钱**的单子，这条 SQL 会把 `submit_id` 与 `credit_count` 一起
/// 清掉 —— 此后 `list_running`（要求 `submit_id IS NOT NULL`）再也认不出它，那条已付费
/// 的视频永远取不回来，下次提交是第二份钱。所以调用方**必须**先用
/// `runner::Evidence::billed` 判一次，并且要么拦住、要么记账（`commands::v2v` 两处入口
/// 都这么做了）。相邻的每一条路径（`release_claim` / `recover_orphan_submits` /
/// `resume_timed_out` / `requeue_after_reject`）都各自带着 `submit_id IS NULL` 或
/// `!billed()` 的闸，唯独这条把判断交给了调用方。
///
/// `expect_submit` 就是那道闸的另一半：调用方读到哪一单、就对哪一单动手。
/// `Some(sid)` = 这一行的 `submit_id` 必须仍是 `sid`，否则一行不改（人在确认框上犹豫的
/// 那几秒里，本地队列可能已经把它提交出去了，或者轮询已经把它结算掉了 —— 那时丢弃的
/// 就不再是人看过的那一单）。`None` = 不设提交单约束，只供无输出失败恢复使用。
pub async fn requeue_for_run<'e, E>(
    ex: E,
    id: i64,
    expect_submit: Option<&str>,
    now: i64,
) -> Result<bool, sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let res = sqlx::query(
        "UPDATE v2v_clips
            SET stage='ready', submit_id=NULL, video_path=NULL, poster_path=NULL,
                width=NULL, height=NULL, fps=NULL, duration_sec=NULL, credit_count=NULL,
                error_type=NULL, error_message=NULL, finished_at=NULL, reviewed_at=NULL,
                gen_status=NULL, queue_idx=NULL, polled_at=NULL, updated_at=?2
          WHERE id=?1 AND stage IN ('fail','run')
            AND (?3 IS NULL OR submit_id IS ?3)",
    )
    .bind(id)
    .bind(now)
    .bind(expect_submit)
    .execute(ex)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// 继续等待：把判了超时、但**提交单还在**的条目放回 run，让轮询器重新认领。
///
/// 这条路径的存在理由是钱：超时判定只是我们这边不等了，即梦那边任务还在跑、额度已经扣了。
/// 而 [`requeue_for_run`] 会清掉 `submit_id` —— 那意味着恢复后再提交、再花一份钱。
/// 实测 19 条在 45 分钟被判超时时，`dreamina list_task` 里它们全都还是 `querying`。
///
/// 重置 `submitted_at` 是必须的：不重置的话下一轮立刻又判超时，按钮点了等于没点。
/// 但**不动 `first_submitted_at`** —— 重置的代价原先是原始提交时刻被永久覆盖，
/// 事故当天看板因此显示「最久已等 10 小时 54 分」，而那只是从按下这个按钮算起的。
///
/// 只放 `timeout`：`phantom` 那类没进队列、没扣费，「继续等待」对它毫无意义
/// （再等一万年也不会出片），该走的是无输出恢复。
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

/// 退回改写：未提交或没有可用成片的终态 → rewrite。
///
/// `run` 明确不在射程内：那一格可能已经扣费，清 submit_id 会把仍在即梦手上的任务
/// 变成孤儿。正在运行的改投必须走带费用确认的换通道命令。
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
          WHERE id=?1 AND stage IN ('rewrite','ready','rev','rej','fail')",
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

/// 不可变消费事件按通道的汇总。`since=None` 表示全部历史。
#[derive(Debug, Clone, FromRow)]
pub struct CreditChannelRow {
    pub channel_key: String,
    pub spent_total: i64,
    pub spent_pass: i64,
    pub spent_rej: i64,
    pub spent_pending: i64,
    pub spent_failed_abandoned: i64,
    pub passed: i64,
    pub reviewed: i64,
    pub events: i64,
}

pub async fn credit_events_by_channel(
    pool: &SqlitePool,
    since: Option<i64>,
) -> Result<Vec<CreditChannelRow>, sqlx::Error> {
    sqlx::query_as::<_, CreditChannelRow>(
        "WITH submissions AS (
           SELECT s.submit_id, s.channel_key, s.charged_at,
                  COALESCE(
                    (SELECT c.credits FROM v2v_credit_events c
                      WHERE c.submit_id=s.submit_id AND c.event_type='charge'
                      ORDER BY c.id DESC LIMIT 1),
                    s.credits
                  ) AS credits,
                  COALESCE(
                    (SELECT o.event_type FROM v2v_credit_events o
                      WHERE o.submit_id=s.submit_id
                        AND o.event_type IN ('pending','pass','rej','failed','abandoned')
                      ORDER BY o.id DESC LIMIT 1),
                    'pending'
                  ) AS outcome
             FROM v2v_credit_events s
            WHERE s.event_type='submit'
         )
         SELECT channel_key,
                COALESCE(SUM(credits),0) AS spent_total,
                COALESCE(SUM(CASE WHEN outcome='pass' THEN credits ELSE 0 END),0) AS spent_pass,
                COALESCE(SUM(CASE WHEN outcome='rej' THEN credits ELSE 0 END),0) AS spent_rej,
                COALESCE(SUM(CASE WHEN outcome='pending' THEN credits ELSE 0 END),0)
                  AS spent_pending,
                COALESCE(SUM(CASE WHEN outcome IN ('failed','abandoned')
                                  THEN credits ELSE 0 END),0) AS spent_failed_abandoned,
                COALESCE(SUM(CASE WHEN outcome='pass' THEN 1 ELSE 0 END),0) AS passed,
                COALESCE(SUM(CASE WHEN outcome IN ('pass','rej') THEN 1 ELSE 0 END),0)
                  AS reviewed,
                COUNT(*) AS events
           FROM submissions
          WHERE credits IS NOT NULL AND (?1 IS NULL OR charged_at >= ?1)
          GROUP BY channel_key
          ORDER BY spent_total DESC, channel_key",
    )
    .bind(since)
    .fetch_all(pool)
    .await
}

/// 消费趋势点。近 7/30 天按日，全部历史按月。
#[derive(Debug, Clone, FromRow)]
pub struct CreditTrendRow {
    pub bucket: String,
    pub spent: i64,
}

pub async fn credit_event_trend(
    pool: &SqlitePool,
    since: Option<i64>,
    monthly: bool,
) -> Result<Vec<CreditTrendRow>, sqlx::Error> {
    let bucket = if monthly { "%Y-%m" } else { "%Y-%m-%d" };
    sqlx::query_as::<_, CreditTrendRow>(
        "WITH submissions AS (
           SELECT s.charged_at,
                  COALESCE(
                    (SELECT c.credits FROM v2v_credit_events c
                      WHERE c.submit_id=s.submit_id AND c.event_type='charge'
                      ORDER BY c.id DESC LIMIT 1),
                    s.credits
                  ) AS credits
             FROM v2v_credit_events s
            WHERE s.event_type='submit'
         )
         SELECT strftime(?2, charged_at, 'unixepoch', 'localtime') AS bucket,
                COALESCE(SUM(credits),0) AS spent
           FROM submissions
          WHERE credits IS NOT NULL AND (?1 IS NULL OR charged_at >= ?1)
          GROUP BY bucket
          ORDER BY bucket",
    )
    .bind(since)
    .bind(bucket)
    .fetch_all(pool)
    .await
}

// ─────────────────────── 排队位次轨迹（0029）───────────────────────

/// 一个排队位次采样点。
#[derive(Debug, Clone, Copy, PartialEq, Eq, FromRow)]
pub struct QueueSample {
    pub clip_id: i64,
    pub at: i64,
    pub queue_idx: i64,
    pub queue_length: Option<i64>,
}

/// 记一个排队位次采样点。
///
/// `INSERT OR IGNORE`：同一秒的重复写（补扫与定时扫在边界上撞到一起）是 no-op，
/// 而不是一个要往上冒泡的错误 —— 这是诊断数据，写丢一个点不该影响轮询。
pub async fn record_queue_sample(
    pool: &SqlitePool,
    clip_id: i64,
    at: i64,
    queue_idx: i64,
    queue_length: Option<i64>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT OR IGNORE INTO v2v_queue_samples (clip_id, at, queue_idx, queue_length)
         VALUES (?1, ?2, ?3, ?4)",
    )
    .bind(clip_id)
    .bind(at)
    .bind(queue_idx)
    .bind(queue_length)
    .execute(pool)
    .await?;
    Ok(())
}

/// 某条 clip 的全部采样，按时间正序（详情栏那条曲线）。
pub async fn queue_samples_of(
    pool: &SqlitePool,
    clip_id: i64,
) -> Result<Vec<QueueSample>, sqlx::Error> {
    sqlx::query_as::<_, QueueSample>(
        "SELECT clip_id, at, queue_idx, queue_length FROM v2v_queue_samples
          WHERE clip_id = ?1 ORDER BY at",
    )
    .bind(clip_id)
    .fetch_all(pool)
    .await
}

/// `since` 之后的全部采样，按 (clip, 时间) 正序。
///
/// 全局速度是**逐条算斜率、再跨条聚合**的，所以这里不在 SQL 里做窗口函数：
/// 聚合规则（丢掉位次回升的段、按小时归桶、取中位数）是业务判断，属于纯函数那一层，
/// 留在 SQL 里既测不动也读不懂。
pub async fn queue_samples_since(
    pool: &SqlitePool,
    since: i64,
) -> Result<Vec<QueueSample>, sqlx::Error> {
    sqlx::query_as::<_, QueueSample>(
        "SELECT clip_id, at, queue_idx, queue_length FROM v2v_queue_samples
          WHERE at >= ?1 ORDER BY clip_id, at",
    )
    .bind(since)
    .fetch_all(pool)
    .await
}

/// 裁掉过期采样，回删除条数。
///
/// 保留期是**观测窗口**，不是业务真相 —— 排产要看的是「最近这段时间队列多快」，
/// 半年前那一周对今晚提交与否毫无参考价值，而它会一直占着索引。
pub async fn prune_queue_samples(pool: &SqlitePool, before: i64) -> Result<u64, sqlx::Error> {
    let r = sqlx::query("DELETE FROM v2v_queue_samples WHERE at < ?1")
        .bind(before)
        .execute(pool)
        .await?;
    Ok(r.rows_affected())
}

// ─────────────────────── 每日额度台账（0030）───────────────────────

/// 一天一条的余额快照。
#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct CreditDay {
    pub day: String,
    pub at: i64,
    pub balance: i64,
    pub spent_since_prev: i64,
    pub delta: Option<i64>,
}

/// 今天记过快照了吗。日切判断读它，`None` = 还没有。
pub async fn credit_day(pool: &SqlitePool, day: &str) -> Result<Option<CreditDay>, sqlx::Error> {
    sqlx::query_as::<_, CreditDay>(
        "SELECT day, at, balance, spent_since_prev, delta FROM v2v_credit_daily WHERE day = ?1",
    )
    .bind(day)
    .fetch_optional(pool)
    .await
}

/// 最近一条快照（用来算 delta 的基准）。
pub async fn latest_credit_day(pool: &SqlitePool) -> Result<Option<CreditDay>, sqlx::Error> {
    sqlx::query_as::<_, CreditDay>(
        "SELECT day, at, balance, spent_since_prev, delta FROM v2v_credit_daily
          ORDER BY day DESC LIMIT 1",
    )
    .fetch_optional(pool)
    .await
}

/// 写入当天快照。
///
/// `INSERT OR IGNORE`：一天只记第一次。**不是 UPSERT** —— 当天第二次写会用一个更晚的
/// 余额覆盖掉，而那之间跑掉的额度就再也对不上账了，实验数据当场作废。
pub async fn insert_credit_day(pool: &SqlitePool, row: &CreditDay) -> Result<bool, sqlx::Error> {
    let r = sqlx::query(
        "INSERT OR IGNORE INTO v2v_credit_daily (day, at, balance, spent_since_prev, delta)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )
    .bind(&row.day)
    .bind(row.at)
    .bind(row.balance)
    .bind(row.spent_since_prev)
    .bind(row.delta)
    .execute(pool)
    .await?;
    Ok(r.rows_affected() > 0)
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

/// 一次取回若干条（撤销那条路径要拿整批的当前状态对比快照）。
pub async fn get_many(pool: &SqlitePool, ids: &[i64]) -> Result<Vec<ClipRow>, sqlx::Error> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let holes = vec!["?"; ids.len()].join(",");
    let sql = format!("{SELECT} WHERE c.id IN ({holes})");
    let mut q = sqlx::query_as::<_, ClipRow>(&sql);
    for i in ids {
        q = q.bind(*i);
    }
    q.fetch_all(pool).await
}

/// 这些 clip **此刻**指着的成片与封面路径（去空）。
///
/// 废纸篓物理删之前拿它当闸门：视频重跑是就地的，路径锚在 clip id 上，于是
/// 「判过不通过 → 重跑 → 新片子落在同一个路径」是常态。一条陈旧的废纸篓记录若还
/// 指着那两个文件，清空废纸篓就会删掉还活着的成片。
pub async fn current_media_paths(
    pool: &SqlitePool,
    ids: &[i64],
) -> Result<std::collections::HashSet<String>, sqlx::Error> {
    if ids.is_empty() {
        return Ok(std::collections::HashSet::new());
    }
    let holes = vec!["?"; ids.len()].join(",");
    let sql =
        format!("SELECT video_path, poster_path, export_path FROM v2v_clips WHERE id IN ({holes})");
    let mut q = sqlx::query_as::<_, (Option<String>, Option<String>, Option<String>)>(&sql);
    for i in ids {
        q = q.bind(*i);
    }
    let rows = q.fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .flat_map(|(v, p, e)| [v, p, e])
        .flatten()
        .filter(|s| !s.trim().is_empty())
        .collect())
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

/// 「这一条走哪条通道」的 SQL 表达式：条目自带的型号优先，没写就落到设置里的默认型号。
///
/// 占位符编号写死成 `?1`，故所有用到它的查询都必须**第一个**绑默认型号。
const CHANNEL_OF: &str = "COALESCE(NULLIF(TRIM(model_version), ''), ?1)";

/// 某条通道此刻在即梦手上的条数（0031）。
///
/// 并发闸门读的是它而不是 [`count_in_flight`]：实测即梦按模型通道各排各的队
/// （`queue_info.debug_info.dreamina_matrix_queue_name` 逐通道不同），2.0fast 排满了
/// 与 2.0mini 能不能发**毫无关系**。按全局口径数会让第二条通道永远发不出去。
pub async fn count_in_flight_on(
    pool: &SqlitePool,
    default_model: &str,
    channel: &str,
) -> Result<i64, sqlx::Error> {
    let (n,): (i64,) = sqlx::query_as(&format!(
        "SELECT COUNT(*) FROM v2v_clips WHERE stage='run' AND {CHANNEL_OF} = ?2"
    ))
    .bind(default_model)
    .bind(channel)
    .fetch_one(pool)
    .await?;
    Ok(n)
}

/// 某条通道上、在跑且**即梦确实收下了**的条数（有计费回执或队列位次）。
///
/// 它是「即梦这一刻在这条通道上实际允许我们同时跑几条」的下界：撞上
/// `ExceedConcurrencyLimit` 那一刻，被收下的这些就是这条通道的上限本身。自适应夹取
/// （`runner::observe_concurrency_reject`）用它，免得逼人去猜一个只有即梦知道的数字。
///
/// **按通道数**（0031）：跨通道数会把别的通道正在跑的条目算进来，于是一条通道撞墙
/// 就把所有通道的上限一起收敛到一个偏大的数，此后每轮都要再撞一次才知道发不出去。
pub async fn count_running_accepted_on(
    pool: &SqlitePool,
    default_model: &str,
    channel: &str,
) -> Result<i64, sqlx::Error> {
    let (n,): (i64,) = sqlx::query_as(&format!(
        "SELECT COUNT(*) FROM v2v_clips WHERE stage='run' AND {CHANNEL_OF} = ?2
           AND (credit_count IS NOT NULL OR submit_credit IS NOT NULL OR queue_idx IS NOT NULL)"
    ))
    .bind(default_model)
    .bind(channel)
    .fetch_one(pool)
    .await?;
    Ok(n)
}

/// 某通道还存在多少条「已调用提交，但远端到底收没收尚无证据」的任务。
///
/// 只要有一条这样的单，容量探测就必须暂停：继续提交不能增加任何关于并发容量的信息，
/// 却可能在 CLI 少回一个计费字段时把整批推成结果不明。`submit_id` 也不是证据——即梦会
/// 给 1310 拒收请求分配 id。待轮询拿到计费/位次，或明确判失败后，这道临时闸自动解除。
pub async fn count_running_unverified_on(
    pool: &SqlitePool,
    default_model: &str,
    channel: &str,
) -> Result<i64, sqlx::Error> {
    let (n,): (i64,) = sqlx::query_as(&format!(
        "SELECT COUNT(*) FROM v2v_clips WHERE stage='run' AND {CHANNEL_OF} = ?2
           AND credit_count IS NULL AND submit_credit IS NULL AND queue_idx IS NULL"
    ))
    .bind(default_model)
    .bind(channel)
    .fetch_one(pool)
    .await?;
    Ok(n)
}

/// 本地待发队列的成员判据（0028）：人已放行、有提示词、还没轮到它。
const QUEUED_POOL: &str = "stage='ready' AND submit_queued_at IS NOT NULL
      AND video_prompt IS NOT NULL AND TRIM(video_prompt) <> ''";

/// 记下「人已放行、在等即梦的空位」。已在队列里的不刷新时刻（保先进先出）。
pub async fn mark_submit_queued(
    pool: &SqlitePool,
    ids: &[i64],
    now: i64,
) -> Result<i64, sqlx::Error> {
    if ids.is_empty() {
        return Ok(0);
    }
    let holes = vec!["?"; ids.len()].join(",");
    let sql = format!(
        "UPDATE v2v_clips SET submit_queued_at=?, updated_at=?
          WHERE stage='ready' AND submit_queued_at IS NULL AND id IN ({holes})"
    );
    let mut q = sqlx::query(&sql).bind(now).bind(now);
    for i in ids {
        q = q.bind(*i);
    }
    Ok(q.execute(pool).await?.rows_affected() as i64)
}

/// 本地待发队列里还有几条。
pub async fn count_submit_queued(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
    let (n,): (i64,) = sqlx::query_as(&format!(
        "SELECT COUNT(*) FROM v2v_clips WHERE {QUEUED_POOL}"
    ))
    .fetch_one(pool)
    .await?;
    Ok(n)
}

/// 取**某条通道**队首的 `limit` 条（0031）。
///
/// **先进先出**（放行时刻，其次 id）：一批 20 条的顺序就是人当时在表格里看到的顺序，
/// 而不是每轮随机挑几条。
///
/// 补位必须按通道各取各的：2.0fast 排满时，队列里那 6 条 2.0mini 照样发得出去，
/// 而按全局队首取会一直取到 2.0fast 的条目、发现没空位、然后什么都不做。
pub async fn pick_submit_queued_on(
    pool: &SqlitePool,
    default_model: &str,
    channel: &str,
    limit: i64,
) -> Result<Vec<i64>, sqlx::Error> {
    if limit <= 0 {
        return Ok(Vec::new());
    }
    let rows: Vec<(i64,)> = sqlx::query_as(&format!(
        "SELECT id FROM v2v_clips WHERE {QUEUED_POOL} AND {CHANNEL_OF} = ?2
          ORDER BY submit_queued_at, id LIMIT ?3"
    ))
    .bind(default_model)
    .bind(channel)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// 一条通道此刻的全貌（0031）：远端在跑几条、本地还压着几条、队首排在第几位。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChannelRow {
    /// 型号全名。空串 = 设置里没指定模型，实际通道由 CLI 自己挑。
    pub model_version: String,
    /// 已提交、还在即梦手上（`stage='run'`）。
    pub running: i64,
    /// 人已放行、在本地等这条通道的空位。
    pub queued: i64,
    /// 有提示词、但还没放行（「等你点确认提交」）。
    pub ready: i64,
    /// 这条通道上最靠前那一单的即梦排队位次。`None` = 没有任何一单报得出位次
    /// （要么在生成中，要么还没问到）—— **绝不补 0**，0 在回体里的含义是「已出队」。
    pub front_queue_idx: Option<i64>,
    /// 这条通道上最早那一单的提交时刻。
    pub oldest_submitted_at: Option<i64>,
    /// 在跑的里面有几条是常驻队列放行的。
    pub auto_running: i64,
}

/// 按通道汇总在跑与本地待发。一次查询出全部通道 —— 顶部那排状态灯与补位循环共用它。
///
/// 只取 `run`/`ready` 两个阶段：已定案的条目不占任何通道的位子，把它们算进来只会让
/// 一条早已跑完的通道永远挂在状态栏上。
pub async fn channel_stats(
    pool: &SqlitePool,
    default_model: &str,
) -> Result<Vec<ChannelRow>, sqlx::Error> {
    type Row = (String, i64, i64, i64, Option<i64>, Option<i64>, i64);
    let rows: Vec<Row> = sqlx::query_as(&format!(
        "SELECT {CHANNEL_OF} AS ch,
                CAST(SUM(CASE WHEN stage='run' THEN 1 ELSE 0 END) AS INTEGER),
                CAST(SUM(CASE WHEN stage='ready' AND submit_queued_at IS NOT NULL
                          AND video_prompt IS NOT NULL AND TRIM(video_prompt) <> ''
                         THEN 1 ELSE 0 END) AS INTEGER),
                CAST(SUM(CASE WHEN stage='ready' AND submit_queued_at IS NULL
                          AND video_prompt IS NOT NULL AND TRIM(video_prompt) <> ''
                         THEN 1 ELSE 0 END) AS INTEGER),
                MIN(CASE WHEN stage='run' AND queue_idx > 0 THEN queue_idx END),
                MIN(CASE WHEN stage='run' THEN submitted_at END),
                CAST(SUM(CASE WHEN stage='run' AND auto_submitted=1 THEN 1 ELSE 0 END) AS INTEGER)
           FROM v2v_clips
          WHERE stage IN ('run','ready')
          GROUP BY ch
          ORDER BY ch"
    ))
    .bind(default_model)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(model_version, running, queued, ready, front, oldest, auto_running)| ChannelRow {
                model_version,
                running,
                queued,
                ready,
                front_queue_idx: front,
                oldest_submitted_at: oldest,
                auto_running,
            },
        )
        .filter(|c| c.running > 0 || c.queued > 0 || c.ready > 0)
        .collect())
}

/// 撤回放行：本地待发 → 回到「等你点确认提交」。只动还没提交出去的。
pub async fn unqueue_submit(pool: &SqlitePool, ids: &[i64], now: i64) -> Result<i64, sqlx::Error> {
    if ids.is_empty() {
        return Ok(0);
    }
    let holes = vec!["?"; ids.len()].join(",");
    let sql = format!(
        "UPDATE v2v_clips SET submit_queued_at=NULL, updated_at=?
          WHERE stage='ready' AND submit_queued_at IS NOT NULL AND id IN ({holes})"
    );
    let mut q = sqlx::query(&sql).bind(now);
    for i in ids {
        q = q.bind(*i);
    }
    Ok(q.execute(pool).await?.rows_affected() as i64)
}

/// 把一条被并发上限判死的 `fail` 条目救回本地队列。
///
/// 它只服务于一次性的存量修复（`runner::heal_concurrency_rejects`）：0028 之前
/// `ExceedConcurrencyLimit` 一律记成 `fail(provider)`，于是升级前被弹回来的那些
/// 会永远躺在「处理异常」里等人一条条点重跑 —— 而重跑又会撞上同一堵墙。
///
/// 谓词里的 `submit_id` 保持不动地被清掉：那个 id 早就死了（即梦判了 fail）。
/// 调用方必须先确认这一条没有任何计费证据。
pub async fn revive_rejected_fail(
    pool: &SqlitePool,
    id: i64,
    queued_at: i64,
    now: i64,
) -> Result<bool, sqlx::Error> {
    let res = sqlx::query(
        "UPDATE v2v_clips
            SET stage='ready', submit_id=NULL, submit_queued_at=?2,
                gen_status=NULL, queue_idx=NULL, polled_at=NULL,
                submitted_at=NULL, submit_credit=NULL, submit_status=NULL,
                error_type=NULL, error_message=NULL, finished_at=NULL, updated_at=?3,
                attempt = MAX(0, attempt - 1)
          WHERE id=?1 AND stage='fail' AND error_type='provider'",
    )
    .bind(id)
    .bind(queued_at)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// 即梦以「同时在跑的太多了」为由拒了这一单 → 放回本地队列，**排在队首**。
///
/// 与 [`mark_failed`] 的区别就是这条路径存在的全部理由：`ExceedConcurrencyLimit`
/// 回来时 `credit_count` 缺席 —— 一分钱没扣，任务也从没跑过。把它记成 `fail` 等于
/// 让一条「排在后面」的片子躺进「处理异常」，还得人一条条去点重跑。
///
/// 清 `submit_id` 是必须的（那个 id 已经死了，下次要重新下单），所以调用方**必须**
/// 先确认这一单没有任何计费证据（`runner::Evidence::billed`）。真扣了钱的那一条
/// 该走 `mark_failed`，让人自己判断。
///
/// `submit_queued_at` 取**原提交时刻**而不是当下：它本来就该排在后面那些还没试过的
/// 前面 —— 它已经等过一轮了。
///
/// `attempt` 要**退回去**：那一列的含义是「这张图花过几份额度」（界面上的「重跑过」
/// 信号就读它），而这一次连队都没入。不退的话，一批被并发上限弹回来的片子会集体
/// 显示成「重跑过」，把真正重跑过、真花过两份钱的那些淹掉。
pub async fn requeue_after_reject(
    pool: &SqlitePool,
    id: i64,
    expect_submit: &str,
    queued_at: i64,
    now: i64,
) -> Result<bool, sqlx::Error> {
    let res = sqlx::query(&format!(
        "UPDATE v2v_clips
            SET stage='ready', submit_id=NULL, submit_queued_at=?2,
                gen_status=NULL, queue_idx=NULL, polled_at=NULL,
                submitted_at=NULL, submit_credit=NULL, submit_status=NULL,
                error_type=NULL, error_message=NULL, updated_at=?3,
                attempt = MAX(0, attempt - 1)
          WHERE id=?1 AND {OWNED}?4"
    ))
    .bind(id)
    .bind(queued_at)
    .bind(now)
    .bind(expect_submit)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// 补单器能碰的条目：待提交、有视频提示词、且**没人给它指定过模型**。
///
/// 最后一条是硬边界。补单器会把自己的廉价参数写进它挑中的条目 —— 若它捡走一条用户
/// （或 skill）特意设了 `seedance2.0_vip / 1080p` 的片子，那份选择会被静悄悄降级，
/// 而人要到出片时才看得出来。指定过参数 = 一个深思熟虑的决定，常驻队列不碰它。
/// 另有一条 `submit_queued_at IS NULL`（0028）：人已经放行、正在本地队列里等空位的
/// 条目，补单器不许碰 —— 它会把自己的廉价参数写进挑中的条目，而那一批人是照着
/// 确认卡上那套参数点的确认。
const AUTOFILL_POOL: &str = "stage='ready'
      AND video_prompt IS NOT NULL AND TRIM(video_prompt) <> ''
      AND submit_queued_at IS NULL
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
///
/// ## 三项生成参数也在里面
///
/// 原先不在，理由是「纯参数改动本来就免费，换回去再点一次即可」。但换通道那条路径把这
/// 个理由推翻了：它**同时**丢弃一单已付费任务并改写参数，撤销只写回 `submit_id` 而不
/// 写回型号，恢复出来的就是一条「提交单属于 A 通道、库里却记着 B 通道」的行 ——
/// 而通道正是并发空位的分桶键（`channel_of`），认错通道就会数着 B 的空位往 A 发单。
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
    pub model_version: Option<String>,
    pub duration: Option<i64>,
    pub video_resolution: Option<String>,
}

/// 取快照（撤销令牌的原料）。
pub async fn snapshot<'e, E>(ex: E, id: i64) -> Result<Option<ClipSnapshot>, sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    sqlx::query_as::<_, ClipSnapshot>(
        "SELECT id, stage, video_prompt, submit_id, video_path, poster_path, width, height,
                fps, duration_sec, credit_count, error_type, error_message, gen_status,
                queue_idx, polled_at, submitted_at, finished_at, reviewed_at, attempt,
                model_version, duration, video_resolution
           FROM v2v_clips WHERE id = ?1",
    )
    .bind(id)
    .fetch_optional(ex)
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
                attempt=?20, updated_at=?21,
                model_version=?22, duration=?23, video_resolution=?24
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
    .bind(&s.model_version)
    .bind(s.duration)
    .bind(&s.video_resolution)
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

/// 启动恢复：run 态但没有 submit_id = 进程可能死在「远端已接单」与「本地回执落库」之间。
///
/// 这个窗口无法证明远端没扣费，所以绝不能自动退回 ready。隔离到 `fail` 让人先去即梦
/// 核对；这是宁可暂时卡住一条，也不拿用户余额赌一次自动重提。
/// 有 submit_id 的条目保持 run，由轮询器继续认领。
pub async fn recover_orphan_submits(pool: &SqlitePool, now: i64) -> Result<i64, sqlx::Error> {
    let res = sqlx::query(
        "UPDATE v2v_clips
            SET stage='fail', error_type='submit_interrupted',
                error_message='提交进程中断且没有拿到任务 ID；远端是否扣费无法确认。为防重复扣费，未自动重提，请先在即梦核对。',
                finished_at=?1, updated_at=?1
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
            "sub-1",
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

    // 深度数的是**全部**在跑条目，不只是补单器自己放出去的（0028 推翻了旧口径）。
    //
    // 旧断言是「只数它自己的 = 1」，理由是「人手动跑 20 条时常驻队列不该静悄悄停摆」。
    // 那条理由被实测推翻：即梦的并发上限是账户级的，人占满了唯一那个位子时补单器
    // 再发出去的单子回来的是 `ExceedConcurrencyLimit` —— 「停摆」正是此时唯一正确的
    // 行为，而按旧口径它会一边发一边被弹回来。`auto_submitted` 仍在，但只用于
    // 「这一条是谁放行的」的展示。
    #[tokio::test]
    async fn in_flight_counts_every_submission_regardless_of_who_released_it() {
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
        assert_eq!(
            count_in_flight_on(&pool, "seedance2.0fast", "seedance2.0fast")
                .await
                .unwrap(),
            2,
            "两条都在即梦手上，抢的是同一份并发配额"
        );
    }

    /// 建 n 条走到 `ready`（待提交）的 clip，返回 id 列表。
    async fn seed_ready(pool: &SqlitePool, n: i64) -> Vec<i64> {
        for w in 1..=n {
            seed_work(pool, w).await;
            enqueue_one(pool, w).await;
        }
        let ids: Vec<i64> = list_by_stages(pool, &["rewrite"])
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
        ids
    }

    // 本地待发队列（0028）：放行 = 记一个时刻，取用严格先进先出。
    //
    // 顺序不是洁癖：一批 20 条的提交顺序就是人在表格里看到的顺序，而它们要花好几个
    // 小时才轮完 —— 每轮随机挑几条的话，「这一批做到哪了」在中途完全无法回答。
    #[tokio::test]
    async fn submit_queue_is_strictly_first_in_first_out() {
        let (pool, _d) = test_pool().await;
        let ids = seed_ready(&pool, 3).await;
        // 放行顺序故意与 id 顺序相反，验证排的是**放行时刻**而不是 id。
        mark_submit_queued(&pool, &[ids[2]], 300).await.unwrap();
        mark_submit_queued(&pool, &[ids[0], ids[1]], 301)
            .await
            .unwrap();
        assert_eq!(count_submit_queued(&pool).await.unwrap(), 3);
        assert_eq!(
            pick_submit_queued_on(&pool, "seedance2.0fast", "seedance2.0fast", 2)
                .await
                .unwrap(),
            vec![ids[2], ids[0]],
            "先放行的先走"
        );
        // 重复放行不刷新时刻，否则再点一次确认就会把队首挤到队尾。
        mark_submit_queued(&pool, &ids, 999).await.unwrap();
        assert_eq!(
            pick_submit_queued_on(&pool, "seedance2.0fast", "seedance2.0fast", 1)
                .await
                .unwrap(),
            vec![ids[2]]
        );
    }

    // 撤回放行只动还没发出去的，且撤回后就不在队列里了。
    #[tokio::test]
    async fn unqueue_only_touches_what_has_not_been_sent() {
        let (pool, _d) = test_pool().await;
        let ids = seed_ready(&pool, 2).await;
        mark_submit_queued(&pool, &ids, 300).await.unwrap();
        mark_submitted(&pool, ids[0], &SubmitReceipt::healthy("s", 8), 400)
            .await
            .unwrap();
        assert_eq!(
            unqueue_submit(&pool, &ids, 500).await.unwrap(),
            1,
            "已经在即梦手上的那条撤不回来"
        );
        assert_eq!(count_submit_queued(&pool).await.unwrap(), 0);
    }

    // 补单器不许碰人已经放行的条目：它会把自己的廉价参数写进去，
    // 而那一批人是照着确认卡上那套参数点的确认。
    #[tokio::test]
    async fn autofill_never_steals_from_the_local_submit_queue() {
        let (pool, _d) = test_pool().await;
        let ids = seed_ready(&pool, 2).await;
        assert_eq!(count_autofill_pool(&pool).await.unwrap(), 2);
        mark_submit_queued(&pool, &[ids[0]], 300).await.unwrap();
        assert_eq!(count_autofill_pool(&pool).await.unwrap(), 1);
        assert_eq!(pick_autofill(&pool, 10).await.unwrap(), vec![ids[1]]);
    }

    // 「同时在跑的太多了」被弹回来 ≠ 失败：回到 ready、排在队首、attempt 退回去。
    //
    // attempt 那一格是这条测试的重点：它的含义是「这张图花过几份额度」，界面上的
    // 「重跑过」信号读它。一批被并发上限弹回来的片子若集体带着 +1，就会把真正
    // 重跑过、真花过两份钱的那些淹掉。
    #[tokio::test]
    async fn concurrency_reject_returns_to_the_head_of_the_queue_without_counting_an_attempt() {
        let (pool, _d) = test_pool().await;
        let ids = seed_ready(&pool, 1).await;
        let id = ids[0];
        mark_submit_queued(&pool, &[id], 300).await.unwrap();
        mark_submitted(&pool, id, &SubmitReceipt::healthy("sub-x", 8), 400)
            .await
            .unwrap();
        let before = list_by_stages(&pool, &["run"]).await.unwrap()[0].clone();
        assert_eq!(before.attempt, 1);

        assert!(requeue_after_reject(&pool, id, "sub-x", 300, 500)
            .await
            .unwrap());
        let after = list_by_stages(&pool, &["ready"]).await.unwrap()[0].clone();
        assert_eq!(after.stage, "ready");
        assert!(after.submit_id.is_none(), "那个 submit_id 已经死了");
        assert_eq!(after.submit_queued_at, Some(300), "保住原来的队列位置");
        assert_eq!(after.attempt, 0, "连队都没入，不算一次尝试");
        assert!(after.error_type.is_none(), "它不是一条出了错的记录");
        assert_eq!(
            pick_submit_queued_on(&pool, "seedance2.0fast", "seedance2.0fast", 5)
                .await
                .unwrap(),
            vec![id],
            "弹回来之后仍在本地队列里等空位"
        );
    }

    // 存量修复：升级前被并发上限误判成 fail 的条目要能救回队列。
    //
    // 只改新逻辑不管存量，等于让这个 bug 的后果留在原地 —— 用户那批 9 条里有 8 条
    // 就躺在「处理异常」，一分钱没扣却要人一条条点重跑，而重跑还会撞同一堵墙。
    #[tokio::test]
    async fn a_rejected_fail_can_be_revived_into_the_queue() {
        let (pool, _d) = test_pool().await;
        let ids = seed_ready(&pool, 1).await;
        let id = ids[0];
        mark_submitted(&pool, id, &SubmitReceipt::bare("dead-id"), 400)
            .await
            .unwrap();
        mark_failed(
            &pool,
            id,
            Some("dead-id"),
            "provider",
            "api error: ret=1310, message=ExceedConcurrencyLimit, logid=x",
            500,
        )
        .await
        .unwrap();

        assert!(revive_rejected_fail(&pool, id, 400, 600).await.unwrap());
        let after = list_by_stages(&pool, &["ready"]).await.unwrap()[0].clone();
        assert_eq!(after.submit_queued_at, Some(400));
        assert!(after.submit_id.is_none());
        assert_eq!(after.attempt, 0);
        assert!(after.finished_at.is_none(), "它并没有「结束」过");
        assert_eq!(
            pick_submit_queued_on(&pool, "seedance2.0fast", "seedance2.0fast", 5)
                .await
                .unwrap(),
            vec![id]
        );

        // 幂等：第二次没东西可救（它已经不是 fail 了）。
        assert!(!revive_rejected_fail(&pool, id, 400, 700).await.unwrap());
    }

    // 同一条通道内，人放行的和补单器放行的算同一份配额。
    #[tokio::test]
    async fn accepted_count_only_counts_what_dreamina_actually_took() {
        let (pool, _d) = test_pool().await;
        let ids = seed_ready(&pool, 2).await;
        mark_submitted(&pool, ids[0], &SubmitReceipt::healthy("a", 8), 400)
            .await
            .unwrap();
        // 第二条：即梦给了 submit_id 却没有任何计费/位次证据（正是被并发上限拒掉的样子）。
        mark_submitted(&pool, ids[1], &SubmitReceipt::bare("b"), 400)
            .await
            .unwrap();
        assert_eq!(
            count_in_flight_on(&pool, "seedance2.0fast", "seedance2.0fast")
                .await
                .unwrap(),
            2
        );
        assert_eq!(
            count_running_accepted_on(&pool, "seedance2.0fast", "seedance2.0fast")
                .await
                .unwrap(),
            1,
            "只有拿到计费回执的那条算即梦真收下了 —— 它就是这条通道上限本身"
        );
        assert_eq!(
            count_running_unverified_on(&pool, "seedance2.0fast", "seedance2.0fast")
                .await
                .unwrap(),
            1,
            "没有计费/位次证据的任务必须暂时挡住本通道继续扩容"
        );
        assert_eq!(
            count_running_unverified_on(&pool, "seedance2.0fast", "seedance2.0mini")
                .await
                .unwrap(),
            0,
            "未决任务不能串台挡住其它通道"
        );
    }

    /// 通道之间必须互不相干（0031）—— 这是这一版的核心不变量。
    ///
    /// 即梦按模型通道各排各的队（实测 `queue_info.debug_info.dreamina_matrix_queue_name`
    /// 逐通道不同：2.0fast 与 2.0mini 是两条完全不同的队）。按全局口径数在跑条数，
    /// 会让 2.0fast 上那条长队把 2.0mini 上的条目一起锁死 —— 而 2.0mini 那条队是空的。
    #[tokio::test]
    async fn one_busy_channel_never_blocks_another() {
        let (pool, _d) = test_pool().await;
        let ids = seed_ready(&pool, 3).await;
        // 两条走默认通道（model_version 留空 → 落到 default_model），一条显式 mini。
        set_params(&pool, &[ids[2]], Some("seedance2.0mini"), None, None, 300)
            .await
            .unwrap();
        mark_submitted(&pool, ids[0], &SubmitReceipt::healthy("a", 8), 400)
            .await
            .unwrap();
        mark_submit_queued(&pool, &[ids[1], ids[2]], 401)
            .await
            .unwrap();

        let fast = count_in_flight_on(&pool, "seedance2.0fast", "seedance2.0fast")
            .await
            .unwrap();
        let mini = count_in_flight_on(&pool, "seedance2.0fast", "seedance2.0mini")
            .await
            .unwrap();
        assert_eq!((fast, mini), (1, 0), "mini 那条通道上一条都没在跑");

        // 补位取队首也必须按通道各取各的：全局队首是 fast 的那条，
        // 而 mini 上那条一样该发得出去。
        assert_eq!(
            pick_submit_queued_on(&pool, "seedance2.0fast", "seedance2.0mini", 5)
                .await
                .unwrap(),
            vec![ids[2]],
        );
        assert_eq!(
            pick_submit_queued_on(&pool, "seedance2.0fast", "seedance2.0fast", 5)
                .await
                .unwrap(),
            vec![ids[1]],
        );

        // 汇总视图：两条通道各占一行，数字各归各的。
        let chans = channel_stats(&pool, "seedance2.0fast").await.unwrap();
        let by = |m: &str| {
            chans
                .iter()
                .find(|c| c.model_version == m)
                .cloned()
                .unwrap_or_default()
        };
        assert_eq!(
            (by("seedance2.0fast").running, by("seedance2.0fast").queued),
            (1, 1)
        );
        assert_eq!(
            (by("seedance2.0mini").running, by("seedance2.0mini").queued),
            (0, 1)
        );
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

        // 退回改写会清空成片引用；撤销必须能整份恢复。
        assert!(requeue_for_rewrite(&pool, id, 500).await.unwrap());
        let after = get(&pool, id).await.unwrap().unwrap();
        assert_eq!(after.stage, "rewrite");
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
        mark_failed(&pool, b, None, "phantom", "无位次、无计费", 450)
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

    /// 轮询结算**只能写给它当初问的那一单**。
    ///
    /// 场景是实跑里必然会撞上的：手动刷新逐条问，一轮几十秒到几分钟；人在这段时间里
    /// 重跑了其中一条，于是那一行换成了新的 submit_id B。此时旧单 A 的回体才到 ——
    /// 若结算只按 id 写，B 会被写成 `rev`，而 `list_running` 从此捞不到它：**一条已经
    /// 扣过费、即梦那边还在跑的任务当场失联**，界面上却摆着 A 的成片，看不出任何异常。
    #[tokio::test]
    async fn settling_an_old_submit_cannot_hijack_the_row_after_a_rerun() {
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
        // 改投时放弃 sub-1，随后重新提交，这一行现在属于 sub-2。
        assert!(requeue_for_run(&pool, id, Some("sub-1"), 500)
            .await
            .unwrap());
        mark_submitted(&pool, id, &SubmitReceipt::healthy("sub-2", 8), 600)
            .await
            .unwrap();

        // 旧单 sub-1 的回体姗姗来迟，四条结算路径**一条都不许落地**。
        assert!(
            !mark_ready_for_review(
                &pool, id, "sub-1", "/old.mp4", None, None, None, None, None, None, None, 700,
            )
            .await
            .unwrap(),
            "旧单出片不得把这一行改成待验收"
        );
        assert!(
            !mark_failed(&pool, id, Some("sub-1"), "timeout", "旧单超时", 700)
                .await
                .unwrap(),
            "旧单判死不得把这一行改成失败"
        );
        assert!(
            !requeue_after_reject(&pool, id, "sub-1", 100, 700)
                .await
                .unwrap(),
            "旧单被弹回不得把这一行推回待提交"
        );
        mark_polled(&pool, id, "sub-1", "success", Some(1), Some(99), None, 700)
            .await
            .unwrap();

        let row = get(&pool, id).await.unwrap().unwrap();
        assert_eq!(row.stage, "run", "这一行仍属于新提交单");
        assert_eq!(row.submit_id.as_deref(), Some("sub-2"));
        assert!(row.video_path.is_none(), "旧单的成片不得挂到新单头上");
        assert!(row.gen_status.is_none(), "旧单的状态原文同样不得写进来");
        assert!(row.polled_at.is_none());
        assert_eq!(
            list_running(&pool).await.unwrap().len(),
            1,
            "新单必须仍在轮询集合里 —— 掉出去就是一条已付费任务永远失联"
        );
    }

    /// 逐条 `query_result` 问到的计费要**当场落库**（同 `mark_swept`，COALESCE 只增不抹）。
    ///
    /// 非 VIP 十分钟才扫一轮，一条排队几小时的单会被问到几十次、每次都带着计费回执；
    /// 原先这条路径只写状态与位次，于是库里那一列直到出片才第一次落值 ——
    /// 「在跑的这些已经花了多少」在最需要它的那几个小时里恒为 0。
    #[tokio::test]
    async fn polling_persists_the_bill_the_first_time_it_sees_one() {
        let (pool, _d) = test_pool().await;
        seed_work(&pool, 1).await;
        enqueue_one(&pool, 1).await;
        let id = list_by_stages(&pool, &["rewrite"]).await.unwrap()[0].id;
        mark_submitted(&pool, id, &SubmitReceipt::bare("sub-1"), 500)
            .await
            .unwrap();
        assert!(get(&pool, id)
            .await
            .unwrap()
            .unwrap()
            .credit_count
            .is_none());

        mark_polled(
            &pool,
            id,
            "sub-1",
            "queue",
            Some(4485),
            Some(8),
            Some("dreamina_x"),
            900,
        )
        .await
        .unwrap();
        let row = get(&pool, id).await.unwrap().unwrap();
        assert_eq!(row.credit_count, Some(8), "问到的计费要落库");
        assert_eq!(row.benefit_type.as_deref(), Some("dreamina_x"));

        // 下一轮回体不再带计费（离开排队之后 commerce_info 会消失）→ 不得抹掉。
        mark_polled(&pool, id, "sub-1", "generating", None, None, None, 1000)
            .await
            .unwrap();
        let row = get(&pool, id).await.unwrap().unwrap();
        assert_eq!(row.credit_count, Some(8), "已问到的账只增不抹");
        assert_eq!(row.queue_idx, Some(4485));
    }

    // 有可用输出的任务不能送回生成队列；不通过只能退回改写。
    #[tokio::test]
    async fn requeue_for_run_never_accepts_completed_output() {
        let (pool, _d) = test_pool().await;
        let id = seed_reviewable(&pool, 1).await;
        assert!(set_reviewed(&pool, id, "rej", 500).await.unwrap());

        assert!(!requeue_for_run(&pool, id, None, 600).await.unwrap());
        let row = get(&pool, id).await.unwrap().unwrap();
        assert_eq!(row.stage, "rej");
        assert!(row.video_path.is_some(), "已有输出不得被清空");
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
                &pool, id, "s", "/v.mp4", None, None, None, None, None, None, None, 400,
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
            &pool, id, "s", "/v.mp4", None, None, None, None, None, None, None, 400,
        )
        .await
        .unwrap();
        assert!(set_reviewed(&pool, id, "pass", 500).await.unwrap());
        assert!(
            !set_reviewed(&pool, id, "rej", 600).await.unwrap(),
            "已定态不可再次验收"
        );
    }

    // 中断保护：无 submit_id 不等于没扣费——进程可能死在远端接单与本地落库之间。
    // 结果不明的条目必须隔离；有 submit_id 的条目继续由轮询器认领。
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
        let orphan_row = get(&pool, orphan).await.unwrap().unwrap();
        assert_eq!(orphan_row.stage, "fail");
        assert_eq!(orphan_row.error_type.as_deref(), Some("submit_interrupted"));
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

    #[tokio::test]
    async fn claimed_rows_preserve_the_requested_fifo_order() {
        let (pool, _d) = test_pool().await;
        let ids = seed_ready(&pool, 3).await;
        let requested = [ids[2], ids[0], ids[1]];
        let claimed = claim_ready(&pool, &requested, 300).await.unwrap();
        assert_eq!(
            claimed.iter().map(|row| row.id).collect::<Vec<_>>(),
            requested,
            "IN 查询的数据库行序不能覆盖队列挑选器给出的 FIFO 顺序"
        );
    }

    #[tokio::test]
    async fn releasing_an_unsubmitted_tail_keeps_manual_fifo_and_queues_autofill_rows() {
        let (pool, _d) = test_pool().await;
        let ids = seed_ready(&pool, 2).await;
        mark_submit_queued(&pool, &[ids[0]], 111).await.unwrap();
        claim_ready(&pool, &ids, 300).await.unwrap();

        assert_eq!(release_claims(&pool, &ids, 500).await.unwrap(), 2);
        let manual = get(&pool, ids[0]).await.unwrap().unwrap();
        let autofill = get(&pool, ids[1]).await.unwrap().unwrap();
        assert_eq!(manual.stage, "ready");
        assert_eq!(autofill.stage, "ready");
        assert_eq!(manual.submit_queued_at, Some(111), "人工队列顺序必须保留");
        assert_eq!(
            autofill.submit_queued_at,
            Some(500),
            "常驻补单被拒后也必须进入自动候补，不能搁浅"
        );
    }

    /// 认领到一半被杀 → 启动恢复认得出来（run 且无 submit_id），但绝不自动重提。
    #[tokio::test]
    async fn a_claim_without_a_submit_id_is_quarantined_on_startup() {
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
        let row = get(&pool, id).await.unwrap().unwrap();
        assert_eq!(row.stage, "fail");
        assert_eq!(row.error_type.as_deref(), Some("submit_interrupted"));
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

        mark_polled(&pool, id, "sub-1", "queue", Some(3), None, None, 900)
            .await
            .unwrap();
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

        mark_polled(&pool, id, "sub-1", "queue", Some(4485), None, None, 900)
            .await
            .unwrap();
        // 开始生成了 —— 这一份回体不再带 queue_info。
        mark_polled(&pool, id, "sub-1", "generating", None, None, None, 1000)
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
        mark_swept(
            &pool,
            id,
            "sub-1",
            "generating",
            Some(8),
            Some("dreamina_x"),
            600,
        )
        .await
        .unwrap();
        // 出片回体只有视频元数据，没有 commerce_info。
        mark_ready_for_review(
            &pool,
            id,
            "sub-1",
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
        mark_polled(
            &pool,
            ids[2],
            "sub-3",
            "queue",
            Some(4485),
            None,
            None,
            submitted + 10,
        )
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
        mark_polled(&pool, id, "sub-1", "success", None, None, None, 900)
            .await
            .unwrap();

        assert!(requeue_for_run(&pool, id, None, 1000).await.unwrap());
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
            "s2",
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
        mark_failed(&pool, id, Some("s"), "timeout", "45 分钟仍未出片", 99_000)
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
        mark_failed(
            &pool,
            id,
            Some("sub-paid"),
            "timeout",
            "45 分钟仍未出片",
            3_200,
        )
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
        mark_failed(
            &pool,
            id,
            Some("sub-ghost"),
            "phantom",
            "即梦接了单但未入队",
            3_200,
        )
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
        mark_failed(&pool, id, None, "submit", "找不到即梦 CLI", 600)
            .await
            .unwrap();
        assert!(!resume_timed_out(&pool, id, 900).await.unwrap());
        assert_eq!(get(&pool, id).await.unwrap().unwrap().stage, "fail");
    }

    // 旧快照仍服务内部每日余额核对；没有 credit_count 的条目不计入。
    #[tokio::test]
    async fn credit_since_uses_actual_receipts_and_ignores_unbilled() {
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
                "s",
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

        // 按出片时刻切窗：1_000 那条落在窗外。
        assert_eq!(credit_since(&pool, 1_500).await.unwrap(), 66);
        assert_eq!(credit_since(&pool, 0).await.unwrap(), 110);
    }

    #[tokio::test]
    async fn credit_events_are_append_only_idempotent_and_survive_clip_deletion() {
        let (pool, _d) = test_pool().await;
        seed_work(&pool, 1).await;
        enqueue_one(&pool, 1).await;
        let id = list_by_stages(&pool, &["rewrite"]).await.unwrap()[0].id;

        mark_submitted_on(
            &pool,
            id,
            &SubmitReceipt::healthy("ledger-1", 8),
            "channel-a",
            1_700_000_000,
        )
        .await
        .unwrap();
        // 同一回体再次落库只会命中唯一索引，不会复制同一事实。
        mark_submitted_on(
            &pool,
            id,
            &SubmitReceipt::healthy("ledger-1", 8),
            "channel-a",
            1_700_000_000,
        )
        .await
        .unwrap();
        let before: Vec<(i64, String, Option<i64>)> =
            sqlx::query_as("SELECT id, event_type, credits FROM v2v_credit_events ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(
            before
                .iter()
                .filter(|(_, kind, _)| kind == "submit")
                .count(),
            1
        );
        assert_eq!(
            before
                .iter()
                .filter(|(_, kind, credit)| kind == "charge" && *credit == Some(8))
                .count(),
            1
        );

        // 后续拿到更准确的额度时追加新事实；旧行不被改写。
        sqlx::query("UPDATE v2v_clips SET credit_count=44, updated_at=1700000100 WHERE id=?1")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        let after: Vec<(i64, String, Option<i64>)> =
            sqlx::query_as("SELECT id, event_type, credits FROM v2v_credit_events ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(after[..before.len()], before);
        assert!(after
            .iter()
            .any(|(_, kind, credit)| kind == "charge" && *credit == Some(44)));

        remove(&pool, &[id]).await.unwrap();
        let rows = credit_events_by_channel(&pool, None).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].channel_key, "channel-a");
        assert_eq!(rows[0].spent_total, 44);
        assert_eq!(rows[0].spent_failed_abandoned, 44);
        assert_eq!(rows[0].events, 1, "一次提交只算一笔消费");
    }

    #[tokio::test]
    async fn resumed_submission_moves_from_failed_to_pending_then_abandoned() {
        let (pool, _d) = test_pool().await;
        seed_work(&pool, 1).await;
        enqueue_one(&pool, 1).await;
        let id = list_by_stages(&pool, &["rewrite"]).await.unwrap()[0].id;
        mark_submitted_on(
            &pool,
            id,
            &SubmitReceipt::healthy("resume-ledger", 18),
            "channel-a",
            1_700_000_000,
        )
        .await
        .unwrap();
        mark_failed(
            &pool,
            id,
            Some("resume-ledger"),
            "timeout",
            "等待超时",
            1_700_000_100,
        )
        .await
        .unwrap();
        let failed = credit_events_by_channel(&pool, None).await.unwrap();
        assert_eq!(failed[0].spent_failed_abandoned, 18);

        assert!(resume_timed_out(&pool, id, 1_700_000_200).await.unwrap());
        let pending = credit_events_by_channel(&pool, None).await.unwrap();
        assert_eq!(pending[0].spent_pending, 18);
        assert_eq!(pending[0].spent_failed_abandoned, 0);

        remove(&pool, &[id]).await.unwrap();
        let abandoned = credit_events_by_channel(&pool, None).await.unwrap();
        assert_eq!(abandoned[0].spent_pending, 0);
        assert_eq!(abandoned[0].spent_failed_abandoned, 18);
    }

    #[tokio::test]
    async fn credit_event_report_conserves_categories_ranges_and_pass_rate_inputs() {
        let (pool, _d) = test_pool().await;
        let cases = [
            (1, "pass-1", "channel-a", 10, "pass", 1_700_000_000),
            (2, "rej-1", "channel-a", 20, "rej", 1_700_086_400),
            (3, "fail-1", "channel-b", 30, "fail", 1_700_172_800),
            (4, "pending-1", "channel-b", 40, "pending", 1_700_259_200),
        ];
        for (work, submit, channel, credit, outcome, at) in cases {
            seed_work(&pool, work).await;
            enqueue_one(&pool, work).await;
            let id = list_by_stages(&pool, &["rewrite"])
                .await
                .unwrap()
                .into_iter()
                .find(|c| c.work_id == work)
                .unwrap()
                .id;
            mark_submitted_on(
                &pool,
                id,
                &SubmitReceipt::healthy(submit, credit),
                channel,
                at,
            )
            .await
            .unwrap();
            match outcome {
                "pass" | "rej" => {
                    mark_ready_for_review(
                        &pool,
                        id,
                        submit,
                        "/v.mp4",
                        None,
                        None,
                        None,
                        None,
                        None,
                        Some(credit),
                        None,
                        at + 10,
                    )
                    .await
                    .unwrap();
                    set_reviewed(&pool, id, outcome, at + 20).await.unwrap();
                }
                "fail" => {
                    mark_failed(&pool, id, Some(submit), "provider", "失败", at + 20)
                        .await
                        .unwrap();
                }
                _ => {}
            }
        }

        let rows = credit_events_by_channel(&pool, None).await.unwrap();
        assert_eq!(rows.len(), 2);
        let total: i64 = rows.iter().map(|r| r.spent_total).sum();
        let categories: i64 = rows
            .iter()
            .map(|r| r.spent_pass + r.spent_rej + r.spent_pending + r.spent_failed_abandoned)
            .sum();
        assert_eq!(total, 100);
        assert_eq!(categories, total, "四类之和必须守恒");
        assert_eq!(rows.iter().map(|r| r.passed).sum::<i64>(), 1);
        assert_eq!(rows.iter().map(|r| r.reviewed).sum::<i64>(), 2);

        let recent = credit_events_by_channel(&pool, Some(1_700_172_800))
            .await
            .unwrap();
        assert_eq!(recent.iter().map(|r| r.spent_total).sum::<i64>(), 70);
        let daily = credit_event_trend(&pool, None, false).await.unwrap();
        assert!(!daily.is_empty());
        assert_eq!(daily.iter().map(|r| r.spent).sum::<i64>(), 100);
        let monthly = credit_event_trend(&pool, None, true).await.unwrap();
        assert_eq!(monthly.iter().map(|r| r.spent).sum::<i64>(), 100);
    }

    #[tokio::test]
    async fn credit_event_migration_backfills_existing_clips() {
        use sqlx::sqlite::SqlitePoolOptions;

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE v2v_clips (
               id INTEGER PRIMARY KEY, submit_id TEXT, model_version TEXT,
               first_submitted_at INTEGER, submitted_at INTEGER, finished_at INTEGER,
               created_at INTEGER, updated_at INTEGER, reviewed_at INTEGER,
               credit_count INTEGER, submit_credit INTEGER, stage TEXT
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO v2v_clips VALUES
             (1,'legacy-submit','legacy-channel',100,NULL,200,10,300,300,12,NULL,'pass')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::raw_sql(include_str!(
            "../../../migrations/0033_v2v_credit_events.sql"
        ))
        .execute(&pool)
        .await
        .unwrap();

        let events: Vec<(String, i64, String, i64)> = sqlx::query_as(
            "SELECT event_type, is_backfill, channel_key, COALESCE(credits,0)
               FROM v2v_credit_events ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            events,
            vec![
                ("submit".into(), 1, "legacy-channel".into(), 12),
                ("pass".into(), 1, "legacy-channel".into(), 0),
            ]
        );
        let rows = credit_events_by_channel(&pool, None).await.unwrap();
        assert_eq!(rows[0].spent_pass, 12);
        assert_eq!(rows[0].passed, 1);
        assert_eq!(rows[0].reviewed, 1);
    }

    // 一天只记第一次。**故意不是 UPSERT**：当天第二次写会用一个更晚的余额覆盖掉，
    // 而那之间跑掉的额度就再也对不上账 —— 这份台账是个实验，覆盖掉就等于数据作废。
    #[tokio::test]
    async fn a_days_credit_snapshot_is_written_once_and_never_overwritten() {
        let (pool, _d) = test_pool().await;
        let first = CreditDay {
            day: "2026-07-28".into(),
            at: 1_000,
            balance: 12_833,
            spent_since_prev: 0,
            delta: None,
        };
        assert!(insert_credit_day(&pool, &first).await.unwrap());
        let second = CreditDay {
            day: "2026-07-28".into(),
            at: 50_000,
            balance: 9_000,
            spent_since_prev: 3_833,
            delta: Some(0),
        };
        assert!(
            !insert_credit_day(&pool, &second).await.unwrap(),
            "同一天的第二次写必须是 no-op"
        );
        let got = credit_day(&pool, "2026-07-28").await.unwrap().unwrap();
        assert_eq!(got.balance, 12_833, "留下的是当天第一次那个余额");
        assert!(
            got.delta.is_none(),
            "首条没有对比基准，delta 必须留空而不是 0"
        );

        // 第二天：delta = 余额差 + 期间本机花掉，正是「凭空进来了多少」。
        insert_credit_day(
            &pool,
            &CreditDay {
                day: "2026-07-29".into(),
                at: 90_000,
                balance: 12_113,
                spent_since_prev: 800,
                delta: Some(12_113 - 12_833 + 800),
            },
        )
        .await
        .unwrap();
        let latest = latest_credit_day(&pool).await.unwrap().unwrap();
        assert_eq!(latest.day, "2026-07-29");
        assert_eq!(
            latest.delta,
            Some(80),
            "花掉的加回来之后，剩下的就是每日进账"
        );
    }

    // 采样保留期是观测窗口：到期的裁掉，窗口内的一个都不能少。
    #[tokio::test]
    async fn pruning_samples_only_removes_what_fell_out_of_the_window() {
        let (pool, _d) = test_pool().await;
        let id = seed_reviewable(&pool, 1).await;
        for at in [100, 5_000, 9_000] {
            record_queue_sample(&pool, id, at, 4_485 - at, Some(574_522))
                .await
                .unwrap();
        }
        assert_eq!(prune_queue_samples(&pool, 5_000).await.unwrap(), 1);
        let left = queue_samples_of(&pool, id).await.unwrap();
        assert_eq!(left.len(), 2);
        assert_eq!(left[0].at, 5_000, "边界上那条属于窗口内");
    }

    // 采样随 clip 一起走：clip 删了，它的轨迹没有单独留存的意义，
    // 留下来只会变成一堆认不出主人的孤儿行。
    #[tokio::test]
    async fn samples_are_removed_with_their_clip() {
        let (pool, _d) = test_pool().await;
        let id = seed_reviewable(&pool, 1).await;
        record_queue_sample(&pool, id, 100, 4_485, None)
            .await
            .unwrap();
        assert_eq!(queue_samples_since(&pool, 0).await.unwrap().len(), 1);
        remove(&pool, &[id]).await.unwrap();
        assert!(queue_samples_since(&pool, 0).await.unwrap().is_empty());
    }

    // 手动刷新那条进度条的分母。谓词必须与 `list_running` 一字不差 —— 差了的话，
    // 按钮上会显示「正在查 8/12」然后停在 8，而人会以为它卡住了。
    #[tokio::test]
    async fn count_running_matches_list_running_exactly() {
        let (pool, _d) = test_pool().await;
        assert_eq!(count_running(&pool).await.unwrap(), 0);
        seed_work(&pool, 1).await;
        enqueue_one(&pool, 1).await;
        let id = list_by_stages(&pool, &["rewrite"]).await.unwrap()[0].id;
        let mut tx = pool.begin().await.unwrap();
        apply_rewrite(&mut tx, id, "视频提示词", None, None, None, 200)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        mark_submitted(&pool, id, &SubmitReceipt::healthy("sub-1", 8), 500)
            .await
            .unwrap();
        assert_eq!(count_running(&pool).await.unwrap(), 1);
        assert_eq!(list_running(&pool).await.unwrap().len(), 1);
        // run 但**没有** submit_id（= 提交到一半被杀的孤儿）两边都必须同时不数它：
        // 退回 ready 再认领一次，就停在「已认领、还没拿到 submit_id」那一格。
        assert!(requeue_for_run(&pool, id, Some("sub-1"), 700)
            .await
            .unwrap());
        assert_eq!(claim_ready(&pool, &[id], 700).await.unwrap().len(), 1);
        assert_eq!(count_running(&pool).await.unwrap(), 0);
        assert_eq!(list_running(&pool).await.unwrap().len(), 0);
    }

    /// 丢弃已提交单时**只丢读到的那一单**。
    ///
    /// 人在「仍要重跑」那张卡上犹豫的几秒里，本地待发队列完全可能把这一条重新发出去。
    /// 没有这道闸的话，丢掉的就不是人看过的那一单，而是一单他还没来得及知道的新提交
    /// —— 而两者都已经扣过钱。
    #[tokio::test]
    async fn discarding_a_submit_only_touches_the_one_that_was_read() {
        let (pool, _d) = test_pool().await;
        seed_work(&pool, 1).await;
        enqueue_one(&pool, 1).await;
        let id = list_by_stages(&pool, &["rewrite"]).await.unwrap()[0].id;
        let mut tx = pool.begin().await.unwrap();
        apply_rewrite(&mut tx, id, "视频提示词", None, None, None, 200)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        mark_submitted(&pool, id, &SubmitReceipt::healthy("sub-new", 8), 600)
            .await
            .unwrap();

        assert!(
            !requeue_for_run(&pool, id, Some("sub-old"), 700)
                .await
                .unwrap(),
            "读到的是 sub-old，而这一行已经是 sub-new —— 一行都不许动"
        );
        let row = get(&pool, id).await.unwrap().unwrap();
        assert_eq!(row.stage, "run");
        assert_eq!(row.submit_id.as_deref(), Some("sub-new"));

        // 指名到当前那一单就放行。
        assert!(requeue_for_run(&pool, id, Some("sub-new"), 800)
            .await
            .unwrap());
        assert_eq!(get(&pool, id).await.unwrap().unwrap().stage, "ready");
    }

    /// 换通道**不能动 `submit_queued_at`**。
    ///
    /// 它是本地队列的排序键（`pick_submit_queued_on` 是 `ORDER BY submit_queued_at, id`）。
    /// 若换通道时把它刷成当下，一条排了两小时的条目换条队就会被罚到新队的队尾 ——
    /// 而人做这个动作的全部动机恰恰是「别再等了」。
    #[tokio::test]
    async fn switching_channel_keeps_its_place_in_line() {
        let (pool, _d) = test_pool().await;
        seed_work(&pool, 1).await;
        enqueue_one(&pool, 1).await;
        let id = list_by_stages(&pool, &["rewrite"]).await.unwrap()[0].id;
        let mut tx = pool.begin().await.unwrap();
        apply_rewrite(&mut tx, id, "视频提示词", None, None, None, 200)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        mark_submit_queued(&pool, &[id], 300).await.unwrap();

        set_params(
            &pool,
            &[id],
            Some("seedance2.0mini"),
            Some(5),
            Some("720p"),
            900,
        )
        .await
        .unwrap();

        let after = get(&pool, id).await.unwrap().unwrap();
        assert_eq!(after.model_version.as_deref(), Some("seedance2.0mini"));
        assert_eq!(
            after.submit_queued_at,
            Some(300),
            "换通道后仍按**原放行时刻**插进新通道的队，不被罚到队尾"
        );
        assert_eq!(after.stage, "ready", "换通道不改阶段");
        assert_eq!(count_submit_queued(&pool).await.unwrap(), 1);
    }

    /// `set_params` 碰不到已提交的条目 —— 这是「换通道不花钱」那条路径的底线。
    ///
    /// 已经在即梦手上的那条要换通道，只能先 `requeue_for_run` 丢弃提交单（花第二份钱），
    /// 而那是一个人必须在确认框里点过头的动作。若 `set_params` 能直接改 `run` 态，
    /// 库里的参数就会与那条视频**实际用的**参数对不上，而账还记在原来的通道上。
    #[tokio::test]
    async fn set_params_refuses_to_touch_submitted_clips() {
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

        let n = set_params(
            &pool,
            &[id],
            Some("seedance2.0mini"),
            Some(5),
            Some("720p"),
            900,
        )
        .await
        .unwrap();
        assert_eq!(n, 0, "已提交的条目一行都不该被改到");
        let after = get(&pool, id).await.unwrap().unwrap();
        assert_eq!(after.stage, "run");
        assert!(after.model_version.is_none());
        assert_eq!(after.submit_id.as_deref(), Some("sub-1"));
    }
}
