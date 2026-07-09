//! 前端唯一 IPC 出入口（架构铁律：前端只经 `lib/ipc/` 出入）。
//
// `bindings.ts` 由 tauri-specta 自动生成（禁手改）。本文件是其薄封装：
// - 统一错误规整
// - 提供事件订阅助手
// guardrails 强制 `invoke(` / `listen(` 仅允许出现在 `src/lib/ipc/` 下。

import { commands } from "./bindings";

export { commands };
export type { FrontendErrorPayload } from "./bindings";

/**
 * 转发前端未捕获错误到 Rust 统一日志流。
 * 本函数自身绝不抛错（避免错误处理链再次崩溃）。
 */
export async function reportFrontendError(payload: {
  message: string;
  stack?: string | undefined;
  source?: string | undefined;
  taskId?: string | undefined;
}): Promise<void> {
  try {
    await commands.logFrontendError({
      message: payload.message,
      stack: payload.stack ?? null,
      source: payload.source ?? null,
      taskId: payload.taskId ?? null,
    });
  } catch {
    // 吞掉：日志上报失败不应影响主流程。
  }
}

/** 是否运行在 Tauri 环境（否则为纯浏览器 dev 预览）。 */
export function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}
