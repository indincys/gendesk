import type { TaskProgress, TaskStatusChanged, TaskView } from "@/lib/ipc/bindings";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

type EngineHandlers = {
  onProgress?: (event: TaskProgress) => void;
  onStatus?: (event: TaskStatusChanged) => void;
};

const ipc = vi.hoisted(() => ({
  handlers: null as EngineHandlers | null,
  unsubscribe: vi.fn(),
}));

vi.mock("@/lib/ipc", () => ({
  commands: {
    getSettings: vi.fn(async () => ({ paused: false })),
  },
  unwrap: async <T>(promise: Promise<T>) => promise,
  subscribeEngine: vi.fn(async (handlers: EngineHandlers) => {
    ipc.handlers = handlers;
    return ipc.unsubscribe;
  }),
}));

import { useEngineStore } from "@/stores/engine";

function task(id: number, status = "run"): TaskView {
  return {
    id,
    batchId: 1,
    status,
    refImageId: 1,
    refName: `ref-${id}`,
    promptId: id,
    promptCode: `P-${id}`,
    promptTitle: null,
    groupName: "group",
    apiKeyId: 1,
    keyAlias: "key",
    errorType: null,
    errorMessage: null,
    retryCount: 0,
    resultThumbPath: null,
    promptTextSnapshot: "prompt",
  };
}

beforeEach(() => {
  vi.useFakeTimers();
  ipc.handlers = null;
  ipc.unsubscribe.mockClear();
  useEngineStore.setState({
    summaries: {},
    progress: {},
    keyHealth: {},
    paused: false,
    autoPauseReason: null,
    trashCount: 0,
    updateReady: false,
    updateVersion: null,
    tasks: [],
  });
});

afterEach(() => {
  vi.runOnlyPendingTimers();
  vi.useRealTimers();
});

describe("engine 高频事件合并", () => {
  it("把 250 个任务的突发进度压成一次 store 更新", async () => {
    const tasks = Array.from({ length: 250 }, (_, index) => task(index + 1));
    useEngineStore.setState({ tasks });
    const cleanup = await useEngineStore.getState().init();
    let updates = 0;
    const stop = useEngineStore.subscribe(() => updates++);

    for (const item of tasks) {
      ipc.handlers?.onProgress?.({ taskId: item.id, pct: 10, phase: "generating" });
      ipc.handlers?.onProgress?.({ taskId: item.id, pct: 20, phase: "generating" });
      ipc.handlers?.onProgress?.({ taskId: item.id, pct: 30, phase: "generating" });
      ipc.handlers?.onProgress?.({ taskId: item.id, pct: 40, phase: "generating" });
    }

    expect(updates).toBe(0);
    await vi.advanceTimersByTimeAsync(100);
    expect(updates).toBe(1);
    expect(Object.keys(useEngineStore.getState().progress)).toHaveLength(250);
    expect(useEngineStore.getState().progress[250]).toEqual({ pct: 40, phase: "generating" });

    stop();
    cleanup();
  });

  it("终态之后到达的旧进度不会复活", async () => {
    useEngineStore.setState({
      tasks: [task(1)],
      progress: { 1: { pct: 80, phase: "generating" } },
    });
    const cleanup = await useEngineStore.getState().init();

    ipc.handlers?.onStatus?.({
      taskId: 1,
      batchId: 1,
      status: "fail",
      errorType: null,
      errorMessage: "failed",
      retryCount: 0,
      apiKeyId: 1,
    });
    await vi.advanceTimersByTimeAsync(100);
    expect(useEngineStore.getState().tasks[0]?.status).toBe("fail");
    expect(useEngineStore.getState().progress).toEqual({});

    ipc.handlers?.onProgress?.({ taskId: 1, pct: 90, phase: "downloading" });
    await vi.advanceTimersByTimeAsync(100);
    expect(useEngineStore.getState().progress).toEqual({});

    cleanup();
  });

  it("反订阅会取消尚未落库的合并窗口", async () => {
    useEngineStore.setState({ tasks: [task(1)] });
    const cleanup = await useEngineStore.getState().init();
    ipc.handlers?.onProgress?.({ taskId: 1, pct: 50, phase: "generating" });

    cleanup();
    await vi.advanceTimersByTimeAsync(100);

    expect(ipc.unsubscribe).toHaveBeenCalledTimes(1);
    expect(useEngineStore.getState().progress).toEqual({});
  });
});
