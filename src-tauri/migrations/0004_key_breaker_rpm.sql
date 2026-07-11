-- Key 级熔断 + RPM 限流（E18 / 需求 §9.2 §11）。forward-only。
-- rpm_limit：可选每分钟请求上限（NULL = 不限）；调度器按滑动窗口限速。
-- circuit_broken：连续 Auth/欠费失败达阈值时自动置 1 并停用（enabled=0），
--   与用户手动停用区分；设置页显示「已熔断」并提供恢复按钮（清此位 + 重新启用）。
ALTER TABLE api_keys ADD COLUMN rpm_limit INTEGER;
ALTER TABLE api_keys ADD COLUMN circuit_broken INTEGER NOT NULL DEFAULT 0;
