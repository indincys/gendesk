//! 视频流水线执行器：提交 → 轮询 → 落盘 → 待验收。
//!
//! 与图片引擎（`engine::dispatcher`）同构但独立：视频任务不吃 API Key 信号量，
//! 也没有 7 态重试策略，硬塞进那条单循环只会给一条成熟链路引入回归风险。
//! 复用的是**模式**（状态在库、崩溃恢复、事件驱动、spawn_blocking 隔离阻塞活），
//! 不是代码。
//!
//! ## 额度是一次性的，所以状态迁移的顺序不能反
//!
//! 提交成功 → 立刻写 submit_id 并置 run。反过来（先置 run 再写 id）会留下
//! 「跑着但认不出是哪条」的孤儿，而 `recover_orphan_submits` 只能把它退回 ready
//! 让人重提——那就是花两份钱买同一条视频。

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use sqlx::SqlitePool;

use crate::db::now_unix;
use crate::db::repo::v2v as repo;
use crate::error::AppResult;
use crate::files::DataDirs;

use super::dreamina::{self, GenOpts, Outcome};
use super::events::StageCounts;

/// 轮询间隔。视频生成是几十秒到几分钟量级，6 秒足够灵敏又不至于刷爆 CLI。
pub const POLL_INTERVAL: Duration = Duration::from_secs(6);

/// 单条任务的最长在跑时长；超过即判超时失败。
///
/// 兜底而非主判据：正常情况下即梦自己会给 expired。但「未知态判 Running」的策略
/// （见 `dreamina::classify_status`）必须有个尽头，否则一条卡住的任务会被永远轮询。
pub const RUN_TIMEOUT_SECS: i64 = 45 * 60;

/// 提交摘要。
#[derive(Debug, Clone, Default, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SubmitSummary {
    pub submitted: i64,
    pub failed: i64,
    /// 第一条失败的原因（批量提交时逐条上报太吵，给一条代表 + 计数）。
    pub first_error: Option<String>,
}

/// 每条 clip 的生成参数：改写结果里带的优先，其次设置里的默认值。
///
/// skill 对某一条给了具体建议（比如这条动势大，给 8 秒）时应当尊重它——那是看过图的判断。
pub fn opts_for(clip: &repo::ClipRow, defaults: &GenOpts) -> GenOpts {
    GenOpts {
        model_version: clip
            .model_version
            .clone()
            .or_else(|| defaults.model_version.clone()),
        duration: clip.duration.or(defaults.duration),
        video_resolution: clip
            .video_resolution
            .clone()
            .or_else(|| defaults.video_resolution.clone()),
        session: defaults.session,
    }
}

/// 成片与封面的落盘路径。
///
/// 封面**独立成文件**（clip 自己的 jpg），绝不复用 `accepted_works.thumb_path`：
/// 清空废纸篓会物理删除 file_paths 里的路径，若封面指着作品缩略图，
/// 删一条未通过的视频就会顺手删掉还活着的那张作品的缩略图，作品库整片瀑布流跟着空掉。
pub fn clip_paths(dirs: &DataDirs, clip_id: i64) -> (PathBuf, PathBuf) {
    let base = dirs.clips();
    (
        base.join(format!("clip{clip_id}.mp4")),
        base.join(format!("clip{clip_id}.jpg")),
    )
}

/// 批量提交待提交条目。顺序执行：CLI 每次要走网络，并发打过去容易撞限流，
/// 而这一步本来就是人点了「提交」之后的后台活，快几秒没有价值。
pub async fn submit_batch(
    pool: &SqlitePool,
    bin: &str,
    ids: &[i64],
    defaults: &GenOpts,
) -> AppResult<SubmitSummary> {
    let rows = repo::take_ready(pool, ids).await?;
    let mut sum = SubmitSummary::default();
    for clip in rows {
        let Some(prompt) = clip
            .video_prompt
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            sum.failed += 1;
            if sum.first_error.is_none() {
                sum.first_error = Some(format!("{} 没有视频提示词", clip.prompt_code));
            }
            continue;
        };
        let opts = opts_for(&clip, defaults);
        match dreamina::submit(bin, Path::new(&clip.image_path), prompt, &opts).await {
            Ok(submit_id) => {
                repo::mark_submitted(pool, clip.id, &submit_id, now_unix()).await?;
                sum.submitted += 1;
            }
            Err(e) => {
                // 提交失败**不置 run**：没有 submit_id 就没花钱，留在 ready 让人改完重提。
                let msg = format!("{e}");
                tracing::warn!(clip = clip.id, error = %msg, "即梦提交失败");
                repo::mark_failed(pool, clip.id, "submit", &msg, now_unix()).await?;
                sum.failed += 1;
                if sum.first_error.is_none() {
                    sum.first_error = Some(msg);
                }
            }
        }
    }
    Ok(sum)
}

/// 一轮轮询的结果（供事件与测试观察）。
#[derive(Debug, Clone, Default)]
pub struct PollSummary {
    pub finished: i64,
    pub failed: i64,
    pub still_running: i64,
    /// 本轮进度快照：(clip_id, gen_status, queue_idx)。
    pub progress: Vec<(i64, String, Option<i64>)>,
}

/// 超时判定（纯函数，便于测试）。
pub fn is_timed_out(submitted_at: Option<i64>, now: i64, timeout: i64) -> bool {
    submitted_at.is_some_and(|t| now - t > timeout)
}

/// 轮询一轮：把出片的搬到 clips/ 并置待验收，失败的置 fail。
pub async fn poll_once(pool: &SqlitePool, dirs: &DataDirs, bin: &str) -> AppResult<PollSummary> {
    let running = repo::list_running(pool).await?;
    let mut sum = PollSummary::default();
    if running.is_empty() {
        return Ok(sum);
    }
    std::fs::create_dir_all(dirs.clips())?;

    for clip in running {
        let Some(submit_id) = clip.submit_id.clone() else {
            continue;
        };
        let now = now_unix();
        // 下载目录用 clips/ 自身：CLI 会以 submit_id 命名，随后我们改名成 clip{id}.mp4，
        // 让文件名与库里的主键对得上（submit_id 在库里可被重跑覆盖，不适合做文件名）。
        let res = dreamina::query(bin, &submit_id, Some(&dirs.clips())).await;
        let q = match res {
            Ok(q) => q,
            Err(e) => {
                // 查询失败不改状态：网络抖动/CLI 临时不可用不该把已付费的任务判死。
                tracing::warn!(clip = clip.id, error = %e, "即梦查询失败，下轮重试");
                sum.still_running += 1;
                continue;
            }
        };
        match dreamina::classify_status(&q.gen_status) {
            Outcome::Done => {
                let Some(src) = q.video_path.as_deref() else {
                    // 报了成功但没落盘：当作还在跑，下轮再查（CLI 下载失败会重试）。
                    tracing::warn!(clip = clip.id, "即梦报成功但未返回本地路径");
                    sum.still_running += 1;
                    continue;
                };
                let (video, poster) = clip_paths(dirs, clip.id);
                if let Err(e) = std::fs::rename(src, &video) {
                    tracing::warn!(clip = clip.id, error = %e, "成片改名失败，下轮重试");
                    sum.still_running += 1;
                    continue;
                }
                // 封面 = 首帧缩略图的**副本**。image2video 的第一帧就是这张图，
                // 语义上正确，而且不必依赖 ffmpeg 抽帧。
                let poster_out = (!clip.thumb_path.is_empty()
                    && std::fs::copy(&clip.thumb_path, &poster).is_ok())
                .then(|| poster.to_string_lossy().to_string());
                repo::mark_ready_for_review(
                    pool,
                    clip.id,
                    &video.to_string_lossy(),
                    poster_out.as_deref(),
                    q.width,
                    q.height,
                    q.fps,
                    q.duration_sec,
                    q.credit_count,
                    now,
                )
                .await?;
                sum.finished += 1;
            }
            Outcome::Failed => {
                let reason = if q.fail_reason.is_empty() {
                    q.gen_status.clone()
                } else {
                    q.fail_reason.clone()
                };
                repo::mark_failed(pool, clip.id, "provider", &reason, now).await?;
                sum.failed += 1;
            }
            Outcome::Running => {
                if is_timed_out(clip.submitted_at, now, RUN_TIMEOUT_SECS) {
                    repo::mark_failed(
                        pool,
                        clip.id,
                        "timeout",
                        &format!(
                            "提交后 {} 分钟仍未出片（末次状态 {}）。可在此重跑。",
                            RUN_TIMEOUT_SECS / 60,
                            q.gen_status
                        ),
                        now,
                    )
                    .await?;
                    sum.failed += 1;
                } else {
                    sum.still_running += 1;
                    sum.progress
                        .push((clip.id, q.gen_status.clone(), q.queue_idx));
                }
            }
        }
    }
    Ok(sum)
}

/// 当前七态计数（事件载荷）。
pub async fn counts(pool: &SqlitePool) -> AppResult<StageCounts> {
    Ok(StageCounts::from_rows(&repo::stage_counts(pool).await?))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试断言失败即失败
mod tests {
    use super::*;

    fn clip(model: Option<&str>, dur: Option<i64>, res: Option<&str>) -> repo::ClipRow {
        repo::ClipRow {
            id: 1,
            work_id: 1,
            group_id: None,
            group_name: String::new(),
            batch_id: None,
            stage: "ready".into(),
            source_prompt: String::new(),
            variable_part: String::new(),
            video_prompt: Some("p".into()),
            model_version: model.map(|s| s.to_string()),
            duration: dur,
            video_resolution: res.map(|s| s.to_string()),
            submit_id: None,
            credit_count: None,
            video_path: None,
            poster_path: None,
            width: None,
            height: None,
            fps: None,
            duration_sec: None,
            attempt: 0,
            error_type: None,
            error_message: None,
            submitted_at: None,
            created_at: 0,
            updated_at: 0,
            prompt_code: "GG-0001".into(),
            image_path: "/img.jpg".into(),
            thumb_path: "/thumb.jpg".into(),
            accepted_at: 0,
        }
    }

    // skill 对某一条给的建议优先于全局默认：它是看过图之后的判断
    // （这条动势大就给 8 秒），全局默认只是兜底。
    #[test]
    fn per_clip_params_win_over_defaults() {
        let defaults = GenOpts {
            model_version: Some("seedance2.0fast".into()),
            duration: Some(4),
            video_resolution: Some("720p".into()),
            session: Some(0),
        };
        let o = opts_for(&clip(Some("seedance2.0_vip"), Some(8), Some("1080p")), &defaults);
        assert_eq!(o.model_version.as_deref(), Some("seedance2.0_vip"));
        assert_eq!(o.duration, Some(8));
        assert_eq!(o.video_resolution.as_deref(), Some("1080p"));
        assert_eq!(o.session, Some(0), "session 只由设置决定，不由 skill 指定");
    }

    // 没给的项回落到默认值，逐项回落而非整组回落。
    #[test]
    fn missing_per_clip_params_fall_back_field_by_field() {
        let defaults = GenOpts {
            model_version: Some("seedance2.0fast".into()),
            duration: Some(4),
            video_resolution: Some("720p".into()),
            session: None,
        };
        let o = opts_for(&clip(None, Some(9), None), &defaults);
        assert_eq!(o.model_version.as_deref(), Some("seedance2.0fast"));
        assert_eq!(o.duration, Some(9), "只覆盖它给了的那一项");
        assert_eq!(o.video_resolution.as_deref(), Some("720p"));
    }

    // 封面必须是 clip 自己的文件，绝不能是作品缩略图路径本身：
    // 清空废纸篓会物理删 file_paths 里的路径，指着作品缩略图就会删掉还活着的作品的图。
    #[test]
    fn poster_path_is_owned_by_the_clip_not_shared_with_the_work() {
        let dirs = DataDirs::new("/data");
        let (video, poster) = clip_paths(&dirs, 42);
        assert!(video.ends_with("clip42.mp4"));
        assert!(poster.ends_with("clip42.jpg"));
        assert_ne!(
            poster.to_string_lossy(),
            "/thumb.jpg",
            "封面不得与作品缩略图同路径"
        );
        assert!(
            poster.starts_with("/data/clips"),
            "封面须落在 clips/ 自己的目录里"
        );
    }

    // 成片文件名锚在 clip id 而不是 submit_id：重跑会换 submit_id，
    // 文件名跟着换就会在 clips/ 里堆出认不出主人的孤儿。
    #[test]
    fn media_filenames_are_anchored_to_clip_id() {
        let dirs = DataDirs::new("/data");
        let (a, _) = clip_paths(&dirs, 7);
        let (b, _) = clip_paths(&dirs, 7);
        assert_eq!(a, b, "同一 clip 的路径必须恒定（重跑覆盖旧文件）");
    }

    // 超时兜底：「未知态判 Running」必须有个尽头，否则卡住的任务被永远轮询。
    #[test]
    fn timeout_bounds_the_unknown_status_policy() {
        assert!(!is_timed_out(Some(1000), 1000 + 10, RUN_TIMEOUT_SECS));
        assert!(is_timed_out(Some(1000), 1000 + RUN_TIMEOUT_SECS + 1, RUN_TIMEOUT_SECS));
        assert!(
            !is_timed_out(None, 999_999, RUN_TIMEOUT_SECS),
            "没有提交时间的条目不该被判超时"
        );
    }
}
