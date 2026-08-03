-- 0045：参数下沉到任务 + 用途载体从标签换成列。forward-only。
--
-- 这两件事合成一次迁移，因为它们是**同一个决定**的两面：批次退到幕后。
-- 批次此后只是内部运行单元（暂停 / 取消 / 重试 / 进度汇总 / 退休清理挂在它上面），
-- 不再是人的浏览轴，也不再决定「一份工单会被切成几段」。

-- ── 1. 生成参数下沉到任务 ───────────────────────────────────────────────
--
-- 参数原来是**批次级**的，于是「一份工单里各条目画幅不同」只能靠拆批次表达
-- （按 (参数, 抽卡) 分桶）。代价是一次投单在任务页上碎成几张认不出彼此关系的卡片，
-- 而那个切分对人没有任何意义 —— 它只是「params_json 存在 batches 上」的副作用。
--
-- 现在：**一次工单 = 一个 run**，参数跟着任务走。
--
-- 空串 = 用批次级的那份（生成页手动建的批仍然是「整批一套参数」，那条路径不变）。
-- 读取单点 `engine::dispatcher::task_params`，任务级优先、批次级兜底。
ALTER TABLE tasks ADD COLUMN params_json TEXT NOT NULL DEFAULT '';

-- ── 2. 用途：从 tag_bindings 换成 prompts.purpose / accepted_works.purpose ──
--
-- 旧载体是 `tag_bindings(entity_type='prompt_group')`，作品库靠
-- `EXISTS(tag_bindings … entity_id = w.group_id)` 判 is_i2v。两个问题：
--
-- **① 无组之后它没有落脚点。** 工单 v2 里没有组的概念，`name` 即身份。
-- **② 它是无外键的多态表。** 分组删掉时若漏清绑定，分组 id 被后来的分组复用，
--    旧绑定就会把用途安到一个无关的组头上 —— 而「这批图会不会自动进视频流水线」
--    因此变成一件说不清的事。
--
-- 新载体是一列，跟着提示词落到作品行上，随作品一起长期存在。
UPDATE prompts SET purpose = '图生视频'
 WHERE purpose = ''
   AND EXISTS (
         SELECT 1 FROM tag_bindings b JOIN tags t ON t.id = b.tag_id
          WHERE b.entity_type = 'prompt_group' AND b.entity_id = prompts.group_id
            AND t.name = '图生视频');

UPDATE accepted_works SET purpose = '图生视频'
 WHERE purpose = ''
   AND EXISTS (
         SELECT 1 FROM tag_bindings b JOIN tags t ON t.id = b.tag_id
          WHERE b.entity_type = 'prompt_group' AND b.entity_id = accepted_works.group_id
            AND t.name = '图生视频');

-- 清掉用途标签的绑定。**只清用途**，不动 txt 里自由写的 `标签: 白底,3C` ——
-- 那些仍然是分组级的自由标签，与用途不是一回事（0032 里已经分过一次同样的账：
-- 受控枚举与自由文本挤进一列之后，两件事都表达不了）。
DELETE FROM tag_bindings
 WHERE entity_type = 'prompt_group'
   AND tag_id IN (SELECT id FROM tags WHERE name = '图生视频');
