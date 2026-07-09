-- GenDesk 数据库 schema 定稿（执行计划 §3）。forward-only。
-- 连接参数（WAL / synchronous=NORMAL / busy_timeout / foreign_keys）在连接池创建时设置。
-- 时间戳统一存 INTEGER（Unix 秒）。

-- 设置：key/value_json 键值对
CREATE TABLE settings (
  key        TEXT PRIMARY KEY,
  value_json TEXT NOT NULL
);

-- API Key：Key 本体在系统钥匙串，库中只存 keyring 引用
CREATE TABLE api_keys (
  id                INTEGER PRIMARY KEY AUTOINCREMENT,
  name              TEXT NOT NULL,
  keyring_account   TEXT NOT NULL UNIQUE,
  base_url          TEXT NOT NULL,
  model             TEXT NOT NULL,
  concurrency_limit INTEGER NOT NULL DEFAULT 2 CHECK (concurrency_limit BETWEEN 1 AND 10),
  enabled           INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
  created_at        INTEGER NOT NULL
);

-- 提示词分组（与参考图库共用分组体系）
CREATE TABLE prompt_groups (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  name       TEXT NOT NULL,
  prefix     TEXT NOT NULL UNIQUE,
  scene      TEXT NOT NULL DEFAULT '',
  is_temp    INTEGER NOT NULL DEFAULT 0 CHECK (is_temp IN (0, 1)),
  created_at INTEGER NOT NULL
);

-- 提示词
CREATE TABLE prompts (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  group_id   INTEGER NOT NULL REFERENCES prompt_groups (id) ON DELETE CASCADE,
  code       TEXT NOT NULL,
  text       TEXT NOT NULL,
  favorite   INTEGER NOT NULL DEFAULT 0 CHECK (favorite IN (0, 1)),
  edited     INTEGER NOT NULL DEFAULT 0 CHECK (edited IN (0, 1)),
  status     TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'trash')),
  source     TEXT NOT NULL DEFAULT 'library',
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  UNIQUE (group_id, code)
);
CREATE INDEX idx_prompts_group_status ON prompts (group_id, status);

-- 标签 / 多态标签绑定（V1 绑定在分组级 entity_type='prompt_group'）
CREATE TABLE tags (
  id   INTEGER PRIMARY KEY AUTOINCREMENT,
  name TEXT NOT NULL UNIQUE
);
CREATE TABLE tag_bindings (
  tag_id      INTEGER NOT NULL REFERENCES tags (id) ON DELETE CASCADE,
  entity_type TEXT NOT NULL,
  entity_id   INTEGER NOT NULL,
  PRIMARY KEY (tag_id, entity_type, entity_id)
);

-- 参考图（group_id 可空 = 未分组）
CREATE TABLE ref_images (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  name       TEXT NOT NULL,
  group_id   INTEGER REFERENCES prompt_groups (id) ON DELETE SET NULL,
  file_path  TEXT NOT NULL,
  thumb_path TEXT NOT NULL,
  width      INTEGER NOT NULL,
  height     INTEGER NOT NULL,
  file_size  INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  deleted_at INTEGER
);
CREATE INDEX idx_ref_images_group ON ref_images (group_id);

-- 批次
CREATE TABLE batches (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  created_at  INTEGER NOT NULL,
  output_dir  TEXT NOT NULL,
  params_json TEXT NOT NULL DEFAULT '{}',
  status      TEXT NOT NULL DEFAULT 'running' CHECK (status IN ('running', 'archived'))
);

-- 批次参考图挂靠（R1：每张参考图在本批次挂靠的提示词组）
CREATE TABLE batch_refs (
  batch_id        INTEGER NOT NULL REFERENCES batches (id) ON DELETE CASCADE,
  ref_image_id    INTEGER NOT NULL REFERENCES ref_images (id) ON DELETE CASCADE,
  prompt_group_id INTEGER NOT NULL REFERENCES prompt_groups (id) ON DELETE CASCADE,
  PRIMARY KEY (batch_id, ref_image_id)
);

-- 任务（八态：q/run/retry/rev/pass/rej/fail；成功即待验收 rev）
CREATE TABLE tasks (
  id                   INTEGER PRIMARY KEY AUTOINCREMENT,
  batch_id             INTEGER NOT NULL REFERENCES batches (id) ON DELETE CASCADE,
  ref_image_id         INTEGER NOT NULL REFERENCES ref_images (id) ON DELETE CASCADE,
  prompt_id            INTEGER NOT NULL REFERENCES prompts (id) ON DELETE CASCADE,
  prompt_text_snapshot TEXT NOT NULL,
  status               TEXT NOT NULL DEFAULT 'q'
                         CHECK (status IN ('q', 'run', 'retry', 'rev', 'pass', 'rej', 'fail')),
  api_key_id           INTEGER REFERENCES api_keys (id) ON DELETE SET NULL,
  error_type           TEXT,
  error_message        TEXT,
  retry_count          INTEGER NOT NULL DEFAULT 0,
  result_image_path    TEXT,
  result_thumb_path    TEXT,
  created_at           INTEGER NOT NULL,
  updated_at           INTEGER NOT NULL
);
CREATE INDEX idx_tasks_batch_status ON tasks (batch_id, status);

-- 任务执行记录（每次尝试）
CREATE TABLE task_attempts (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  task_id       INTEGER NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
  api_key_id    INTEGER REFERENCES api_keys (id) ON DELETE SET NULL,
  started_at    INTEGER NOT NULL,
  finished_at   INTEGER,
  outcome       TEXT NOT NULL,
  error_type    TEXT,
  error_message TEXT,
  http_status   INTEGER,
  duration_ms   INTEGER
);
CREATE INDEX idx_task_attempts_task ON task_attempts (task_id);
CREATE INDEX idx_task_attempts_key_time ON task_attempts (api_key_id, started_at);

-- 通过作品（快照式冗余，长期可追溯）
CREATE TABLE accepted_works (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  task_id      INTEGER NOT NULL REFERENCES tasks (id) ON DELETE SET NULL,
  image_path   TEXT NOT NULL,
  thumb_path   TEXT NOT NULL,
  prompt_id    INTEGER,
  prompt_text  TEXT NOT NULL,
  group_id     INTEGER,
  ref_image_id INTEGER,
  batch_id     INTEGER,
  favorite     INTEGER NOT NULL DEFAULT 0 CHECK (favorite IN (0, 1)),
  accepted_at  INTEGER NOT NULL
);
CREATE INDEX idx_accepted_works_group ON accepted_works (group_id);

-- 编号号池 / 回收池
CREATE TABLE id_pools (
  prefix   TEXT PRIMARY KEY,
  next_seq INTEGER NOT NULL DEFAULT 1
);
CREATE TABLE id_recycled (
  prefix TEXT NOT NULL,
  number INTEGER NOT NULL,
  PRIMARY KEY (prefix, number)
);

-- 废纸篓清单
CREATE TABLE trash_items (
  id              INTEGER PRIMARY KEY AUTOINCREMENT,
  entity_type     TEXT NOT NULL,
  ref_id          INTEGER,
  thumb_path      TEXT,
  prompt_text     TEXT,
  code            TEXT,
  source_label    TEXT NOT NULL,
  file_paths_json TEXT NOT NULL DEFAULT '[]',
  deleted_at      INTEGER NOT NULL
);
