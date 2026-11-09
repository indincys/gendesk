-- 库级元数据。目前只有一项 reader_floor：打开这份库的应用，内嵌迁移至少要到第几号。
--
-- 旧版本应用（例如回退安装、另一台电脑还没更新）打开被新版本升级过的库时，
-- 只要 reader_floor 不高于它内嵌的最新迁移，就忽略它不认识的迁移照常启动
-- （见 src-tauri/src/db/upgrade.rs）。
--
-- 迁移默认只做加法（新表、带默认值的新列、新索引），旧代码看不见也不受影响，
-- 不必改这里。会让旧代码读写出错的迁移（删表、删列、改名、重建表、改变已有列的含义）
-- 必须在同一个文件里抬高下限：
--   UPDATE schema_meta SET value = MAX(value, <本迁移号>) WHERE key = 'reader_floor';
CREATE TABLE IF NOT EXISTS schema_meta (
    key   TEXT PRIMARY KEY,
    value INTEGER NOT NULL
);

-- 0052 之前的应用没有这套判断，拿到较新的库一律拒绝启动，所以下限从 52 起算。
INSERT OR IGNORE INTO schema_meta (key, value) VALUES ('reader_floor', 52);
