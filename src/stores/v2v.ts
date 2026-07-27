import { type StageCounts, commands, subscribeV2v, unwrap } from "@/lib/ipc";
import { create } from "zustand";

/**
 * 视频流水线的事件镜像（Zustand 只做镜像与 UI 态，业务真相在 Rust）。
 *
 * 存在的理由只有一个：侧栏徽章。看板页自己订阅事件取全量数据，但徽章要在任何页面上
 * 都是对的，所以计数得有一处全局镜像。
 */
interface V2vState {
  counts: StageCounts;
  init: () => Promise<() => void>;
  refresh: () => Promise<void>;
}

const EMPTY: StageCounts = {
  rewrite: 0,
  ready: 0,
  run: 0,
  rev: 0,
  pass: 0,
  rej: 0,
  fail: 0,
  phantom: 0,
  actionable: 0,
  undelivered: 0,
};

export const useV2vStore = create<V2vState>((set, get) => ({
  counts: EMPTY,

  init: async () => {
    await get().refresh();
    // 事件里已带全量计数 → 直接镜像，不再回查一次（铁律 4：事件驱动不轮询）。
    return subscribeV2v({ onChanged: (e) => set({ counts: e.counts }) });
  },

  refresh: async () => {
    try {
      set({ counts: await unwrap(commands.v2vCounts()) });
    } catch {
      // 读不到不该影响别的启动步骤；下一次事件会纠正。
      set({ counts: EMPTY });
    }
  },
}));
