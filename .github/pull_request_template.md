<!-- 审查协议见 CLAUDE.md §审查。实现与审查分离：由全新上下文会话执行 /code-review。 -->

## 变更摘要

<!-- 本 PR 做了什么、对应执行计划的哪个任务（如 M2 2.4） -->

## DoD 勾选

- [ ] 对应任务的 DoD 逐条满足
- [ ] `pnpm check`（全门禁镜像）本地全绿
- [ ] 视觉变更已对照原型 HTML 源码（非截图猜测），token 全走 globals.css

## 契约与数据层

- [ ] 若新增/改动 IPC 命令或事件：已重新生成 `bindings.ts`（`cargo test`）并提交
- [ ] 若改动 SQL / schema：已更新 `.sqlx` 快照（`cargo sqlx prepare`）
- [ ] IPC 载荷字段 camelCase 由 specta 序列化保证（未手写 TS 类型）

## 测试完整性（重点核对）

- [ ] **未**为让 CI 变绿而修改/删除/放宽既有测试断言
- [ ] 如确需改动既有测试：下方单独说明理由

<!-- 若动过既有测试断言，在此说明理由： -->

## 错误与日志

- [ ] 错误分类落在六类之内（Timeout/RateLimited/ContentPolicy/Auth/Interrupted/Other）
- [ ] 日志不会泄露 API Key（脱敏为 `name(****后4位)`）
- [ ] 非测试代码无 `unwrap`/`expect`/`panic`（由 Cargo lints 强制）
