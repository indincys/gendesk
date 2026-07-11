-- 挂靠记忆（E32）：记录每张参考图最近一次挂靠的提示词组。
-- 生成页选中参考图时，若该组在本次已选分组中则自动预填挂靠，可手动改。
-- 组被删除时置空（ON DELETE SET NULL），不阻塞删组。forward-only。
ALTER TABLE ref_images ADD COLUMN last_group_id INTEGER
  REFERENCES prompt_groups(id) ON DELETE SET NULL;
