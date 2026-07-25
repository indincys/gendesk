-- 图生视频流水线。forward-only。
--
-- 起因是 v0.13.0 的「导出图生视频包」把流程状态交给了包内的 ledger.jsonl：包一旦被移走、
-- 删掉、重建，「哪条改写完了 / 哪条提交了还没取回」当场失忆，而视频的**终点本来就在库内**
-- （发布模块的视频型素材包 = 1 视频 + 封面）。终点在里面、中段在外面，两边各拥有一半真相，
-- 于是没有任何一处能回答「这批视频做到哪了」。
--
-- 本条把流水线状态收回库内：GenDesk 全程持有真相，Claude Code / Codex 侧的 skill 退化成
-- **无状态的改写服务**（读工单 → 写回改写结果），提交/轮询/下载/重试由本机引擎做。
-- 磁盘上的交接目录只是一次往返的信箱，不再是状态的持有者，故不需要 ledger。
--
-- 只新增表，不 ALTER 既有表。accepted_works 不加 `v2v_stage` 这类列：视频是它的**下游**
-- 而不是它的属性，第二个下游（比如图生图二创）来了就要再加一列。

CREATE TABLE v2v_clips (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  -- 锚点 = accepted_works.id。**不设 FK**，与 0018 work_exports 同一理由：
  -- 作品可进废纸篓/被删，而「这张图当时做过视频」的痕迹要留得住；
  -- 且视频本身是独立资产，父图没了它照样能发布。
  work_id       INTEGER NOT NULL,
  -- 组是成片单元（同组分镜要剪进同一条成片，运镜语言与时长必须统一），
  -- 故冗余组名：作品/分组被删后，看板仍要能把这几条归在一起显示。
  group_id      INTEGER,
  group_name    TEXT    NOT NULL DEFAULT '',
  batch_id      INTEGER,

  -- 七态，与图片侧 tasks.status 同构（一一对应，学一次用两处）：
  --   rewrite 待改写（刚入队，等 skill 改写）
  --   ready   待提交（已有 video_prompt，人过目后提交）
  --   run     已提交（有 submit_id，轮询中）
  --   rev     待验收（mp4 已落盘）
  --   pass    成片 / rej 未通过 / fail 失败
  stage         TEXT    NOT NULL DEFAULT 'rewrite'
                CHECK (stage IN ('rewrite','ready','run','rev','pass','rej','fail')),

  -- 生图提示词快照。取自 accepted_works.prompt_text 而非现读 prompts.text：
  -- 提示词库可被后续微调改写（R8），而这条视频是按当时那份文字做的。
  source_prompt TEXT    NOT NULL,
  -- 剥掉组内公共前后缀后的可变部分（场景/构图/动势）。剥离是提示不是契约，
  -- 故全文与可变部分并存，skill 拿不准可回读全文。
  variable_part TEXT    NOT NULL DEFAULT '',
  -- 改写后的即梦提示词（skill 回写 / 人可在待提交列编辑）。
  video_prompt  TEXT,
  -- 改写附带的生成参数建议；为空则用设置里的默认值。
  model_version TEXT,
  duration      INTEGER,
  video_resolution TEXT,

  submit_id     TEXT,
  credit_count  INTEGER,
  -- 成片落盘绝对路径。
  video_path    TEXT,
  -- 封面**独立成文件**，绝不复用作品缩略图：清空废纸篓会物理删除 file_paths 里的路径，
  -- 若 poster 指向 accepted_works.thumb_path，删一条未通过的视频就会顺手删掉
  -- 还活着的那张作品的缩略图（0016 之后作品库靠它渲染整个瀑布流）。
  poster_path   TEXT,
  width         INTEGER,
  height        INTEGER,
  fps           REAL,
  duration_sec  REAL,

  -- 重跑次数（视频不通过多半是没抽中，不是提示词不对 → 重跑同提示词是最常见的修法）。
  attempt       INTEGER NOT NULL DEFAULT 0,
  error_type    TEXT,
  error_message TEXT,

  rewrote_at    INTEGER,
  submitted_at  INTEGER,
  finished_at   INTEGER,
  reviewed_at   INTEGER,
  created_at    INTEGER NOT NULL,
  updated_at    INTEGER NOT NULL
);

-- 一张验收图同时只有一条在跑的视频：重跑是就地 attempt+1 而不是新增行，
-- 否则看板会堆出同一张图的多条重影，而「这张图做到哪了」又变成没有答案。
-- 历史成片不靠这张表留档——通过的成片会入资产库，未通过的进废纸篓。
CREATE UNIQUE INDEX idx_v2v_clips_work ON v2v_clips (work_id);
-- 看板按 stage 分列取数；组内按 id 保持验收序。
CREATE INDEX idx_v2v_clips_stage ON v2v_clips (stage, group_id, id);
-- 轮询器按 submit_id 回写。
CREATE INDEX idx_v2v_clips_submit ON v2v_clips (submit_id);
