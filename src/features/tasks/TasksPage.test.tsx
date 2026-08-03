import type { TaskView } from "@/lib/ipc/bindings";
import { render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

const engine = vi.hoisted(() => ({
  state: {
    tasks: [] as TaskView[],
    progress: {} as Record<number, { pct: number; phase: string }>,
    paused: false,
    autoPauseReason: null as string | null,
    setPaused: vi.fn(),
    loadBatchTasks: vi.fn(async () => {}),
    dropTasks: vi.fn(),
  },
}));

const ui = vi.hoisted(() => ({ go: vi.fn() }));
const virtual = vi.hoisted(() => ({ count: 0, measureElement: vi.fn() }));

vi.mock("@/stores/engine", () => ({
  useEngineStore: <T,>(selector: (state: typeof engine.state) => T) => selector(engine.state),
}));

vi.mock("@/stores/ui", () => ({
  useUiStore: <T,>(selector: (state: typeof ui) => T) => selector(ui),
}));

vi.mock("@/lib/img", () => ({ assetSrc: () => null }));

vi.mock("@/lib/ipc", () => ({
  commands: {
    countInterrupted: vi.fn(async () => 0),
    estimateTaskSeconds: vi.fn(async () => null),
    listApiKeys: vi.fn(async () => []),
  },
  unwrap: async <T,>(promise: Promise<T>) => promise,
}));

vi.mock("@tanstack/react-virtual", () => ({
  useVirtualizer: (options: { count: number }) => {
    virtual.count = options.count;
    const rendered = Math.min(options.count, 12);
    return {
      measureElement: virtual.measureElement,
      getTotalSize: () => options.count * 40,
      getVirtualItems: () =>
        Array.from({ length: rendered }, (_, index) => ({
          index,
          key: index + 1,
          start: index * 40,
          size: 40,
          end: (index + 1) * 40,
        })),
    };
  },
}));

import { TasksPage } from "@/features/tasks/TasksPage";

function task(id: number): TaskView {
  return {
    id,
    batchId: 1,
    status: "q",
    refImageId: 1,
    refName: `ref-${id}`,
    promptId: id,
    promptCode: `P-${id}`,
    promptTitle: null,
    groupName: "group",
    apiKeyId: null,
    keyAlias: null,
    errorType: null,
    errorMessage: null,
    retryCount: 0,
    resultThumbPath: null,
    promptTextSnapshot: "prompt",
  };
}

describe("TasksPage 虚拟任务表", () => {
  it("500 条任务只挂载视口附近的少量行", () => {
    engine.state.tasks = Array.from({ length: 500 }, (_, index) => task(index + 1));

    const { container } = render(<TasksPage />);

    expect(virtual.count).toBe(500);
    expect(container.querySelectorAll(".taskvrow")).toHaveLength(12);
    expect(container.querySelector<HTMLElement>(".taskvirt")?.style.height).toBe("20000px");
  });
});
