-- 发布与资产管理模块 schema（发布模块执行计划 §3）。forward-only。
-- 相对路径是真相：dir_rel / file_rel 只存根目录内相对路径，绝对路径从不入库。
-- 时间戳统一 INTEGER（Unix 秒）；日期用 TEXT（'YYYY-MM-DD'）；时刻用 TEXT（'HH:MM'）。

-- SKU 档案。内置一行「通用」分组（is_general=1，code='GENERAL'），收纳不挂具体 SKU 的文本。
CREATE TABLE skus (
  id             INTEGER PRIMARY KEY,
  code           TEXT NOT NULL UNIQUE,            -- ASCII，如 SF-YD-201
  style_name     TEXT NOT NULL,                   -- 款式名
  product_name   TEXT NOT NULL DEFAULT '',        -- 商品名
  tier           TEXT NOT NULL DEFAULT 'warm',    -- hot|warm|cold
  topics_json    TEXT NOT NULL DEFAULT '[]',      -- 固定话题标签，有序，导出取前 5
  platforms_json TEXT,                            -- 平台覆盖（NULL=跟随全局矩阵）
  status         TEXT NOT NULL DEFAULT 'active',  -- active|paused（停发）
  is_general     INTEGER NOT NULL DEFAULT 0 CHECK (is_general IN (0, 1)),
  note           TEXT NOT NULL DEFAULT '',
  created_at     INTEGER NOT NULL,
  updated_at     INTEGER NOT NULL
);

-- 素材包。dir_rel 指向 资产库/{SKU}/{pack} 目录；files_json=[{name,origName,bytes}]。
CREATE TABLE asset_packs (
  id         INTEGER PRIMARY KEY,
  sku_id     INTEGER NOT NULL REFERENCES skus(id),
  kind       TEXT NOT NULL,                       -- video|gallery
  dir_rel    TEXT NOT NULL UNIQUE,
  files_json TEXT NOT NULL,
  cover      TEXT,                                -- 包内封面文件名，可空
  lifecycle  TEXT NOT NULL DEFAULT 'new',         -- new|active|retired（exhausted 为派生态）
  source     TEXT NOT NULL DEFAULT 'inbox',       -- inbox|works|manual
  note       TEXT NOT NULL DEFAULT '',
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE INDEX idx_packs_sku ON asset_packs(sku_id);

-- 标题池 + 正文池（合表，见前置事实 20）
CREATE TABLE text_items (
  id         INTEGER PRIMARY KEY,
  sku_id     INTEGER NOT NULL REFERENCES skus(id),
  kind       TEXT NOT NULL,                       -- title|body
  text       TEXT NOT NULL,
  platform   TEXT NOT NULL DEFAULT 'general',     -- douyin|xhs|kuaishou|shipinhao|bilibili|general
  source     TEXT NOT NULL DEFAULT 'manual',      -- inbox|manual（V2 预留 'ai'）
  enabled    INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
  use_count  INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL
);
CREATE INDEX idx_texts_sku ON text_items(sku_id, kind);

CREATE TABLE accounts (
  id          INTEGER PRIMARY KEY,
  platform    TEXT NOT NULL,
  name        TEXT NOT NULL,                      -- 任务单「平台账号名称」列的值
  daily_limit INTEGER NOT NULL DEFAULT 3,
  slots_json  TEXT,                               -- 可用时段（NULL=用全局时段模板）
  status      TEXT NOT NULL DEFAULT 'active',     -- active|disabled
  created_at  INTEGER NOT NULL,
  UNIQUE(platform, name)
);

-- 日内容套装：一天一 SKU 一套。account_id 列为 V2「每账号独立变体」预留，V1 恒 NULL。
CREATE TABLE daily_sets (
  id         INTEGER PRIMARY KEY,
  date       TEXT NOT NULL,                       -- '2026-07-15'
  sku_id     INTEGER NOT NULL REFERENCES skus(id),
  pack_id    INTEGER NOT NULL REFERENCES asset_packs(id),
  title_id   INTEGER NOT NULL REFERENCES text_items(id),
  body_id    INTEGER REFERENCES text_items(id),   -- 图文才有
  account_id INTEGER,                             -- V2 预留
  UNIQUE(date, sku_id)
);

CREATE TABLE task_sheets (
  id            INTEGER PRIMARY KEY,
  date          TEXT NOT NULL UNIQUE,
  status        TEXT NOT NULL DEFAULT 'draft',    -- draft|confirmed|exported|reconciling|closed
  shortage_json TEXT NOT NULL DEFAULT '[]',       -- 缺料清单（生成副产物）
  report_json   TEXT,                             -- 日报，关闭时写入
  exported_at   INTEGER,
  closed_at     INTEGER,
  created_at    INTEGER NOT NULL,
  updated_at    INTEGER NOT NULL
);

CREATE TABLE publish_tasks (
  id           INTEGER PRIMARY KEY,
  sheet_id     INTEGER NOT NULL REFERENCES task_sheets(id),
  task_code    TEXT NOT NULL UNIQUE,              -- T260715-001
  set_id       INTEGER NOT NULL REFERENCES daily_sets(id),
  account_id   INTEGER NOT NULL REFERENCES accounts(id),
  platform     TEXT NOT NULL,
  content_kind TEXT NOT NULL,                     -- video|gallery
  planned_time TEXT,                              -- 'HH:MM'；NULL=立即发布
  status       TEXT NOT NULL DEFAULT 'pending',   -- pending|published|failed|suspect|canceled
  fail_kind    TEXT,                              -- login|risk|content|page|timeout|other
  result_url   TEXT,
  result_msg   TEXT,
  result_time  INTEGER,
  screenshot   TEXT,                              -- 回执截图文件名
  updated_at   INTEGER NOT NULL
);
CREATE INDEX idx_ptasks_sheet ON publish_tasks(sheet_id);

-- 使用台账：套装粒度，冗余展开列（查重窗口判定不 join）。与对账写入同事务。
CREATE TABLE usage_ledger (
  id           INTEGER PRIMARY KEY,
  date         TEXT NOT NULL,
  sku_id       INTEGER NOT NULL,
  pack_id      INTEGER NOT NULL,
  title_id     INTEGER NOT NULL,
  body_id      INTEGER,
  platform     TEXT NOT NULL,
  account_id   INTEGER NOT NULL,
  task_code    TEXT NOT NULL,
  published_at INTEGER NOT NULL,                  -- 实际发布时间（回执解析）
  url          TEXT
);
CREATE INDEX idx_ledger_pack ON usage_ledger(pack_id, platform, published_at);

-- 收件箱收录记录：成功收录留报告，未知 SKU=待认领，解析失败=待人工确认
CREATE TABLE inbox_items (
  id          INTEGER PRIMARY KEY,
  file_rel    TEXT NOT NULL,                      -- 收件箱内相对路径（收录成功后=归档后路径）
  kind        TEXT,                               -- title|body|combo|media
  sku_code    TEXT,                               -- 识别出的 SKU 编码（可空）
  state       TEXT NOT NULL,                      -- ingested|unclaimed|failed
  detail_json TEXT,                               -- 收录报告：条数/话题差异/错误信息
  created_at  INTEGER NOT NULL
);

-- 内置「通用」分组：收纳不挂具体 SKU 的文本（如节日文案）。
INSERT INTO skus (code, style_name, tier, is_general, status, created_at, updated_at)
VALUES ('GENERAL', '通用', 'warm', 1, 'active',
        CAST(strftime('%s','now') AS INTEGER), CAST(strftime('%s','now') AS INTEGER));
