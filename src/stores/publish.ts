import { type PublishBadges, commands, subscribePublish, unwrap } from "@/lib/ipc";
import { create } from "zustand";

/** 单条收录 toast（收件箱事件镜像）。 */
export interface IngestToast {
  id: number;
  fileName: string;
  state: string;
  text: string;
}

interface PublishState {
  badges: PublishBadges;
  toasts: IngestToast[];
  init: () => Promise<() => void>;
  refreshBadges: () => Promise<void>;
  dismissToast: (id: number) => void;
}

const EMPTY: PublishBadges = {
  unclaimed: 0,
  warn: 0,
  pendingSheets: 0,
  pendingReconcile: 0,
};

let toastSeq = 1;

export const usePublishStore = create<PublishState>((set, get) => ({
  badges: EMPTY,
  toasts: [],

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
        let text = "";
        if (o.state === "ingested") {
          text = `${o.skuCode} 入库：标题 ×${o.titles} 正文 ×${o.bodies}`;
        } else if (o.state === "unclaimed") {
          text = "待认领：未识别到已知 SKU";
        } else if (o.state === "failed") {
          text = `解析失败：${o.reason}`;
        }
        const toast: IngestToast = {
          id: toastSeq++,
          fileName: e.fileName,
          state: o.state,
          text,
        };
        set((s) => ({ toasts: [...s.toasts, toast].slice(-4) }));
        // 自动淡出
        setTimeout(() => get().dismissToast(toast.id), 6000);
      },
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

  dismissToast: (id) => set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) })),
}));
