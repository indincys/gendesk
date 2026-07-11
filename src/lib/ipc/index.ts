//! 前端唯一 IPC 出入口（架构铁律：前端只经 `lib/ipc/` 出入）。
//
// `bindings.ts` 由 tauri-specta 自动生成（禁手改）。本文件是其薄封装：
// - 统一错误规整
// - 提供事件订阅助手
// guardrails 强制 `invoke(` / `listen(` 仅允许出现在 `src/lib/ipc/` 下。

import { events, type AppError, type Result, commands } from "./bindings";

export { commands, events };
export type {
  AddApiKeyInput,
  ApiKeyView,
  AppError,
  BackupProgress,
  BatchSummary,
  BatchView,
  DataDirInfo,
  CreateBatchInput,
  FrontendErrorPayload,
  GroupView,
  ImportPreview,
  ImportPreviewGroup,
  ImportResult,
  KeyHealth,
  PromptView,
  RefImageDetail,
  RefImageView,
  RefMappingInput,
  Result,
  ReviewItemView,
  Settings,
  SettingsPatch,
  SummaryCounts,
  TaskProgress,
  TaskStatusChanged,
  TaskView,
  TrashItemView,
  UpdateApiKeyPatch,
  WorkView,
} from "./bindings";

/** 应用错误转为 Error 抛出（tauri-specta Result → 抛异常，便于 try/catch 统一处理）。 */
export class IpcError extends Error {
  constructor(public readonly appError: AppError) {
    super(`${appError.type}: ${appError.message}`);
    this.name = "IpcError";
  }
}

/** 解包 tauri-specta 的 Result：ok 返回数据，error 抛 [`IpcError`]。 */
export async function unwrap<T>(promise: Promise<Result<T, AppError>>): Promise<T> {
  const result = await promise;
  if (result.status === "ok") return result.data;
  throw new IpcError(result.error);
}

/**
 * 订阅引擎事件（唯一的 `listen` 出入口，铁律：只在 lib/ipc 内）。
 * 返回反订阅函数；非 Tauri 环境为 no-op。
 */
export async function subscribeEngine(handlers: {
  onSummary?: (e: import("./bindings").BatchSummary) => void;
  onProgress?: (e: import("./bindings").TaskProgress) => void;
  onStatus?: (e: import("./bindings").TaskStatusChanged) => void;
  onKeyHealth?: (e: import("./bindings").KeyHealth) => void;
  onUpdateState?: (e: import("./bindings").UpdateStateChanged) => void;
}): Promise<() => void> {
  if (!isTauri()) return () => {};
  const unlisteners = await Promise.all([
    events.batchSummary.listen((e) => handlers.onSummary?.(e.payload)),
    events.taskProgress.listen((e) => handlers.onProgress?.(e.payload)),
    events.taskStatusChanged.listen((e) => handlers.onStatus?.(e.payload)),
    events.keyHealth.listen((e) => handlers.onKeyHealth?.(e.payload)),
    events.updateStateChanged.listen((e) => handlers.onUpdateState?.(e.payload)),
  ]);
  return () => {
    for (const un of unlisteners) un();
  };
}

/**
 * 订阅数据备份进度事件（E19）。返回反订阅函数；非 Tauri 环境为 no-op。
 * 独立于引擎事件订阅：仅设置页导出期间临时挂载。
 */
export async function subscribeBackupProgress(
  handler: (e: import("./bindings").BackupProgress) => void,
): Promise<() => void> {
  if (!isTauri()) return () => {};
  const un = await events.backupProgress.listen((e) => handler(e.payload));
  return () => un();
}

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

/**
 * 订阅窗口文件拖入事件（E14 拖拽导入）。仅在 drop 完成时回调落盘路径列表。
 * Tauri webview 事件封装于此，遵守「前端只经 lib/ipc 出入」铁律。
 * 返回反订阅函数；非 Tauri 环境为 no-op。
 */
export async function subscribeFileDrop(handler: (paths: string[]) => void): Promise<() => void> {
  if (!isTauri()) return () => {};
  const { getCurrentWebview } = await import("@tauri-apps/api/webview");
  const un = await getCurrentWebview().onDragDropEvent((event) => {
    if (event.payload.type === "drop") handler(event.payload.paths);
  });
  return () => un();
}
