-- 参考图库：按款式、分类长期保存商品参考图，工单可写「从某款式某分类随机抽一张」，
-- 由应用在收录时抽图并记账，近期用过的图在冷却期内优先跳过。
--
-- 图库图片的文件仍是一行 ref_images（任务、batch_refs 的外键父表），故收录直接复用
-- 那一行、不再复制文件；本表只加款式、分类、启停与使用记录这层元信息。
-- ref_images 侧不加列：生成页选择器靠「不在本表里」把图库图片排除在外。

CREATE TABLE ref_library_styles (
  id          INTEGER PRIMARY KEY,
  -- 款式名，与工单 organization.style 对齐（如 王一博、奶油白款）。
  name        TEXT    NOT NULL,
  -- 所属产品，可空；同名款式分属不同产品时靠它区分。
  product     TEXT    NOT NULL DEFAULT '',
  note        TEXT    NOT NULL DEFAULT '',
  created_at  INTEGER NOT NULL,
  UNIQUE (product, name)
);

-- 分类全局共用：每个款式都按同一套分类分图，冷却天数也按分类设。
CREATE TABLE ref_library_categories (
  id             INTEGER PRIMARY KEY,
  name           TEXT    NOT NULL UNIQUE,
  -- 冷却天数：最近一次使用后这么多天内优先跳过；0 = 不冷却（可重复用）。
  cooldown_days  INTEGER NOT NULL DEFAULT 7 CHECK (cooldown_days >= 0 AND cooldown_days <= 365),
  sort_order     INTEGER NOT NULL DEFAULT 0,
  created_at     INTEGER NOT NULL
);

CREATE TABLE ref_library_images (
  id            INTEGER PRIMARY KEY,
  style_id      INTEGER NOT NULL REFERENCES ref_library_styles (id),
  category_id   INTEGER NOT NULL REFERENCES ref_library_categories (id),
  -- 文件与缩略图由这一行 ref_images 持有。
  ref_image_id  INTEGER NOT NULL UNIQUE REFERENCES ref_images (id),
  content_hash  TEXT    NOT NULL,
  note          TEXT    NOT NULL DEFAULT '',
  -- 停用：保留在库里、可手动点名，但不参与随机抽取。
  enabled       INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
  -- 冗余的最近使用时间与次数，与 ref_library_usage 同事务写。
  last_used_at  INTEGER,
  use_count     INTEGER NOT NULL DEFAULT 0,
  created_at    INTEGER NOT NULL,
  -- 移除只做标记：排队中的任务可能还挂着这张图的文件。
  deleted_at    INTEGER,
  UNIQUE (style_id, content_hash)
);
CREATE INDEX idx_ref_library_images_pick ON ref_library_images (style_id, category_id, deleted_at);

-- 使用台账：每次被工单抽中或手动标记都记一行，只增不删。
CREATE TABLE ref_library_usage (
  id         INTEGER PRIMARY KEY,
  image_id   INTEGER NOT NULL REFERENCES ref_library_images (id),
  used_at    INTEGER NOT NULL,
  -- intake = 工单收录；manual = CLI/界面手动标记。
  source     TEXT    NOT NULL CHECK (source IN ('intake', 'manual')),
  job_id     TEXT,
  item_name  TEXT,
  -- 条目内第几张参考图（1 起，对应 Image 1、Image 2）。
  slot       INTEGER,
  -- 1 = 可用图不够，退而用了冷却期内或本单已用过的图。
  fallback   INTEGER NOT NULL DEFAULT 0 CHECK (fallback IN (0, 1))
);
CREATE INDEX idx_ref_library_usage_job ON ref_library_usage (job_id);
CREATE INDEX idx_ref_library_usage_image ON ref_library_usage (image_id, used_at);

INSERT INTO ref_library_categories (name, cooldown_days, sort_order, created_at) VALUES
  ('正面图', 7, 10, CAST(strftime('%s', 'now') AS INTEGER)),
  ('45度角', 7, 20, CAST(strftime('%s', 'now') AS INTEGER)),
  ('礼盒图', 7, 30, CAST(strftime('%s', 'now') AS INTEGER)),
  ('内页展示图', 7, 40, CAST(strftime('%s', 'now') AS INTEGER));
