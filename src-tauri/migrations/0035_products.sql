-- 图文发布链路重构 P0：商品成为发布域根实体。forward-only。
CREATE TABLE products (
  id                 INTEGER PRIMARY KEY,
  code               TEXT NOT NULL UNIQUE COLLATE NOCASE,
  name               TEXT NOT NULL,
  platforms_json     TEXT NOT NULL DEFAULT '[]',
  cart_enabled       INTEGER NOT NULL DEFAULT 0 CHECK (cart_enabled IN (0,1)),
  douyin_product_url TEXT NOT NULL DEFAULT '',
  douyin_short_title TEXT NOT NULL DEFAULT '',
  status             TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active','paused')),
  note               TEXT NOT NULL DEFAULT '',
  created_at          INTEGER NOT NULL,
  updated_at          INTEGER NOT NULL
);

ALTER TABLE skus ADD COLUMN product_id INTEGER REFERENCES products(id);
ALTER TABLE skus ADD COLUMN music_keyword TEXT NOT NULL DEFAULT '';
CREATE INDEX idx_skus_product ON skus(product_id);

ALTER TABLE prompt_groups ADD COLUMN sku_id INTEGER REFERENCES skus(id);
ALTER TABLE accepted_works ADD COLUMN sku_id INTEGER REFERENCES skus(id);
CREATE INDEX idx_prompt_groups_sku ON prompt_groups(sku_id);
CREATE INDEX idx_accepted_works_sku ON accepted_works(sku_id);

