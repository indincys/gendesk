-- 图文发布链路重构 P3：旧发布数据无历史保留价值，替换为商品任务单模型。
DROP TABLE IF EXISTS publish_tasks;
DROP TABLE IF EXISTS daily_sets;
DROP TABLE IF EXISTS task_sheets;
DROP TABLE IF EXISTS asset_packs;
DROP TABLE IF EXISTS usage_ledger;
DROP TABLE IF EXISTS accounts;

-- 0010 的 sku_id 是 NOT NULL。上移商品后必须让废弃列真正可以不再写，旧发布表已在
-- 上方删除，所以此刻重建不会留下指向旧表名的外键。
ALTER TABLE text_items RENAME TO text_items_legacy;
CREATE TABLE text_items (
  id         INTEGER PRIMARY KEY,
  sku_id     INTEGER REFERENCES skus(id),
  product_id INTEGER REFERENCES products(id),
  kind       TEXT NOT NULL CHECK (kind IN ('title','body')),
  text       TEXT NOT NULL,
  platform   TEXT NOT NULL DEFAULT 'general',
  source     TEXT NOT NULL DEFAULT 'manual',
  enabled    INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0,1)),
  use_count  INTEGER NOT NULL DEFAULT 0,
  state      TEXT NOT NULL DEFAULT 'free' CHECK (state IN ('free','held','used')),
  post_id    INTEGER,
  created_at INTEGER NOT NULL
);
INSERT INTO text_items
  (id,sku_id,product_id,kind,text,platform,source,enabled,use_count,state,post_id,created_at)
SELECT id,sku_id,product_id,kind,text,platform,source,enabled,use_count,state,post_id,created_at
FROM text_items_legacy;
DROP TABLE text_items_legacy;
CREATE INDEX idx_text_items_product_pick ON text_items(product_id,kind,state,enabled);
CREATE INDEX idx_text_items_post ON text_items(post_id);

CREATE TABLE sheet_configs (
  id              INTEGER PRIMARY KEY,
  product_id      INTEGER NOT NULL REFERENCES products(id),
  name            TEXT NOT NULL,
  sku_scope_json  TEXT NOT NULL DEFAULT '[]',
  platforms_json  TEXT NOT NULL DEFAULT '[]',
  posts_per_day   INTEGER NOT NULL DEFAULT 5 CHECK (posts_per_day > 0),
  images_per_post INTEGER NOT NULL DEFAULT 5 CHECK (images_per_post > 0),
  mixed_count     INTEGER NOT NULL DEFAULT 1 CHECK (mixed_count >= 0),
  anchors_json    TEXT NOT NULL DEFAULT '[]',
  jitter_min      INTEGER NOT NULL DEFAULT 15 CHECK (jitter_min >= 0),
  min_gap_min     INTEGER NOT NULL DEFAULT 3 CHECK (min_gap_min >= 1),
  target_day      TEXT NOT NULL DEFAULT 'next' CHECK (target_day IN ('next','same')),
  enabled         INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0,1)),
  created_at      INTEGER NOT NULL,
  updated_at      INTEGER NOT NULL
);
CREATE INDEX idx_sheet_configs_product ON sheet_configs(product_id, enabled);

CREATE TABLE task_sheets (
  id            INTEGER PRIMARY KEY,
  date          TEXT NOT NULL,
  product_id    INTEGER NOT NULL REFERENCES products(id),
  config_id     INTEGER NOT NULL REFERENCES sheet_configs(id),
  title         TEXT NOT NULL,
  status        TEXT NOT NULL DEFAULT 'draft'
                CHECK (status IN ('draft','confirmed','exported','reconciling','closed')),
  export_dir    TEXT,
  shortage_json TEXT NOT NULL DEFAULT '[]',
  report_json   TEXT,
  exported_at   INTEGER,
  closed_at     INTEGER,
  created_at    INTEGER NOT NULL,
  updated_at    INTEGER NOT NULL,
  UNIQUE(date, product_id)
);

CREATE TABLE posts (
  id           INTEGER PRIMARY KEY,
  sheet_id     INTEGER NOT NULL REFERENCES task_sheets(id) ON DELETE CASCADE,
  content_code TEXT NOT NULL UNIQUE,
  seq          INTEGER NOT NULL,
  kind         TEXT NOT NULL CHECK (kind IN ('single','mixed')),
  title_id     INTEGER REFERENCES text_items(id),
  body_id      INTEGER REFERENCES text_items(id),
  title_text   TEXT,
  body_text    TEXT,
  topics_json  TEXT NOT NULL DEFAULT '[]',
  edited       INTEGER NOT NULL DEFAULT 0 CHECK (edited IN (0,1)),
  UNIQUE(sheet_id, seq)
);

CREATE TABLE post_images (
  post_id  INTEGER NOT NULL REFERENCES posts(id) ON DELETE CASCADE,
  asset_id INTEGER NOT NULL REFERENCES image_assets(id),
  ord      INTEGER NOT NULL,
  PRIMARY KEY (post_id, ord),
  UNIQUE(post_id, asset_id)
);

CREATE TABLE publish_tasks (
  id           INTEGER PRIMARY KEY,
  sheet_id     INTEGER NOT NULL REFERENCES task_sheets(id) ON DELETE CASCADE,
  post_id      INTEGER NOT NULL REFERENCES posts(id) ON DELETE CASCADE,
  task_code    TEXT NOT NULL UNIQUE,
  platform     TEXT NOT NULL CHECK (platform IN ('douyin','xhs','shipinhao','kuaishou')),
  scheduled_at TEXT NOT NULL,
  status       TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending','done','failed')),
  fail_kind    TEXT,
  result_msg   TEXT,
  result_time  INTEGER,
  updated_at   INTEGER NOT NULL
);
CREATE INDEX idx_ptasks_sheet ON publish_tasks(sheet_id);
CREATE INDEX idx_ptasks_post ON publish_tasks(post_id);
