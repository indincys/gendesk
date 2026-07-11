-- E30b：参考图内容去重（content_hash，导入时按内容比对已有库 + 本次批内）。
-- E41：超限原图导入时生成压缩副本用于上传（upload_path），原图 file_path 仅用于展示。
-- 二者均为可空追加列：历史行 content_hash 为空（不参与去重），upload_path 为空表示用原图上传。
-- forward-only。
ALTER TABLE ref_images ADD COLUMN content_hash TEXT;
ALTER TABLE ref_images ADD COLUMN upload_path TEXT;
CREATE INDEX idx_ref_images_hash ON ref_images (content_hash);
