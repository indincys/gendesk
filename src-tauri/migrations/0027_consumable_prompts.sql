-- 提示词消耗品化 + 验收页真实比例 + 成片交付 + 废纸篓还原。forward-only。
--
-- 本条服务四件互相牵连的事，之所以合成一次迁移：它们改的是**同一个决定**的四个面 ——
-- 「批次跑完就退出历史、提示词用完即弃」。删掉提示词与分组之后，凡是靠 JOIN 现读
-- prompts/prompt_groups 的地方都会当场失忆，所以必须先把这些真相冗余到不会被删的行里。

-- ── 1. 作品的编号与组名改存快照 ──────────────────────────────────────────
-- accepted_works 原本靠 `LEFT JOIN prompts / prompt_groups` 现读编号与组名。提示词一旦
-- 成为消耗品，那两张表随批次一起消失，作品库的编号、分组筛选、全文搜索会同时哑掉 ——
-- 而作品本身是长期资产，它不该因为上游被清理而丢掉自己的身份。
--
-- 与 v2v_clips.group_name 的理由完全一样（0020 已经这么做过一次）：**下游存快照**。
ALTER TABLE accepted_works ADD COLUMN prompt_code TEXT NOT NULL DEFAULT '';
ALTER TABLE accepted_works ADD COLUMN group_name  TEXT NOT NULL DEFAULT '';
UPDATE accepted_works SET
  prompt_code = COALESCE((SELECT code FROM prompts        WHERE id = accepted_works.prompt_id), ''),
  group_name  = COALESCE((SELECT name FROM prompt_groups  WHERE id = accepted_works.group_id),  '');

-- ── 2. 生成结果的真实像素 ────────────────────────────────────────────────
-- 验收页从「统一正方形」改为按原图比例排版。比例必须在**渲染之前**就知道：
-- 等 <img> 加载完再量，每张图落地都会把它下面的所有行往下顶一次，滚动时就是持续抖动
-- （而这一页恰恰是要连续快速翻的）。历史行为 NULL，由 list_pending_review 读缩略图
-- 文件头补齐并写回——只读文件头，不解码整张图。
ALTER TABLE tasks ADD COLUMN result_width  INTEGER;
ALTER TABLE tasks ADD COLUMN result_height INTEGER;

-- ── 3. 成片验收通过即交付 ────────────────────────────────────────────────
-- 通过的图会被拷进 outputs/{批次}/{分组}/，通过的视频此前却只留在 clips/ 这个内部暂存区
-- （文件名是 clip{id}.mp4，人在 Finder 里认不出哪条是哪条）。这里记下那份交付拷贝的路径：
-- 有它才答得出「这条片子在哪」，撤销验收时也才知道该收回哪个文件。
ALTER TABLE v2v_clips ADD COLUMN export_path TEXT;

-- ── 4. 废纸篓还原载荷 ────────────────────────────────────────────────────
-- 「误删可回归原位」对 task/prompt/ref/clip 都只是把状态拨回去（行还在）；唯独作品是
-- 真删了行（accepted_works 没有 deleted_at），只靠 trash_items 现有那几列还原不回来。
-- 故删除时把整行序列化存在这里，还原即原样写回（连 id 一起，v2v_clips.work_id 的锚点
-- 才不会指错人）。
ALTER TABLE trash_items ADD COLUMN payload_json TEXT;
