-- 文案库：全局话题标签池（标题/正文复用 text_items 表，本表只补话题池）。
-- topic_tags 是全局话题池的持久化投影；SKU 固定话题（skus.topics_json）仍是权威，
-- 本表通过 ensure/sync 幂等补齐，使用次数实时从 SKU JSON 统计、不落冗余列。
CREATE TABLE topic_tags (
  id         INTEGER PRIMARY KEY,
  tag        TEXT NOT NULL UNIQUE,
  enabled    INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE INDEX idx_topic_tags_tag ON topic_tags(tag);
