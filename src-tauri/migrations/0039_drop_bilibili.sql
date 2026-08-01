-- 图文发布链路重构 P5：发布平台固定为抖音/小红书/视频号/快手。
DELETE FROM text_items WHERE platform = 'bilibili';
UPDATE settings
SET value_json = replace(replace(value_json, ',"bilibili":true', ''), '"bilibili":true,', '')
WHERE key = 'publish';
