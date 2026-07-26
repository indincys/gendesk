-- 生图工单收件台账（Claude Code / Codex 侧 skill 投单 → 自动导入 + 自动建批）。forward-only。
--
-- 为什么必须落库而不是「在工单目录里放个已处理标记」：目录会被移走、被 skill 重建、
-- 被用户手动整理，标记文件一丢就失忆，同一份工单会被反复收录 = 反复建批 = 反复花钱
-- （v0.13.0 的 work_exports 因同一个理由存在）。
--
-- job_id UNIQUE 是这张表的全部意义：**收录必须恰好一次**。任何状态的行都挡住重投，
-- 包括 error —— 失败的工单里可能已经有一半东西进了库，自动重来会造成重复提示词。
-- 想重来是人的决定（设置页「重试」删掉这行）。
CREATE TABLE intake_jobs (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  -- 工单标识：job.json 里的 jobId，缺省时取工单目录名。
  job_id      TEXT    NOT NULL UNIQUE,
  dir_name    TEXT    NOT NULL,
  -- running（收录中，进程中断会留在这个态）/ done / error
  -- hold（任务数超阈值，**什么都没导入**，等人确认）
  status      TEXT    NOT NULL CHECK (status IN ('running', 'done', 'error', 'hold')),
  -- 本工单建出的批次 id（JSON 数组）。
  --
  -- 为什么是多个：批次参数（比例/尺寸/格式/抽卡）是**批次级**的，而一份工单里各组
  -- 可以各写各的比例。收件侧按参数分桶，参数相同的组并进一个批次、不同的拆开——
  -- 这不是妥协，参数不同本来就该是不同批次（验收页按批次分节正好对得上）。
  --
  -- 存 JSON 数组而不是建子表：它是**指针清单不是真相**（真相是 batches 表本身），
  -- 只供设置页台账显示，不参与任何 JOIN。
  batch_ids   TEXT    NOT NULL DEFAULT '[]',
  task_count  INTEGER NOT NULL DEFAULT 0,
  group_count INTEGER NOT NULL DEFAULT 0,
  ref_count   INTEGER NOT NULL DEFAULT 0,
  -- 各批次的参数快照与「实际发往接口的字段」（JSON 数组，与 batch_ids 同序）。
  -- 展示与执行同一来源：全自动收录没有经过生成页那张确认卡，
  -- 「我在组头写了 9:16，到底发出去没有」只能靠它回答。
  params_json TEXT    NOT NULL DEFAULT '[]',
  wire_json   TEXT    NOT NULL DEFAULT '[]',
  message     TEXT    NOT NULL DEFAULT '',
  created_at  INTEGER NOT NULL,
  updated_at  INTEGER NOT NULL
);

-- 设置页按时间倒序列最近工单。
CREATE INDEX idx_intake_jobs_created ON intake_jobs (created_at DESC);
