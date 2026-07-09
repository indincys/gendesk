#!/usr/bin/env bash
# ============================================================================
# check.sh —— 本地一键全门禁，是 CI check.yml 的完整镜像（执行计划 §1.6）。
# 提交前跑它 == 预演 CI。任一步失败即整体失败。
# ============================================================================
set -euo pipefail
cd "$(dirname "$0")/.."

step() { echo ""; echo "════════ $1 ════════"; }

step "1/9 guardrails 架构铁律"
bash scripts/guardrails.sh

step "2/9 Biome（lint + format）"
pnpm biome ci .

step "3/9 TypeScript strict"
pnpm tsc --noEmit

step "4/9 前端单测（vitest）"
pnpm vitest run

step "5/9 前端构建"
pnpm vite build

step "6/9 Rust fmt"
( cd src-tauri && cargo fmt --check )

step "7/9 Rust clippy（-D warnings）"
( cd src-tauri && cargo clippy --all-targets -- -D warnings )

step "8/9 Rust 测试 + 绑定同步校验"
( cd src-tauri && cargo test )
if ! git diff --quiet -- src/lib/ipc/bindings.ts; then
  echo "✗ bindings.ts 与 Rust 契约不同步（cargo test 已重新生成，请提交）"
  git --no-pager diff -- src/lib/ipc/bindings.ts
  exit 1
fi

step "9/9 Rust check（Tauri 侧可编译）"
( cd src-tauri && cargo check )

echo ""
echo "✓ 全部门禁通过（本地镜像 == CI）"
