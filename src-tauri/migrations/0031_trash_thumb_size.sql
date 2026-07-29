-- 0031：废纸篓缩略图像素缓存。
--
-- 废纸篓改成按真实比例排版的网格之后，「每张图占多宽、每行多高」必须在渲染**之前**
-- 算得出来 —— 等图片加载完再量，每张图落地都会把它下面那一行往下顶一次，
-- 滚动时就是持续抖动，而这一页恰恰是拿来逐张排查误删的。
--
-- 与 0027 给 tasks 加 result_width/result_height 是同一件事、同一个理由。这里另存一份
-- 而不是回源头去查，是因为废纸篓里五类实体的尺寸散在四张表上（tasks / ref_images /
-- v2v_clips / 作品的整行快照），而作品那一份根本没有尺寸列 —— 唯一对五类都成立的
-- 事实是「它有一张缩略图躺在盘上」。测一次写回来，此后不再测。
ALTER TABLE trash_items ADD COLUMN thumb_w INTEGER;
ALTER TABLE trash_items ADD COLUMN thumb_h INTEGER;
