import { type Settings, type SettingsPatch, commands, unwrap } from "@/lib/ipc";
import { create } from "zustand";

/** 设置 store（执行计划 1.3）：镜像 Rust 侧设置，读写往返类型安全。 */
interface SettingsState {
  settings: Settings | null;
  loading: boolean;
  error: string | null;
  saving: number;
  load: () => Promise<void>;
  update: (patch: Partial<SettingsPatch>) => Promise<boolean>;
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
  onboarded: null,
  notifications: null,
  resetScheduling: false,
};

export const useSettingsStore = create<SettingsState>((set) => ({
  settings: null,
  loading: false,
  error: null,
  saving: 0,

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
    set((current) => ({ saving: current.saving + 1, error: null }));
    try {
      const settings = await unwrap(commands.updateSettings({ ...EMPTY_PATCH, ...patch }));
      set({ settings });
      return true;
    } catch (e) {
      set({ error: e instanceof Error ? e.message : String(e) });
      return false;
    } finally {
      set((current) => ({ saving: current.saving - 1 }));
    }
  },
}));
