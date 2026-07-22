-- 生成页归档位。forward-only。
--
-- archived_at：点「开始生成」后，本批用到的提示词组与参考图随批次创建同事务打上归档
-- 时间戳。归档只影响「生成页的选择器默认不再列出它们」——库里仍在、仍可查、可一键恢复。
-- 解决：每批新导入一批临时组 + 新上传一批参考图，选择器越积越多，只能手动逐个删。
--
-- 与 0017（api_keys 重建）拆开：那一条必须在事务外跑，一旦中途崩溃无从回滚；本条留在
-- 事务内，保证「加列」这半边要么全成要么全不成，不会卡在「列已加但迁移未记账」的死局。
ALTER TABLE prompt_groups ADD COLUMN archived_at INTEGER;
ALTER TABLE ref_images ADD COLUMN archived_at INTEGER;
CREATE INDEX idx_prompt_groups_archived ON prompt_groups (archived_at);
CREATE INDEX idx_ref_images_archived ON ref_images (archived_at);
