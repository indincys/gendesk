-- no-transaction
-- 单 Key 并发上限 10 → 100。forward-only。
--
-- 首行的 `-- no-transaction` 是 sqlx 指令，必须留在第一行：重建 api_keys 需要先关掉
-- PRAGMA foreign_keys，而该 PRAGMA 在事务内是 no-op（试过 legacy_alter_table 想留在
-- 事务内规避，测试直接抓到它没生效 —— 子表外键被改写成了 REFERENCES "api_keys_old"）。
--
-- SQLite 改不了 CHECK，须按官方 12 步重建表。关键：api_keys 是 tasks / task_attempts
-- 的**父表**（ON DELETE SET NULL）。FK 开启时 DROP 父表会触发隐式 DELETE，把两张子表的
-- api_key_id 整列置空 —— 成功率统计与验收页「按 Key」分组一并报废；且 RENAME 会改写子表
-- 的 REFERENCES 子句。关掉 foreign_keys 后两种副作用都不发生，子表的 `REFERENCES
-- api_keys` 原样留存，重建完自然重新对上（见 migration_0016_widens_concurrency_* 测试）。
PRAGMA foreign_keys = OFF;

-- 事务外无回滚可言：先清掉上一次中途失败可能残留的中间表，使重试可行。
DROP TABLE IF EXISTS api_keys_new;

CREATE TABLE api_keys_new (
  id                INTEGER PRIMARY KEY AUTOINCREMENT,
  name              TEXT NOT NULL,
  keyring_account   TEXT NOT NULL UNIQUE,
  base_url          TEXT NOT NULL,
  model             TEXT NOT NULL,
  concurrency_limit INTEGER NOT NULL DEFAULT 2 CHECK (concurrency_limit BETWEEN 1 AND 100),
  enabled           INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
  created_at        INTEGER NOT NULL,
  rpm_limit         INTEGER,
  circuit_broken    INTEGER NOT NULL DEFAULT 0
);

INSERT INTO api_keys_new
  (id, name, keyring_account, base_url, model, concurrency_limit, enabled, created_at,
   rpm_limit, circuit_broken)
  SELECT id, name, keyring_account, base_url, model, concurrency_limit, enabled, created_at,
         rpm_limit, circuit_broken
  FROM api_keys;

DROP TABLE api_keys;
ALTER TABLE api_keys_new RENAME TO api_keys;

PRAGMA foreign_keys = ON;
