import {
  type PublishBadges,
  type SheetChangedEvent,
  commands,
  subscribePublish,
  unwrap,
} from "@/lib/ipc";
import { toast } from "sonner";
import { create } from "zustand";

interface PublishState {
  badges: PublishBadges;
  /**
   * 事件驱动刷新的版本号（铁律：事件驱动不轮询）。后端每发一次对应事件就自增，
   * 页面把它放进 `useEffect` 依赖里即可自动重载——比让每个页面各自订阅事件简单，
   * 也不会漏掉某个页面。
   */
  sheetRev: number;
  inboxRev: number;
  /** 最近一次任务单变化事件（工作台据此判断变的是不是自己那一单）。 */
  lastSheetChanged: SheetChangedEvent | null;
  init: () => Promise<() => void>;
  refreshBadges: () => Promise<void>;
}

const EMPTY: PublishBadges = {
  unclaimed: 0,
  warn: 0,
  pendingSheets: 0,
  pendingReconcile: 0,
};

export const usePublishStore = create<PublishState>((set, get) => ({
  badges: EMPTY,
  sheetRev: 0,
  inboxRev: 0,
  lastSheetChanged: null,

  init: async () => {
    await get().refreshBadges();
    const un = await subscribePublish({
      onBadges: (b) =>
        set({
          badges: {
            unclaimed: b.unclaimed,
            warn: b.warn,
            pendingSheets: b.pendingSheets,
            pendingReconcile: b.pendingReconcile,
          },
        }),

      onInboxIngest: (e) => {
        const o = e.outcome;
        if (o.state === "ingested") {
          toast.success(`${o.skuCode} 入库：标题 ×${o.titles} 正文 ×${o.bodies}`, {
            description: e.fileName,
          });
        } else if (o.state === "ingestedMedia") {
          toast.success(`${o.skuCode} 入库：素材包 ×${o.packs}`, { description: e.fileName });
        } else if (o.state === "unclaimed") {
          toast.warning("待认领：未识别到已知 SKU", { description: e.fileName });
        } else if (o.state === "unclaimedMedia") {
          toast.warning(`待认领：${o.folder}（${o.files} 个媒体文件）`, {
            description: "到资产库 › 收件箱指认 SKU",
          });
        } else if (o.state === "failed") {
          toast.error(`解析失败：${o.reason}`, { description: e.fileName });
        }
        set((s) => ({ inboxRev: s.inboxRev + 1 }));
      },

      onSheetChanged: (e) => set((s) => ({ sheetRev: s.sheetRev + 1, lastSheetChanged: e })),
    });
    return un;
  },

  refreshBadges: async () => {
    try {
      const b = await unwrap(commands.getPublishBadges());
      set({ badges: b });
    } catch {
      // 首次未配置根目录等：徽章保持为空即可。
    }
  },
}));
