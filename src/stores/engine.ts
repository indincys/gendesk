import {
  type BatchSummary,
  type KeyHealth,
  type TaskProgress,
  type TaskStatusChanged,
  type TaskView,
  commands,
  subscribeEngine,
  unwrap,
} from "@/lib/ipc";
import { create } from "zustand";

/** 把最高约 2,000 次/秒的逐任务事件压成至多 10 次 store 更新/秒。 */
const ENGINE_FLUSH_MS = 100;

function isProgressActive(status: string): boolean {
  return status === "run" || status === "retry";
}

interface BatchSummaryState {
  pending: number;
  running: number;
  failed: number;
  review: number;
  passed: number;
  rejected: number;
  total: number;
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
  /** 自动暂停原因（E05 全局熔断）；null = 非自动暂停。 */
  autoPauseReason: string | null;
  trashCount: number;

  /** 应用内更新态 */
  updateReady: boolean;
  updateVersion: string | null;
  tasks: TaskView[];

  /** 订阅引擎事件（仅 Tauri 环境）。返回反订阅函数。 */
  init: () => Promise<() => void>;
  /** 乐观设置暂停态（暂停/继续命令不会立即回推汇总事件，需前端即时反映）。 */
  setPaused: (paused: boolean) => void;
  /**
   * 拉任务列表。`batchId = null` = 全部批次，这是现在唯一的用法：
   * 批次不再是可切换的对象（v0.21.0），任务队列答的是「现在还有哪些活」。
   */
  loadBatchTasks: (batchId: number | null, statusGroup?: string | null) => Promise<void>;
  dropTasks: (ids: number[]) => void;
  /** 刷新废纸篓徽章计数（切页/清理后调用；非轮询）。 */
  refreshBadgeCounts: () => Promise<void>;
}

export const useEngineStore = create<EngineState>((set) => ({
  summaries: {},
  progress: {},
  keyHealth: {},
  paused: false,
  autoPauseReason: null,
  trashCount: 0,
  updateReady: false,
  updateVersion: null,
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
    // Tauri 事件回调发生在 WebView 主线程。250 并发下逐条 set 会复制进度表、跑 React
    // 订阅并重绘几百行；实跑热重载叠到 509 时约 2,000 次/秒，连暂停按钮都点不动。
    // 四类高频事件先按实体取最后一条，再在同一个 100ms flush 里一次性落 store。
    const pendingProgress = new Map<number, TaskProgress>();
    const pendingStatus = new Map<number, TaskStatusChanged>();
    const pendingSummaries = new Map<number, BatchSummary>();
    const pendingKeyHealth = new Map<number, KeyHealth>();
    let flushTimer: ReturnType<typeof setTimeout> | null = null;

    const flush = () => {
      flushTimer = null;
      if (
        pendingProgress.size === 0 &&
        pendingStatus.size === 0 &&
        pendingSummaries.size === 0 &&
        pendingKeyHealth.size === 0
      )
        return;

      const progresses = new Map(pendingProgress);
      const statuses = new Map(pendingStatus);
      const summaries = new Map(pendingSummaries);
      const health = new Map(pendingKeyHealth);
      pendingProgress.clear();
      pendingStatus.clear();
      pendingSummaries.clear();
      pendingKeyHealth.clear();

      set((s) => {
        let tasks = s.tasks;
        if (statuses.size > 0) {
          tasks = tasks.map((task) => {
            const event = statuses.get(task.id);
            return event
              ? {
                  ...task,
                  status: event.status,
                  errorType: event.errorType,
                  errorMessage: event.errorMessage,
                  retryCount: event.retryCount,
                  apiKeyId: event.apiKeyId,
                }
              : task;
          });
        }

        let summaryState = s.summaries;
        let paused = s.paused;
        let autoPauseReason = s.autoPauseReason;
        if (summaries.size > 0) {
          summaryState = { ...summaryState };
          for (const event of summaries.values()) {
            summaryState[event.batchId] = { ...event.counts, paused: event.paused };
            paused = event.paused;
            autoPauseReason = event.autoPauseReason;
            // 批次全终态后数据库会归档；只留下仍需人工处理的 fail。
            const resolved =
              event.counts.pending === 0 && event.counts.running === 0 && event.counts.review === 0;
            if (resolved) {
              tasks = tasks.filter(
                (task) => task.batchId !== event.batchId || task.status === "fail",
              );
            }
          }
        }

        let progress = s.progress;
        if (progresses.size > 0 || statuses.size > 0 || summaries.size > 0) {
          progress = { ...progress };
          // 跨事件通道不假定到达顺序：终态 status 可能先到，而上一拍 progress 后到。
          // 以合并后的任务镜像为准，只允许仍在 run/retry 的 id 留在进度表。
          const active = new Set(
            tasks.filter((task) => isProgressActive(task.status)).map((task) => task.id),
          );
          for (const [taskId, event] of progresses) {
            if (active.has(taskId)) {
              progress[taskId] = { pct: event.pct, phase: event.phase };
            }
          }
          // 旧实现永不删除；终态、归档或列表刷新后会把整张表永久留在 WebView。
          for (const id of Object.keys(progress)) {
            if (!active.has(Number(id))) delete progress[Number(id)];
          }
        }

        let keyHealth = s.keyHealth;
        if (health.size > 0) {
          keyHealth = { ...keyHealth };
          for (const event of health.values()) keyHealth[event.keyId] = event.state;
        }

        return {
          summaries: summaryState,
          progress,
          keyHealth,
          tasks,
          paused,
          autoPauseReason,
        };
      });
    };

    const scheduleFlush = () => {
      if (flushTimer === null) flushTimer = setTimeout(flush, ENGINE_FLUSH_MS);
    };

    const unsubscribe = await subscribeEngine({
      onSummary: (event) => {
        pendingSummaries.set(event.batchId, event);
        scheduleFlush();
      },
      onProgress: (event) => {
        pendingProgress.set(event.taskId, event);
        scheduleFlush();
      },
      // 批次不再是可切换对象：全部批次状态变化都进入同一个合并窗口。
      onStatus: (event) => {
        pendingStatus.set(event.taskId, event);
        scheduleFlush();
      },
      onKeyHealth: (event) => {
        pendingKeyHealth.set(event.keyId, event);
        scheduleFlush();
      },
      onUpdateState: (p) =>
        set({
          updateReady: p.state === "ready",
          updateVersion: p.version,
        }),
    });
    return () => {
      unsubscribe();
      if (flushTimer !== null) clearTimeout(flushTimer);
      flushTimer = null;
      pendingProgress.clear();
      pendingStatus.clear();
      pendingSummaries.clear();
      pendingKeyHealth.clear();
    };
  },

  // 继续队列即消费自动暂停：乐观清除原因（后端 resume 同步清除并回推 summary）。
  setPaused: (paused) => set(paused ? { paused } : { paused, autoPauseReason: null }),

  loadBatchTasks: async (batchId, statusGroup) => {
    const tasks = await unwrap(commands.listTasks(batchId, statusGroup ?? null, null));
    set((s) => {
      const active = new Set(
        tasks.filter((task) => isProgressActive(task.status)).map((task) => task.id),
      );
      return {
        tasks,
        progress: Object.fromEntries(
          Object.entries(s.progress).filter(([id]) => active.has(Number(id))),
        ),
      };
    });
  },

  /** 清掉某几个任务的本地镜像（批量中止/删除之后立刻生效，不必等重拉）。 */
  dropTasks: (ids) =>
    set((s) => {
      const dropped = new Set(ids);
      return {
        tasks: s.tasks.filter((task) => !dropped.has(task.id)),
        progress: Object.fromEntries(
          Object.entries(s.progress).filter(([id]) => !dropped.has(Number(id))),
        ),
      };
    }),

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
