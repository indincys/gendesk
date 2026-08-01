-- 图文发布链路重构 P2：标题/正文上移到商品，新增三档话题组。
ALTER TABLE text_items ADD COLUMN product_id INTEGER REFERENCES products(id);
ALTER TABLE text_items ADD COLUMN state TEXT NOT NULL DEFAULT 'free'
  CHECK (state IN ('free','held','used'));
ALTER TABLE text_items ADD COLUMN post_id INTEGER;

UPDATE text_items
SET product_id = (SELECT product_id FROM skus WHERE skus.id = text_items.sku_id)
WHERE product_id IS NULL;

CREATE INDEX idx_text_items_product_pick ON text_items(product_id, kind, state, enabled);
CREATE INDEX idx_text_items_post ON text_items(post_id);

CREATE TABLE topic_groups (
  id           INTEGER PRIMARY KEY,
  product_id   INTEGER REFERENCES products(id),
  scope        TEXT NOT NULL CHECK (scope IN ('combo','product','general')),
  sku_ids_json TEXT NOT NULL DEFAULT '[]',
  tags_json    TEXT NOT NULL DEFAULT '[]',
  enabled      INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0,1)),
  created_at   INTEGER NOT NULL,
  updated_at   INTEGER NOT NULL
);
CREATE INDEX idx_topic_groups_match ON topic_groups(product_id, scope, enabled);

-- watcher 恰好一次记账。按内容而非文件名去重，记账与入池在同一事务内先写。
CREATE TABLE copy_ingest_hashes (
  content_hash TEXT PRIMARY KEY,
  file_rel     TEXT NOT NULL,
  created_at   INTEGER NOT NULL
);
