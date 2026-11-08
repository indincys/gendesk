//! 待改写队列与 Agent 的一次往返：`gendesk v2v pending` 取工单，`gendesk v2v rewrite` 交回。
//!
//! 状态全在 `v2v_clips`：这里不写交接目录、不监听文件，CLI 经本机控制通道直接读写。
//! 一组 = 一个提示词组：同组分镜最后要剪进同一条成片，运镜语言与时长必须统一。

use std::path::Path;

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::db::now_unix;
use crate::db::repo::v2v as repo;
use crate::error::AppResult;

/// 待改写的一条（给 Agent 读）。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PendingItem {
    /// 主键 `W{work_id}`。**Agent 必须原样回传**。
    pub id: String,
    pub work_id: i64,
    pub clip_id: i64,
    pub prompt_code: String,
    pub batch_id: Option<i64>,
    /// 缩略图绝对路径。**只给缩略图不给原图**：384×512 约 260 token，
    /// 比原图省一个量级，而原图由 GenDesk 自己留着喂即梦 `--image`。
    pub thumb: String,
    /// 生图提示词全文（快照）。
    pub source_prompt: String,
    /// 剥掉组内公共前后缀后的可变部分——场景/构图/动势，改写的真正素材。
    pub variable_part: String,
}

/// 一组待改写。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PendingGroup {
    pub group_id: Option<i64>,
    pub group_name: String,
    pub stripped_prefix_chars: usize,
    pub stripped_suffix_chars: usize,
    /// 本组改写说明（给 Agent 的任务书）。
    pub guide: String,
    pub items: Vec<PendingItem>,
}

/// 当前待改写队列。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Pending {
    pub groups: Vec<PendingGroup>,
    /// 缩略图缺失而无法交给 Agent 的条目数（父图被清理掉了）。阶段不变，看板上仍可见。
    pub skipped: i64,
}

/// `W123` → 123。非该形状返回 None。
pub fn parse_item_id(id: &str) -> Option<i64> {
    id.strip_prefix('W')
        .or_else(|| id.strip_prefix('w'))
        .and_then(|n| n.trim().parse().ok())
}

/// 一条改写结果指向哪个 clip：clipId 优先，其次 workId，其次 `W{id}`。
///
/// **绝不按文件名匹配**：输出名里的编号已去连字符，`BR140010` 反推不出是
/// `BR14-0010` 还是 `BR1-40010`——文件名本来就不可逆（v0.13.0 已论证）。
pub fn resolve_target(line: &RewriteLine) -> Option<(Option<i64>, Option<i64>)> {
    if let Some(cid) = line.clip_id {
        return Some((Some(cid), None));
    }
    if let Some(wid) = line.work_id {
        return Some((None, Some(wid)));
    }
    line.id
        .as_deref()
        .and_then(parse_item_id)
        .map(|wid| (None, Some(wid)))
}

/// Agent 交回的一条改写结果。
///
/// 入参宽容、出参严格：camelCase 与 snake_case 都收，`id`/`workId`/`clipId` 任一都能定位。
/// 理由很实际——Agent 是另一个人（或另一个模型）写的，为一处大小写差异让整批静默失败
/// 是最不值得的失败方式。
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct RewriteLine {
    pub id: Option<String>,
    #[serde(alias = "work_id")]
    pub work_id: Option<i64>,
    #[serde(alias = "clip_id")]
    pub clip_id: Option<i64>,
    #[serde(alias = "video_prompt", alias = "prompt")]
    pub video_prompt: Option<String>,
    #[serde(alias = "model_version", alias = "model")]
    pub model_version: Option<String>,
    pub duration: Option<i64>,
    #[serde(alias = "video_resolution", alias = "resolution")]
    pub video_resolution: Option<String>,
}

/// 列出待改写队列（按组）。只读：不改任何条目的阶段。
pub async fn pending(pool: &SqlitePool) -> AppResult<Pending> {
    let clips = repo::list_by_stages(pool, &["rewrite"]).await?;
    let now = now_unix();
    let mut order: Vec<(Option<i64>, String)> = Vec::new();
    let mut buckets: Vec<Vec<repo::ClipRow>> = Vec::new();
    for c in clips {
        let key = (c.group_id, c.group_name.clone());
        match order.iter().position(|k| *k == key) {
            Some(i) => buckets[i].push(c),
            None => {
                order.push(key);
                buckets.push(vec![c]);
            }
        }
    }
    let mut out = Pending::default();
    for ((group_id, group_name), rows) in order.into_iter().zip(buckets) {
        // 公共前后缀取自该组**全部**验收作品，而非本次待改写的这几条：超集的公共缀
        // 必然是子集公共缀的前缀，取超集更保守——宁可少剥，不可把场景描述剥掉。
        let corpus = group_prompt_corpus(pool, group_id).await?;
        let (pre, suf) = super::common_affixes(&corpus);
        let mut items = Vec::new();
        for c in rows {
            if c.thumb_path.is_empty() || !Path::new(&c.thumb_path).is_file() {
                out.skipped += 1;
                continue;
            }
            let variable = super::variable_part(&c.source_prompt, pre, suf);
            // 顺手把可变部分落库：看板读它，不必每次重算。
            repo::set_variable_part(pool, c.id, &variable, now).await?;
            items.push(PendingItem {
                id: format!("W{}", c.work_id),
                work_id: c.work_id,
                clip_id: c.id,
                prompt_code: c.prompt_code.clone(),
                batch_id: c.batch_id,
                thumb: c.thumb_path.clone(),
                source_prompt: c.source_prompt.clone(),
                variable_part: variable,
            });
        }
        if items.is_empty() {
            continue;
        }
        out.groups.push(PendingGroup {
            guide: guide(&group_name, items.len(), pre, suf),
            group_id,
            group_name,
            stripped_prefix_chars: pre,
            stripped_suffix_chars: suf,
            items,
        });
    }
    Ok(out)
}

/// 该组**全部**验收作品的提示词全文（公共前后缀的取样基准）。
async fn group_prompt_corpus(pool: &SqlitePool, group_id: Option<i64>) -> AppResult<Vec<String>> {
    let rows: Vec<(String,)> = match group_id {
        Some(g) => {
            sqlx::query_as("SELECT prompt_text FROM accepted_works WHERE group_id = ?1 ORDER BY id")
                .bind(g)
                .fetch_all(pool)
                .await?
        }
        None => {
            sqlx::query_as(
                "SELECT prompt_text FROM accepted_works WHERE group_id IS NULL ORDER BY id",
            )
            .fetch_all(pool)
            .await?
        }
    };
    Ok(rows.into_iter().map(|(t,)| t).collect())
}

fn guide(group_name: &str, count: usize, pre: usize, suf: usize) -> String {
    format!(
        "# 改写工单 · {group_name}\n\n\
         共 {n} 条。一条 = 一张验收通过的首帧图 + 它的**生图**提示词。\n\n\
         ## 你的唯一任务\n\n\
         把每条的生图提示词改写成**图生视频**提示词，整理成 JSON Lines 文件后运行\n\
         `gendesk v2v rewrite --file <文件>` 交回 GenDesk。一行一条：\n\n\
         ```json\n\
         {{\"id\":\"W123\",\"videoPrompt\":\"首帧自然延续：…\"}}\n\
         ```\n\n\
         可选字段：`modelVersion` `duration` `videoResolution`（不给则用设置里的默认值）。\n\n\
         **`id` 必须原样回传**（就是 `gendesk v2v pending` 给出的那个 `W…`）。不要按文件名匹配——\n\
         输出文件名里的编号已去连字符，本来就不可逆。\n\n\
         ## 改写要点\n\n\
         官方公式「主体+运动+环境+运镜+美学描述」，图生视频**略掉主体与环境的外观描写**\n\
         （首帧图已定死），只写：运动 + 运镜 + 约束。中文 150–250 字，按四段写：\n\n\
         1. 开头固定「首帧自然延续：」\n\
         2. **运动**：人物/宠物/背景/光影可以动，按时序写，用程度副词限定幅度\n\
         （极缓、缓慢、轻微、连续不停顿）\n\
         3. **运镜**：只给一种（官方词表：推/拉/摇/移/跟/升/降/甩/环绕/旋转/变焦），\n\
         或写「镜头固定不动」；再加「带轻微手持的呼吸感」\n\
         4. **约束**：产品（卡套与每个扣环/铃铛/挂绳）**逐项点名**完全静止、\n\
         形状比例材质保持首帧原样、**不发生形变**；构图/色彩/光线与首帧一致；\n\
         画面稳定无切镜；只有首帧已有的物体；延续手机实拍的真实质感\n\n\
         动静分层是核心：**真实场景元素（人、宠物、路人、光影、机位）可以动**，\n\
         那是真实感的来源；**产品本体必须锁死且绝不形变**。\n\n\
         ## 不要做的事\n\n\
         - 不要调 `dreamina`。提交/轮询/下载/重试由 GenDesk 做，它有状态机和崩溃恢复。\n\
         - 不要直接改 GenDesk 的数据库或文件。\n\
         - 不要让产品自己动（旋转展示/被拿起）；不要堆运镜；不要切镜；\n\
         不要写「高级质感/电影级打光」这类广告词。\n\
         - 不要照抄生图提示词里那段产品保真模板（图已经画对了，再喂一遍会把改写挤偏）。\n\n\
         ## 条目字段\n\n\
         - `thumb` 缩略图绝对路径（看图用；原图 GenDesk 自己留着喂即梦）\n\
         - `sourcePrompt` 生图提示词全文\n\
         - `variablePart` 剥掉组内公共前后缀后的可变部分（本组剥前 {pre} 字、后 {suf} 字），\
         即场景/构图/动势——改写的真正素材\n\
         - 剥离只是提示不是契约；拿不准就读 `sourcePrompt` 全文\n",
        n = count,
    )
}

/// 收录摘要。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IngestSummary {
    pub applied: i64,
    /// 认不出目标（缺 id / 编号对不上任何在队条目）的行数。
    pub unmatched: i64,
    /// 目标已越过待提交阶段（已提交/已出片）而被拒绝的行数。
    pub stale: i64,
}

/// 应用 Agent 交回的改写结果。逐行容错：一行认不出不影响其余行。
pub async fn apply(pool: &SqlitePool, lines: &[RewriteLine]) -> AppResult<IngestSummary> {
    let mut sum = IngestSummary::default();
    let now = now_unix();
    for line in lines {
        let Some(prompt) = line
            .video_prompt
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            sum.unmatched += 1;
            continue;
        };
        let Some((clip_id, work_id)) = resolve_target(line) else {
            sum.unmatched += 1;
            continue;
        };
        let Some(clip) = find_clip(pool, clip_id, work_id).await? else {
            sum.unmatched += 1;
            continue;
        };
        let mut tx = pool.begin().await?;
        let ok = repo::apply_rewrite(
            &mut tx,
            clip,
            prompt,
            line.model_version.as_deref(),
            line.duration,
            line.video_resolution.as_deref(),
            now,
        )
        .await?;
        tx.commit().await?;
        if ok {
            sum.applied += 1;
        } else {
            sum.stale += 1;
        }
    }
    Ok(sum)
}

/// 按 clip_id 或 work_id 定位一条 clip 的 id。
async fn find_clip(
    pool: &SqlitePool,
    clip_id: Option<i64>,
    work_id: Option<i64>,
) -> AppResult<Option<i64>> {
    let row: Option<(i64,)> = match (clip_id, work_id) {
        (Some(c), _) => {
            sqlx::query_as("SELECT id FROM v2v_clips WHERE id = ?1")
                .bind(c)
                .fetch_optional(pool)
                .await?
        }
        (None, Some(w)) => {
            sqlx::query_as("SELECT id FROM v2v_clips WHERE work_id = ?1")
                .bind(w)
                .fetch_optional(pool)
                .await?
        }
        _ => None,
    };
    Ok(row.map(|(id,)| id))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即失败
mod tests {
    use super::*;
    use crate::db::test_support::test_pool;
    use crate::v2v::dreamina::SubmitReceipt;

    async fn seed(
        pool: &SqlitePool,
        work_id: i64,
        group_id: i64,
        prompt: &str,
        thumb: &str,
    ) -> i64 {
        sqlx::query("INSERT OR IGNORE INTO prompt_groups (id,name,prefix,scene,is_temp,created_at) VALUES (?1,?2,'GG','',0,0)")
            .bind(group_id).bind(format!("组{group_id}")).execute(pool).await.unwrap();
        sqlx::query("INSERT OR IGNORE INTO prompts (id,group_id,code,text,status,source,created_at,updated_at) VALUES (?1,?2,?3,'t','active','library',0,0)")
            .bind(work_id).bind(group_id).bind(format!("GG-{work_id:04}")).execute(pool).await.unwrap();
        sqlx::query("INSERT INTO accepted_works (id,image_path,thumb_path,prompt_id,prompt_text,group_id,batch_id,accepted_at) VALUES (?1,'/img.jpg',?2,?1,?3,?4,7,100)")
            .bind(work_id).bind(thumb).bind(prompt).bind(group_id)
            .execute(pool).await.unwrap();
        let mut tx = pool.begin().await.unwrap();
        repo::enqueue(
            &mut tx,
            work_id,
            Some(group_id),
            &format!("组{group_id}"),
            Some(7),
            prompt,
            100,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        work_id
    }

    fn touch_thumb(dir: &Path, name: &str) -> String {
        let p = dir.join(name);
        std::fs::write(&p, b"\xff\xd8thumb").unwrap();
        p.to_string_lossy().to_string()
    }

    // skill 是别人写的：大小写与字段别名都得收，否则一处差异让整批静默失败。
    #[test]
    fn rewrite_line_accepts_both_casings_and_aliases() {
        let camel: RewriteLine = serde_json::from_str(
            r#"{"id":"W12","videoPrompt":"镜头缓推","modelVersion":"seedance2.0fast","duration":5,"videoResolution":"720p"}"#,
        )
        .unwrap();
        assert_eq!(camel.video_prompt.as_deref(), Some("镜头缓推"));
        assert_eq!(camel.model_version.as_deref(), Some("seedance2.0fast"));
        assert_eq!(camel.video_resolution.as_deref(), Some("720p"));

        let snake: RewriteLine = serde_json::from_str(
            r#"{"work_id":12,"video_prompt":"镜头缓推","model_version":"seedance2.0","resolution":"720p"}"#,
        )
        .unwrap();
        assert_eq!(snake.video_prompt.as_deref(), Some("镜头缓推"));
        assert_eq!(snake.work_id, Some(12));
        assert_eq!(snake.video_resolution.as_deref(), Some("720p"));

        // 只写 prompt 也认（skill 作者最容易顺手写成这个）。
        let plain: RewriteLine = serde_json::from_str(r#"{"id":"W1","prompt":"x"}"#).unwrap();
        assert_eq!(plain.video_prompt.as_deref(), Some("x"));
    }

    #[test]
    fn item_id_roundtrip_and_rejects_junk() {
        assert_eq!(parse_item_id("W123"), Some(123));
        assert_eq!(parse_item_id("w7"), Some(7));
        assert_eq!(parse_item_id("BR140010"), None, "不得把去连字符的编号当 id");
        assert_eq!(parse_item_id(""), None);
    }

    // clipId 优先于 workId 优先于 W{id}。
    #[test]
    fn target_resolution_prefers_explicit_clip_id() {
        let l = RewriteLine {
            id: Some("W9".into()),
            work_id: Some(8),
            clip_id: Some(7),
            ..Default::default()
        };
        assert_eq!(resolve_target(&l), Some((Some(7), None)));
        let l = RewriteLine {
            id: Some("W9".into()),
            work_id: Some(8),
            ..Default::default()
        };
        assert_eq!(resolve_target(&l), Some((None, Some(8))));
        let l = RewriteLine {
            id: Some("W9".into()),
            ..Default::default()
        };
        assert_eq!(resolve_target(&l), Some((None, Some(9))));
        assert_eq!(resolve_target(&RewriteLine::default()), None);
    }

    // 端到端一轮：列出待改写 → 交回改写 → 条目进待提交，队列随之清空。
    #[tokio::test]
    async fn pending_then_apply_moves_clips_to_ready() {
        let (pool, _d) = test_pool().await;
        let tmp = tempfile::tempdir().unwrap();
        let t1 = touch_thumb(tmp.path(), "a.jpg");
        let t2 = touch_thumb(tmp.path(), "b.jpg");
        seed(
            &pool,
            1,
            5,
            "保留产品原样。背景换成屋顶花园。像素级完整保留。",
            &t1,
        )
        .await;
        seed(
            &pool,
            2,
            5,
            "保留产品原样。背景换成宠物餐吧。像素级完整保留。",
            &t2,
        )
        .await;

        let p = pending(&pool).await.unwrap();
        assert_eq!(p.groups.len(), 1);
        assert_eq!(p.skipped, 0);
        let g = &p.groups[0];
        assert_eq!(g.group_id, Some(5));
        assert_eq!(g.items.len(), 2);
        assert_eq!(g.items[0].id, "W1");
        assert_eq!(g.items[0].thumb, t1, "缩略图给绝对路径");
        assert!(g.items[0].variable_part.contains("屋顶花园"));
        assert!(
            !g.items[0].variable_part.contains("像素级完整保留"),
            "组内公共尾巴须剥掉"
        );
        assert!(g.guide.contains("gendesk v2v rewrite"));

        let lines: Vec<RewriteLine> = serde_json::from_str(
            r#"[{"id":"W1","videoPrompt":"镜头缓推"},{"id":"W2","videoPrompt":"光斑掠过","modelVersion":"seedance2.0fast","duration":5}]"#,
        )
        .unwrap();
        let sum = apply(&pool, &lines).await.unwrap();
        assert_eq!((sum.applied, sum.unmatched, sum.stale), (2, 0, 0));
        let ready = repo::list_by_stages(&pool, &["ready"]).await.unwrap();
        assert_eq!(ready.len(), 2);
        assert_eq!(ready[0].video_prompt.as_deref(), Some("镜头缓推"));
        assert_eq!(ready[1].model_version.as_deref(), Some("seedance2.0fast"));
        assert!(pending(&pool).await.unwrap().groups.is_empty());
    }

    // 缺缩略图的条目跳过但**不改阶段**：静默丢弃比留在待改写更糟。
    #[tokio::test]
    async fn missing_thumb_is_skipped_without_losing_the_clip() {
        let (pool, _d) = test_pool().await;
        seed(&pool, 1, 5, "提示词", "/nonexistent/gone.jpg").await;
        let p = pending(&pool).await.unwrap();
        assert!(p.groups.is_empty());
        assert_eq!(p.skipped, 1);
        assert_eq!(
            repo::list_by_stages(&pool, &["rewrite"])
                .await
                .unwrap()
                .len(),
            1,
            "条目须留在待改写，人能在看板上看见它卡着"
        );
    }

    // 认不出目标的行不得静默吞掉，要计数上报。
    #[tokio::test]
    async fn unmatched_and_empty_lines_are_counted_not_swallowed() {
        let (pool, _d) = test_pool().await;
        let lines: Vec<RewriteLine> = serde_json::from_str(
            r#"[{"id":"W999","videoPrompt":"没有这条"},{"videoPrompt":"没有 id"},{"id":"W1"}]"#,
        )
        .unwrap();
        let sum = apply(&pool, &lines).await.unwrap();
        assert_eq!(sum.applied, 0);
        assert_eq!(sum.unmatched, 3, "找不到目标/缺 id/缺提示词 各计一次");
    }

    // 迟到的改写结果不得把已提交的条目打回（白烧额度），要计入 stale。
    #[tokio::test]
    async fn late_rewrite_against_submitted_clip_is_reported_stale() {
        let (pool, _d) = test_pool().await;
        let tmp = tempfile::tempdir().unwrap();
        let t = touch_thumb(tmp.path(), "a.jpg");
        seed(&pool, 1, 5, "提示词", &t).await;
        let id = repo::list_by_stages(&pool, &["rewrite"]).await.unwrap()[0].id;
        let mut tx = pool.begin().await.unwrap();
        repo::apply_rewrite(&mut tx, id, "第一版", None, None, None, 200)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        repo::mark_submitted(&pool, id, &SubmitReceipt::healthy("sub-1", 8), 300)
            .await
            .unwrap();
        let lines: Vec<RewriteLine> =
            serde_json::from_str(r#"[{"id":"W1","videoPrompt":"第二版"}]"#).unwrap();
        let sum = apply(&pool, &lines).await.unwrap();
        assert_eq!((sum.applied, sum.stale), (0, 1));
        let row = repo::get(&pool, id).await.unwrap().unwrap();
        assert_eq!(row.stage, "run");
        assert_eq!(row.video_prompt.as_deref(), Some("第一版"));
    }
}
