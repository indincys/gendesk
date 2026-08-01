-- 同一任务单导出必须先原子占用，防双击/并发 IPC 互相替换 READY 包。
ALTER TABLE task_sheets ADD COLUMN export_token TEXT;
