import { type PosterBatch, type PosterSettings, commands, unwrap } from "@/lib/ipc";
import { create } from "zustand";

/** 只镜像后端 Poster 台账与设置；是否自动导入、批次状态都只在 Rust。 */
export const usePosterStore = create<{
  batches: PosterBatch[];
  settings: PosterSettings | null;
  refresh: () => Promise<void>;
}>((set) => ({
  batches: [],
  settings: null,
  refresh: async () => {
    const [batches, settings] = await Promise.all([
      unwrap(commands.listPosterBatches()),
      unwrap(commands.getPosterSettings()),
    ]);
    set({ batches, settings });
  },
}));
