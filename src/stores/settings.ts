import { type Settings, type SettingsPatch, commands, unwrap } from "@/lib/ipc";
import { create } from "zustand";

/** 设置 store（执行计划 1.3）：镜像 Rust 侧设置，读写往返类型安全。 */
interface SettingsState {
  settings: Settings | null;
  loading: boolean;
  error: string | null;
  load: () => Promise<void>;
  update: (patch: Partial<SettingsPatch>) => Promise<void>;
}

/** 全字段 null 的补丁基底：仅传入变更字段即可，其余保持不变。 */
const EMPTY_PATCH: SettingsPatch = {
  scheduleStrategy: null,
  retryCount: null,
  outputDir: null,
  motion: null,
  paused: null,
  globalFailThreshold: null,
  trashRetentionDays: null,
  batchRetentionDays: null,
};

export const useSettingsStore = create<SettingsState>((set) => ({
  settings: null,
  loading: false,
  error: null,

  load: async () => {
    set({ loading: true, error: null });
    try {
      const settings = await unwrap(commands.getSettings());
      set({ settings, loading: false });
    } catch (e) {
      set({ loading: false, error: e instanceof Error ? e.message : String(e) });
    }
  },

  update: async (patch) => {
    // 乐观：以 Rust 返回的规整后设置为准（含 clamp / 枚举纠偏）。
    try {
      const settings = await unwrap(commands.updateSettings({ ...EMPTY_PATCH, ...patch }));
      set({ settings });
    } catch (e) {
      set({ error: e instanceof Error ? e.message : String(e) });
    }
  },
}));
