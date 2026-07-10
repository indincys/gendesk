import { type TaskView, commands, subscribeEngine, unwrap } from "@/lib/ipc";
import { create } from "zustand";

interface BatchSummaryState {
  pending: number;
  running: number;
  failed: number;
  review: number;
  passed: number;
  rejected: number;
  total: number;
  activeConcurrency: number;
  paused: boolean;
}

interface TaskProgressState {
  pct: number;
  phase: string;
}

interface EngineState {
  summaries: Record<number, BatchSummaryState>;
  progress: Record<number, TaskProgressState>;
  keyHealth: Record<number, string>;
  paused: boolean;
  trashCount: number;

  /** 应用内更新态 */
  updateReady: boolean;
  updateVersion: string | null;
  updateChecking: boolean;

  currentBatchId: number | null;
  tasks: TaskView[];

  /** 订阅引擎事件（仅 Tauri 环境）。返回反订阅函数。 */
  init: () => Promise<() => void>;
  /** 乐观设置暂停态（暂停/继续命令不会立即回推汇总事件，需前端即时反映）。 */
  setPaused: (paused: boolean) => void;
  setCurrentBatch: (batchId: number | null) => void;
  loadBatchTasks: (batchId: number, statusGroup?: string | null) => Promise<void>;
  /** 刷新废纸篓徽章计数（切页/清理后调用；非轮询）。 */
  refreshBadgeCounts: () => Promise<void>;
}

export const useEngineStore = create<EngineState>((set) => ({
  summaries: {},
  progress: {},
  keyHealth: {},
  paused: false,
  trashCount: 0,
  updateReady: false,
  updateVersion: null,
  updateChecking: false,
  currentBatchId: null,
  tasks: [],

  init: async () => {
    // 启动时同步持久化的暂停态：暂停下引擎不派发、不回推汇总事件，若不主动拉取，
    // 页脚/工具栏按钮会一直显示为「运行中」，用户新建批次也不会跑却毫无提示。
    try {
      const s = await unwrap(commands.getSettings());
      set({ paused: s.paused });
    } catch {
      // 忽略：拉取失败不阻断事件订阅
    }
    return subscribeEngine({
      onSummary: (p) =>
        set((s) => ({
          summaries: {
            ...s.summaries,
            [p.batchId]: {
              ...p.counts,
              activeConcurrency: p.activeConcurrency,
              paused: p.paused,
            },
          },
          paused: p.paused,
        })),
      onProgress: (p) =>
        set((s) => ({ progress: { ...s.progress, [p.taskId]: { pct: p.pct, phase: p.phase } } })),
      onStatus: (p) =>
        set((s) => {
          if (p.batchId !== s.currentBatchId) return {};
          return {
            tasks: s.tasks.map((t) =>
              t.id === p.taskId
                ? {
                    ...t,
                    status: p.status,
                    errorType: p.errorType,
                    errorMessage: p.errorMessage,
                    retryCount: p.retryCount,
                    apiKeyId: p.apiKeyId,
                  }
                : t,
            ),
          };
        }),
      onKeyHealth: (p) => set((s) => ({ keyHealth: { ...s.keyHealth, [p.keyId]: p.state } })),
      onUpdateState: (p) =>
        set({
          updateChecking: p.state === "checking" || p.state === "downloading",
          updateReady: p.state === "ready",
          updateVersion: p.version,
        }),
    });
  },

  setPaused: (paused) => set({ paused }),

  setCurrentBatch: (batchId) => set({ currentBatchId: batchId }),

  loadBatchTasks: async (batchId, statusGroup) => {
    const tasks = await unwrap(commands.listTasks(batchId, statusGroup ?? null, null));
    set({ currentBatchId: batchId, tasks });
  },

  refreshBadgeCounts: async () => {
    try {
      set({ trashCount: await unwrap(commands.countTrash()) });
    } catch {
      // 忽略：徽章计数失败不影响主流程
    }
  },
}));

/** 导航徽章计数（事件驱动 + 切页刷新，不轮询）。 */
export function navBadges(state: EngineState): { running: number; review: number; trash: number } {
  let running = 0;
  let review = 0;
  for (const s of Object.values(state.summaries)) {
    running += s.running;
    review += s.review;
  }
  return { running, review, trash: state.trashCount };
}
