import { type PosterBatch, type PosterSettings, commands, unwrap } from "@/lib/ipc";
import { create } from "zustand";

/** 仅缓存后端交接台账和本次会话暂时收起的弹窗。是否同意导入只在 Rust 台账中。 */
export const usePosterStore = create<{
  batches: PosterBatch[];
  settings: PosterSettings | null;
  deferred: Set<string>;
  refresh: () => Promise<void>;
  defer: (id: string) => void;
  review: (id: string) => void;
}>((set) => ({
  batches: [],
  settings: null,
  deferred: new Set(),
  refresh: async () => {
    const [batches, settings] = await Promise.all([
      unwrap(commands.listPosterBatches()),
      unwrap(commands.getPosterSettings()),
    ]);
    set({ batches, settings });
  },
  defer: (id) => set((s) => ({ deferred: new Set([...s.deferred, id]) })),
  review: (id) =>
    set((s) => {
      const deferred = new Set(s.deferred);
      deferred.delete(id);
      return { deferred };
    }),
}));
