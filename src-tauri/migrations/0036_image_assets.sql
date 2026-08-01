-- 图文发布链路重构 P1：一张图一条素材记录，库内路径只存 RelPath。
CREATE TABLE image_assets (
  id         INTEGER PRIMARY KEY,
  sku_id     INTEGER NOT NULL REFERENCES skus(id),
  path_rel   TEXT NOT NULL UNIQUE,
  thumb_rel  TEXT NOT NULL,
  source     TEXT NOT NULL CHECK (source IN ('works','import')),
  work_id    INTEGER,
  state      TEXT NOT NULL DEFAULT 'free' CHECK (state IN ('free','held','used')),
  post_id    INTEGER,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE INDEX idx_image_assets_pick ON image_assets(sku_id, state);
CREATE INDEX idx_image_assets_post ON image_assets(post_id);

