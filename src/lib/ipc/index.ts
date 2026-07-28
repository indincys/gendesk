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
  ProductionOverview,
  PurposeView,
  ImportPreview,
  ImportPreviewGroup,
  ImportPreviewPrompt,
  ImportResult,
  KeyHealth,
  PromptView,
  RefGroupView,
  RefImageDetail,
  RefImageView,
  RefImportProgress,
  RefMappingInput,
  RefScanItem,
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
  // 发布与资产管理模块
  PublishSettings,
  PublishSettingsPatch,
  PlatformInfo,
  PlatformMatrix,
  TierRules,
  SkuView,
  SkuDetail,
  SkuFilter,
  CreateSkuInput,
  SkuPatch,
  MappingImportReport,
  HistoryItem,
  PublishBadges,
  TextItemView,
  AddTextItemInput,
  TextItemPatch,
  PackView,
  PackFileView,
  PackPatch,
  InboxItemView,
  IngestOutcome,
  RescanResult,
  AccountView,
  CreateAccountInput,
  AccountPatch,
  SheetSummary,
  SheetDetail,
  TaskRowView,
  TaskRowPatch,
  AddTaskRowInput,
  ShortageItem,
  ExportResult,
  PreflightReport,
  PackHistoryItem,
  PreviewDay,
  PreviewEntry,
  CalendarDay,
  BriefView,
  DashboardView,
  PlatformStat,
  AccountStat,
  ReconcileResult,
  ReportView,
  ReportFail,
  SuspectOutcome,
  PublishBadgesEvent,
  InboxIngestEvent,
  SheetChangedEvent,
  ExportProgressEvent,
  // 视频流水线（图生视频）
  ClipView,
  StageCounts,
  V2vSettings,
  V2vChanged,
  V2vProgress,
  V2vActivity,
  V2vTick,
  ActivityEntry,
  ModelInfo,
  ResPrice,
  SessionInfo,
  CreditInfo,
  CreditStats,
  EffectiveParams,
  QueueStats,
  AutofillCfg,
  AutofillStatus,
  AwayDigest,
  HandoffStatus,
  V2vAction,
  V2vUndoEntry,
  SubmitSummary,
  SubmitPreview,
  MaterializeSummary,
  IngestSummary,
  // 生图工单收件（Claude Code / Codex 投单）
  IntakeSettings,
  IntakeChanged,
  IntakeProgress,
  JobView,
  JobPreview,
  JobPreviewRef,
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
 * 订阅参考图导入进度（生成页上传 / 图库批量上传共用）。
 * 返回反订阅函数；非 Tauri 环境为 no-op。
 */
export async function subscribeRefImportProgress(
  handler: (e: import("./bindings").RefImportProgress) => void,
): Promise<() => void> {
  if (!isTauri()) return () => {};
  const un = await events.refImportProgress.listen((e) => handler(e.payload));
  return () => un();
}

/**
 * 订阅发布模块事件（徽章 / 收件箱收录 / 任务单变化）。
 * 返回反订阅函数；非 Tauri 环境为 no-op。
 */
export async function subscribePublish(handlers: {
  onBadges?: (e: import("./bindings").PublishBadgesEvent) => void;
  onInboxIngest?: (e: import("./bindings").InboxIngestEvent) => void;
  onSheetChanged?: (e: import("./bindings").SheetChangedEvent) => void;
}): Promise<() => void> {
  if (!isTauri()) return () => {};
  const unlisteners = await Promise.all([
    events.publishBadgesEvent.listen((e) => handlers.onBadges?.(e.payload)),
    events.inboxIngestEvent.listen((e) => handlers.onInboxIngest?.(e.payload)),
    events.sheetChangedEvent.listen((e) => handlers.onSheetChanged?.(e.payload)),
  ]);
  return () => {
    for (const un of unlisteners) un();
  };
}

/**
 * 订阅任务包导出进度（复制视频可达数百 MB，导出期间需要交代进度）。
 * 仅导出弹窗打开期间临时挂载。
 */
export async function subscribeExportProgress(
  handler: (e: import("./bindings").ExportProgressEvent) => void,
): Promise<() => void> {
  if (!isTauri()) return () => {};
  const un = await events.exportProgressEvent.listen((e) => handler(e.payload));
  return () => un();
}

/**
 * 订阅生图工单收件事件。
 *
 * 收录是**自动**发生的（skill 投单 → watcher 收录 → 建批开跑），用户没有按任何按钮，
 * 所以这条订阅挂在应用外壳上而不是设置页：一个批次凭空出现在任务页，
 * 和一份工单静默失败，都需要一句解释——而人当时未必正好停在设置页。
 */
export async function subscribeIntake(
  handler: (e: import("./bindings").IntakeChanged) => void,
): Promise<() => void> {
  if (!isTauri()) return () => {};
  const un = await events.intakeChanged.listen((e) => handler(e.payload));
  return () => un();
}

/**
 * 订阅工单收录进度（逐张参考图落盘）。
 *
 * 确认一份大工单要几十秒，这条事件是那段时间里唯一的动静 ——
 * 没有它，「看起来卡死了」的下一步永远是再点一次。
 */
export async function subscribeIntakeProgress(
  handler: (e: import("./bindings").IntakeProgress) => void,
): Promise<() => void> {
  if (!isTauri()) return () => {};
  const un = await events.intakeProgress.listen((e) => handler(e.payload));
  return () => un();
}

/**
 * 订阅视频流水线事件（阶段变化 / 已提交条目的轮询进度）。
 * 返回反订阅函数；非 Tauri 环境为 no-op。
 */
export async function subscribeV2v(handlers: {
  onChanged?: (e: import("./bindings").V2vChanged) => void;
  onProgress?: (e: import("./bindings").V2vProgress) => void;
  onActivity?: (e: import("./bindings").V2vActivity) => void;
  onTick?: (e: import("./bindings").V2vTick) => void;
}): Promise<() => void> {
  if (!isTauri()) return () => {};
  const unlisteners = await Promise.all([
    events.v2vChanged.listen((e) => handlers.onChanged?.(e.payload)),
    events.v2vProgress.listen((e) => handlers.onProgress?.(e.payload)),
    events.v2vActivity.listen((e) => handlers.onActivity?.(e.payload)),
    events.v2vTick.listen((e) => handlers.onTick?.(e.payload)),
  ]);
  return () => {
    for (const un of unlisteners) un();
  };
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

/** 拖放位置（物理像素，需除以 devicePixelRatio 才是 CSS 像素）。 */
export interface DropPosition {
  x: number;
  y: number;
}

/**
 * 订阅带**位置**的文件拖放（F7 拖放直投）。
 *
 * Tauri 的原生拖放不经过 DOM，`dragover`/`drop` 那套 DOM 事件根本不会触发；
 * 想知道「用户把文件拖到了哪一行」，只能靠事件里的窗口坐标做命中测试。
 * 坐标是物理像素，调用方需自行换算（见 `AssetsPage` 的 `hitTestSku`）。
 */
export async function subscribeFileDropWithPosition(handlers: {
  onOver?: (pos: DropPosition) => void;
  onLeave?: () => void;
  onDrop?: (paths: string[], pos: DropPosition) => void;
}): Promise<() => void> {
  if (!isTauri()) return () => {};
  const { getCurrentWebview } = await import("@tauri-apps/api/webview");
  const un = await getCurrentWebview().onDragDropEvent((event) => {
    const p = event.payload;
    if (p.type === "over") handlers.onOver?.(p.position);
    else if (p.type === "drop") handlers.onDrop?.(p.paths, p.position);
    else handlers.onLeave?.();
  });
  return () => un();
}
