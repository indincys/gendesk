import type { Settings } from "@/lib/ipc";
import { useSettingsStore } from "@/stores/settings";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, describe, expect, it } from "vitest";

const base: Settings = {
  scheduleStrategy: "round_robin",
  retryCount: 1,
  outputDir: "/out",
  motion: "standard",
  paused: false,
};

afterEach(() => {
  clearMocks();
  useSettingsStore.setState({ settings: null, loading: false, error: null });
});

describe("settings store", () => {
  it("load 通过 IPC 拉取设置", async () => {
    mockIPC((cmd) => {
      if (cmd === "get_settings") return base;
      throw new Error(`未预期命令 ${cmd}`);
    });
    await useSettingsStore.getState().load();
    expect(useSettingsStore.getState().settings).toEqual(base);
    expect(useSettingsStore.getState().error).toBeNull();
  });

  it("update 以 Rust 返回的规整设置为准", async () => {
    mockIPC((cmd, args) => {
      if (cmd === "update_settings") {
        // 模拟 Rust 侧 clamp：retryCount 超界被夹到 3
        const patch = (args as { patch: { retryCount?: number } }).patch;
        return { ...base, retryCount: Math.min(patch.retryCount ?? base.retryCount, 3) };
      }
      throw new Error(`未预期命令 ${cmd}`);
    });
    await useSettingsStore.getState().update({
      scheduleStrategy: null,
      retryCount: 99,
      outputDir: null,
      motion: null,
      paused: null,
    });
    expect(useSettingsStore.getState().settings?.retryCount).toBe(3);
  });
});
