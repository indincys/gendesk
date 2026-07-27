//! 交接目录 —— GenDesk 与 Claude Code / Codex 侧改写 skill 的一次往返。
//!
//! ```text
//! <交接根>/v2v/待改写/index.jsonl          ← 当前待改写的组清单（skill 从这里起步）
//! <交接根>/v2v/待改写/<组>/manifest.jsonl   ← 一行一条：缩略图 + 生图提示词 + 可变部分
//! <交接根>/v2v/待改写/<组>/thumbs/W123.jpg
//! <交接根>/v2v/待改写/<组>/READY.txt        ← 最后写；skill 只认带它的目录
//! <交接根>/v2v/已改写/<组>/rewrite.jsonl    ← skill 写回，本模块监听收录
//! <交接根>/v2v/已改写/_已收录/…             ← 收录后移档留证
//! ```
//!
//! ## 三条设计决定
//!
//! **1. 队列非空即自动物化，不等任何按钮。** 「验收通过后不需要点导出」这句话要成立，
//! 就必须有人在队列变化时把工单写到磁盘上。那个人是本模块，触发点是验收命令与收录命令。
//!
//! **2. 目录名对同一组恒定（`g{group_id}`）。** 若每次物化都生成带时间戳的新目录，
//! skill 每轮都会看到一个「没见过的」目录，重复改写同一批。恒定目录 + 每次全量重写
//! = 天然幂等，也让「已经改写完的组从待改写里消失」这件事可以靠删目录表达。
//!
//! **3. 交接目录不是状态的持有者。** 它是信箱，不是账本。真相在 `v2v_clips`。
//! 故这里没有 v0.13.0 那份 `ledger.jsonl`——少一个真相来源是收益，不是损失。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::db::now_unix;
use crate::db::repo::v2v as repo;
use crate::error::{AppError, AppResult};
use crate::publish::paths::ascii_slug;

/// 交接根下的固定子目录名。skill 侧写死这几个名字，故它们是契约，不可改。
pub const V2V: &str = "v2v";
pub const PENDING: &str = "待改写";
pub const DONE: &str = "已改写";
pub const CONSUMED: &str = "_已收录";
pub const INDEX: &str = "index.jsonl";
pub const MANIFEST: &str = "manifest.jsonl";
pub const REWRITE: &str = "rewrite.jsonl";
pub const READY: &str = "READY.txt";

/// 默认交接根：`<用户主目录>/GenDesk交接`。
///
/// **故意不放在应用数据目录里**：那个路径在 macOS 上是
/// `~/Library/Application Support/com.…/`，随 bundle id 变化且不可预测，而 skill 要把
/// 路径写死才能做到「什么都不用输入」。主目录下的固定名字是唯一能同时满足
/// 「跨平台稳定」「用户找得到」「skill 写得死」的位置。
pub fn default_root() -> PathBuf {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join("GenDesk交接")
}

/// 组目录名：`g{group_id}` 或 `g{group_id}-{ascii-slug}`。
///
/// 带上 group_id 是为了**恒定且唯一**——组可以改名（改名不该让 skill 重做一遍），
/// 也可以有两个组 slug 相同（纯中文名一律退化成 `x`）。slug 只是给人看的尾巴。
pub fn group_dir_name(group_id: Option<i64>, group_name: &str) -> String {
    let id = group_id.unwrap_or(0);
    let slug = ascii_slug(group_name);
    if slug == "x" {
        format!("g{id}")
    } else {
        format!("g{id}-{slug}")
    }
}

/// 工单条目（写给 skill 读）。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JobItem {
    /// 包内主键 `W{work_id}`。**skill 必须原样回传**。
    pub id: String,
    pub work_id: i64,
    pub clip_id: i64,
    pub prompt_code: String,
    pub group_name: String,
    pub batch_id: Option<i64>,
    /// 组内相对路径的缩略图。**只给缩略图不给原图**：384×512 约 260 token，
    /// 比原图省一个量级，而原图由 GenDesk 自己留着喂即梦 `--image`。
    pub thumb: String,
    /// 生图提示词全文（快照）。
    pub source_prompt: String,
    /// 剥掉组内公共前后缀后的可变部分——场景/构图/动势，改写的真正素材。
    pub variable_part: String,
    pub stripped_prefix_chars: usize,
    pub stripped_suffix_chars: usize,
}

/// index.jsonl 的一行（组清单）。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JobGroup {
    pub dir: String,
    pub group_id: Option<i64>,
    pub group_name: String,
    pub count: usize,
}

/// skill 写回的一条改写结果。
///
/// 入参宽容、出参严格：camelCase 与 snake_case 都收，`id`/`workId`/`clipId` 任一都能定位。
/// 理由很实际——skill 是另一个人（或另一个模型）写的，为一处大小写差异让整批静默失败
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

/// 物化结果摘要。
#[derive(Debug, Clone, Default, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct MaterializeSummary {
    /// 工单根目录（待改写）绝对路径 —— 前端把它显示出来供用户复制给 skill。
    pub pending_dir: String,
    pub groups: i64,
    pub items: i64,
    /// 缩略图缺失而无法写进工单的条目数（父图被清理掉了）。
    pub skipped: i64,
}

fn write_atomic(path: &Path, contents: &str) -> AppResult<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, contents).map_err(|e| AppError::Io(e.to_string()))?;
    std::fs::rename(&tmp, path).map_err(|e| AppError::Io(e.to_string()))?;
    Ok(())
}

/// 把「待改写」队列全量物化到磁盘（幂等；每次全量重写并清理已不在队列的组目录）。
pub async fn materialize(pool: &SqlitePool, root: &Path) -> AppResult<MaterializeSummary> {
    let pending_root = root.join(V2V).join(PENDING);
    std::fs::create_dir_all(&pending_root).map_err(|e| AppError::Io(e.to_string()))?;
    // 已改写目录一并先建好：skill 不必判断存在性，人也能一眼看懂往哪写。
    std::fs::create_dir_all(root.join(V2V).join(DONE)).map_err(|e| AppError::Io(e.to_string()))?;

    let clips = repo::list_by_stages(pool, &["rewrite"]).await?;
    let now = now_unix();

    // 按组分堆。**一包一组**：同组分镜最后要剪进同一条成片，运镜语言与时长必须统一，
    // 跨组混一个工单改写风格会飘。
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

    let mut summary = MaterializeSummary {
        pending_dir: pending_root.to_string_lossy().to_string(),
        ..Default::default()
    };
    let mut index: Vec<JobGroup> = Vec::new();
    let mut live_dirs: Vec<String> = Vec::new();

    for ((group_id, group_name), rows) in order.iter().zip(buckets.iter()) {
        let dir_name = group_dir_name(*group_id, group_name);
        let dir = pending_root.join(&dir_name);
        std::fs::create_dir_all(dir.join("thumbs")).map_err(|e| AppError::Io(e.to_string()))?;

        // 公共前后缀取自该组**全部**验收作品，而非本次待改写的这几条：超集的公共缀
        // 必然是子集公共缀的前缀，取超集更保守——宁可少剥，不可把场景描述剥掉。
        let corpus = group_prompt_corpus(pool, *group_id).await?;
        let (pre, suf) = super::common_affixes(&corpus);

        let mut items: Vec<JobItem> = Vec::new();
        for c in rows {
            let thumb_name = format!("W{}.jpg", c.work_id);
            if c.thumb_path.is_empty() || !Path::new(&c.thumb_path).is_file() {
                // 缩略图没了（父图被彻底删除）→ 这条进不了工单，但**不改它的阶段**：
                // 静默丢弃比留在待改写里更糟，人至少能在看板上看见它还卡着。
                summary.skipped += 1;
                continue;
            }
            // 目标已存在就跳过：物化是自动的（队列一变就重写一遍），而缩略图是
            // 只读的输入拷贝 —— 每轮把同一批图重拷一遍纯属白干，条目多时还很响。
            let thumb_dest = dir.join("thumbs").join(&thumb_name);
            if !thumb_dest.is_file() {
                std::fs::copy(&c.thumb_path, &thumb_dest)
                    .map_err(|e| AppError::Io(e.to_string()))?;
            }
            let variable = super::variable_part(&c.source_prompt, pre, suf);
            // 顺手把可变部分落库：看板与「重新物化」都读它，不必每次重算。
            repo::set_variable_part(pool, c.id, &variable, now).await?;
            items.push(JobItem {
                id: format!("W{}", c.work_id),
                work_id: c.work_id,
                clip_id: c.id,
                prompt_code: c.prompt_code.clone(),
                group_name: group_name.clone(),
                batch_id: c.batch_id,
                thumb: format!("thumbs/{thumb_name}"),
                source_prompt: c.source_prompt.clone(),
                variable_part: variable,
                stripped_prefix_chars: pre,
                stripped_suffix_chars: suf,
            });
        }
        if items.is_empty() {
            continue;
        }

        let mut manifest = String::new();
        for it in &items {
            manifest.push_str(
                &serde_json::to_string(it)
                    .map_err(|e| AppError::Internal(format!("manifest 序列化失败：{e}")))?,
            );
            manifest.push('\n');
        }
        write_atomic(&dir.join(MANIFEST), &manifest)?;
        write_atomic(
            &dir.join("改写说明.md"),
            &readme(group_name, &items, pre, suf),
        )?;
        // READY.txt **最后写**：skill 只认带 READY.txt 的目录，写到一半的工单
        // （磁盘满 / 进程被杀）不会被当成可执行输入。
        write_atomic(&dir.join(READY), &format!("{} 条待改写\n", items.len()))?;

        summary.groups += 1;
        summary.items += items.len() as i64;
        index.push(JobGroup {
            dir: dir_name.clone(),
            group_id: *group_id,
            group_name: group_name.clone(),
            count: items.len(),
        });
        live_dirs.push(dir_name);
    }

    // 清理已不在待改写队列的组目录：不清就等于让 skill 反复改写已经改完的那批，
    // 每轮都白花一遍上下文。
    if let Ok(entries) = std::fs::read_dir(&pending_root) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if e.path().is_dir() && !live_dirs.contains(&name) {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }

    let mut index_body = String::new();
    for g in &index {
        index_body.push_str(
            &serde_json::to_string(g)
                .map_err(|e| AppError::Internal(format!("index 序列化失败：{e}")))?,
        );
        index_body.push('\n');
    }
    write_atomic(&pending_root.join(INDEX), &index_body)?;
    Ok(summary)
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

fn readme(group_name: &str, items: &[JobItem], pre: usize, suf: usize) -> String {
    format!(
        "# 改写工单 · {group_name}\n\n\
         共 {n} 条。一条 = 一张验收通过的首帧图 + 它的**生图**提示词。\n\n\
         ## 你的唯一任务\n\n\
         把每条的生图提示词改写成**图生视频**提示词，写回：\n\n\
         ```\n../../{done}/<本目录名>/{rewrite}\n```\n\n\
         一行一条 JSON：\n\n\
         ```json\n\
         {{\"id\":\"W123\",\"videoPrompt\":\"首帧自然延续：…\"}}\n\
         ```\n\n\
         可选字段：`modelVersion` `duration` `videoResolution`（不给则用设置里的默认值）。\n\n\
         **`id` 必须原样回传**（就是 manifest 里那个 `W…`）。不要按文件名匹配——\n\
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
         - 不要写别的文件，不要改 manifest。\n\
         - 不要让产品自己动（旋转展示/被拿起）；不要堆运镜；不要切镜；\n\
         不要写「高级质感/电影级打光」这类广告词。\n\
         - 不要照抄生图提示词里那段产品保真模板（图已经画对了，再喂一遍会把改写挤偏）。\n\n\
         ## manifest 字段\n\n\
         - `thumb` 缩略图（看图用；原图 GenDesk 自己留着喂即梦）\n\
         - `sourcePrompt` 生图提示词全文\n\
         - `variablePart` 剥掉组内公共前后缀后的可变部分（本组剥前 {pre} 字、后 {suf} 字），\
         即场景/构图/动势——改写的真正素材\n\
         - 剥离只是提示不是契约；拿不准就读 `sourcePrompt` 全文\n",
        n = items.len(),
        done = DONE,
        rewrite = REWRITE,
    )
}

/// 收录摘要。
#[derive(Debug, Clone, Default, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct IngestSummary {
    pub applied: i64,
    /// 认不出目标（缺 id / 编号对不上任何在队条目）的行数。
    pub unmatched: i64,
    /// 目标已越过待提交阶段（已提交/已出片）而被拒绝的行数。
    pub stale: i64,
}

/// 扫描 `已改写/*/rewrite.jsonl` 并收录（幂等：收录后移档到 `_已收录/`）。
pub async fn ingest(pool: &SqlitePool, root: &Path) -> AppResult<IngestSummary> {
    let done_root = root.join(V2V).join(DONE);
    let mut sum = IngestSummary::default();
    let Ok(entries) = std::fs::read_dir(&done_root) else {
        return Ok(sum);
    };
    let now = now_unix();

    for entry in entries.flatten() {
        let dir = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if !dir.is_dir() || name == CONSUMED {
            continue;
        }
        let file = dir.join(REWRITE);
        if !file.is_file() {
            continue;
        }
        let body = match std::fs::read_to_string(&file) {
            Ok(b) => b,
            Err(e) => {
                // 逐文件容错：一个组的文件读不动不该拖垮其余组（v0.9.0 的教训）。
                tracing::warn!(path = %file.display(), error = %e, "读改写结果失败");
                continue;
            }
        };
        // **逐文件计数**。归档判据只能看这一份文件干了什么：`sum.*` 是跨文件累计的，
        // 用它做判据意味着「前面某个文件出过一条 unmatched」之后，后面每一个文件都会被
        // 移进 _已收录 —— 包括那些一行都没解析成的。那种文件被移走，人会以为它已经收了。
        let mut applied_here = 0i64;
        let mut touched_here = 0i64;
        for (lineno, raw) in body.lines().enumerate() {
            let raw = raw.trim();
            if raw.is_empty() {
                continue;
            }
            let line: RewriteLine = match serde_json::from_str(raw) {
                Ok(l) => l,
                Err(e) => {
                    tracing::warn!(line = lineno + 1, error = %e, "改写结果 JSON 行解析失败");
                    sum.unmatched += 1;
                    touched_here += 1;
                    continue;
                }
            };
            let Some(prompt) = line
                .video_prompt
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            else {
                sum.unmatched += 1;
                touched_here += 1;
                continue;
            };
            let Some((clip_id, work_id)) = resolve_target(&line) else {
                sum.unmatched += 1;
                touched_here += 1;
                continue;
            };
            let Some(clip) = find_clip(pool, clip_id, work_id).await? else {
                sum.unmatched += 1;
                touched_here += 1;
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
                applied_here += 1;
            } else {
                sum.stale += 1;
                touched_here += 1;
            }
        }

        // 移档而非删除：留证（同 v0.9.0「丢弃改移档」），也避免下一轮 rescan 反复收录。
        // 移不动就地留着——下轮会再收一次，`apply_rewrite` 幂等，代价只是多一次无效更新。
        if applied_here > 0 || touched_here > 0 {
            let consumed = done_root.join(CONSUMED);
            if std::fs::create_dir_all(&consumed).is_ok() {
                let dest = consumed.join(format!("{name}-{now}.jsonl"));
                if let Err(e) = std::fs::rename(&file, &dest) {
                    tracing::warn!(error = %e, "改写结果移档失败，将于下轮重试");
                }
            }
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

    // 组目录名必须对同一组恒定：改名不该让 skill 把整组重做一遍。
    #[test]
    fn group_dir_name_is_stable_across_renames() {
        let a = group_dir_name(Some(12), "鹿晗-B-Roll素材分镜图");
        let b = group_dir_name(Some(12), "改了个名字");
        assert!(a.starts_with("g12"), "{a}");
        assert!(b.starts_with("g12"), "{b}");
        // 纯中文名 slug 退化成 x → 不追加无信息的尾巴。
        assert_eq!(group_dir_name(Some(3), "侯明昊"), "g3");
        assert_eq!(group_dir_name(None, "未分组"), "g0");
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

    // 端到端一轮：物化 → 改写回写 → 收录 → 条目进待提交，且工单目录被清理。
    #[tokio::test]
    async fn full_roundtrip_materialize_then_ingest() {
        let (pool, _d) = test_pool().await;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let thumbs = tmp.path().join("src");
        std::fs::create_dir_all(&thumbs).unwrap();

        // 同组两条，共享产品保真前后缀。
        let t1 = touch_thumb(&thumbs, "a.jpg");
        let t2 = touch_thumb(&thumbs, "b.jpg");
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

        let sum = materialize(&pool, &root).await.unwrap();
        assert_eq!(sum.groups, 1);
        assert_eq!(sum.items, 2);
        assert_eq!(sum.skipped, 0);

        // 组目录名从 index.jsonl 读出来，而不是在测试里硬编码：
        // index 是 skill 的起步契约，让测试也走这条路才守得住它。
        let index = std::fs::read_to_string(root.join(V2V).join(PENDING).join(INDEX)).unwrap();
        let g: serde_json::Value = serde_json::from_str(index.lines().next().unwrap()).unwrap();
        assert_eq!(g["groupId"], 5);
        assert_eq!(g["count"], 2);
        let dir_name = g["dir"].as_str().unwrap().to_string();
        let dir = root.join(V2V).join(PENDING).join(&dir_name);
        assert!(dir.join(READY).is_file(), "READY.txt 须存在");
        assert!(dir.join("thumbs/W1.jpg").is_file(), "缩略图须拷进工单");
        let manifest = std::fs::read_to_string(dir.join(MANIFEST)).unwrap();
        assert_eq!(manifest.lines().count(), 2, "一行一条");
        let first: serde_json::Value =
            serde_json::from_str(manifest.lines().next().unwrap()).unwrap();
        assert_eq!(first["id"], "W1");
        assert_eq!(first["thumb"], "thumbs/W1.jpg");
        assert!(
            first["variablePart"].as_str().unwrap().contains("屋顶花园"),
            "场景差异须保留：{first}"
        );
        assert!(
            !first["variablePart"]
                .as_str()
                .unwrap()
                .contains("像素级完整保留"),
            "组内公共尾巴须剥掉：{first}"
        );
        // 原图**不**进工单：喂 skill 只给缩略图省一个量级的 token。
        assert!(!dir.join("images").exists(), "工单不该带原图目录");

        // skill 写回（目录名与工单侧一致）。
        let done = root.join(V2V).join(DONE).join(&dir_name);
        std::fs::create_dir_all(&done).unwrap();
        std::fs::write(
            done.join(REWRITE),
            "{\"id\":\"W1\",\"videoPrompt\":\"镜头缓推\"}\n{\"id\":\"W2\",\"videoPrompt\":\"光斑掠过\",\"modelVersion\":\"seedance2.0fast\",\"duration\":5}\n",
        )
        .unwrap();

        let ing = ingest(&pool, &root).await.unwrap();
        assert_eq!(ing.applied, 2);
        assert_eq!(ing.unmatched, 0);
        let ready = repo::list_by_stages(&pool, &["ready"]).await.unwrap();
        assert_eq!(ready.len(), 2);
        assert_eq!(ready[0].video_prompt.as_deref(), Some("镜头缓推"));
        assert_eq!(ready[1].model_version.as_deref(), Some("seedance2.0fast"));

        // 收录后移档，再收一次不得重复计数。
        assert!(!done.join(REWRITE).exists(), "收录后须移档");
        assert_eq!(ingest(&pool, &root).await.unwrap().applied, 0);

        // 队列空了 → 重新物化须清掉工单目录，否则 skill 会反复改写同一批。
        let sum = materialize(&pool, &root).await.unwrap();
        assert_eq!(sum.items, 0);
        assert!(!dir.exists(), "已改写完的组目录须被清理");
    }

    // 缺缩略图的条目跳过但**不改阶段**：静默丢弃比留在待改写更糟。
    #[tokio::test]
    async fn missing_thumb_is_skipped_without_losing_the_clip() {
        let (pool, _d) = test_pool().await;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        seed(&pool, 1, 5, "提示词", "/nonexistent/gone.jpg").await;

        let sum = materialize(&pool, &root).await.unwrap();
        assert_eq!(sum.items, 0);
        assert_eq!(sum.skipped, 1);
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
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let done = root
            .join(V2V)
            .join(DONE)
            .join(group_dir_name(Some(5), "组5"));
        std::fs::create_dir_all(&done).unwrap();
        std::fs::write(
            done.join(REWRITE),
            "\n{\"id\":\"W999\",\"videoPrompt\":\"没有这条\"}\n{\"videoPrompt\":\"没有 id\"}\n不是 JSON\n{\"id\":\"W1\"}\n",
        )
        .unwrap();
        let ing = ingest(&pool, &root).await.unwrap();
        assert_eq!(ing.applied, 0);
        assert_eq!(
            ing.unmatched, 4,
            "找不到目标/缺 id/坏 JSON/缺提示词 各计一次"
        );
    }

    // 迟到的改写结果不得把已提交的条目打回（白烧额度），要计入 stale。
    #[tokio::test]
    async fn late_rewrite_against_submitted_clip_is_reported_stale() {
        let (pool, _d) = test_pool().await;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let thumbs = tmp.path().join("src");
        std::fs::create_dir_all(&thumbs).unwrap();
        let t = touch_thumb(&thumbs, "a.jpg");
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

        let done = root
            .join(V2V)
            .join(DONE)
            .join(group_dir_name(Some(5), "组5"));
        std::fs::create_dir_all(&done).unwrap();
        std::fs::write(
            done.join(REWRITE),
            "{\"id\":\"W1\",\"videoPrompt\":\"第二版\"}\n",
        )
        .unwrap();
        let ing = ingest(&pool, &root).await.unwrap();
        assert_eq!(ing.applied, 0);
        assert_eq!(ing.stale, 1);
        let row = repo::get(&pool, id).await.unwrap().unwrap();
        assert_eq!(row.stage, "run");
        assert_eq!(row.video_prompt.as_deref(), Some("第一版"));
    }

    // 默认交接根必须是可预测的固定位置：skill 要把它写死才能做到「什么都不用输入」。
    #[test]
    fn default_root_is_predictable_under_home() {
        let r = default_root();
        assert!(r.ends_with("GenDesk交接"), "{r:?}");
        assert!(r.is_absolute() || r.starts_with("."), "{r:?}");
    }
}
