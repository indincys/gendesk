-- 提示词小标题（【小标题】导入解析）。forward-only。
-- prompts.title：来自 txt 中每条提示词上方的 `【小标题】` 行；无则 NULL。
-- trash_items.title：删除时冗余快照，废纸篓按 `编号_小标题` 展示。
ALTER TABLE prompts ADD COLUMN title TEXT;
ALTER TABLE trash_items ADD COLUMN title TEXT;
