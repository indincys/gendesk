-- Poster 改为「审稿通过即自动导入」（设置里手动开启），不再逐批弹窗确认。
-- 开启实况图时，同一作品的 Poster 条目等实况配对落盘后带着视频一起导入。
ALTER TABLE poster_images ADD COLUMN wants_live INTEGER NOT NULL DEFAULT 0;
ALTER TABLE poster_images ADD COLUMN live_image TEXT;
ALTER TABLE poster_images ADD COLUMN live_video TEXT;
-- 旧版「审稿后询问」留下的未确认批次：用户从未同意导入，标为跳过，不随新开关被补导。
UPDATE poster_batches SET state = 'skipped', last_error = '旧版待确认批次，未导入'
WHERE state IN ('awaiting', 'preparing');
