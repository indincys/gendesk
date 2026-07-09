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

  currentBatchId: number | null;
  tasks: TaskView[];

  /** 订阅引擎事件（仅 Tauri 环境）。返回反订阅函数。 */
  init: () => Promise<() => void>;
  setCurrentBatch: (batchId: number | null) => void;
  loadBatchTasks: (batchId: number, statusGroup?: string | null) => Promise<void>;
}

export const useEngineStore = create<EngineState>((set) => ({
  summaries: {},
  progress: {},
  keyHealth: {},
  paused: false,
  currentBatchId: null,
  tasks: [],

  init: () =>
    subscribeEngine({
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
    }),

  setCurrentBatch: (batchId) => set({ currentBatchId: batchId }),

  loadBatchTasks: async (batchId, statusGroup) => {
    const tasks = await unwrap(commands.listTasks(batchId, statusGroup ?? null, null));
    set({ currentBatchId: batchId, tasks });
  },
}));

/** 导航徽章计数（事件驱动，不轮询）。 */
export function navBadges(state: EngineState): { running: number; review: number } {
  let running = 0;
  let review = 0;
  for (const s of Object.values(state.summaries)) {
    running += s.running;
    review += s.review;
  }
  return { running, review };
}
