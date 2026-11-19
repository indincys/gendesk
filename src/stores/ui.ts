import type { RouteKey } from "@/routes";
import { create } from "zustand";

/** 平台判定 —— 决定窗口壳（mac 交通灯 / win 自绘窗控）与修饰键显示。 */
export type Platform = "mac" | "win" | "other";

function detectPlatform(): Platform {
  if (typeof navigator === "undefined") return "other";
  const ua = navigator.userAgent;
  if (/Mac|iPhone|iPad/.test(ua)) return "mac";
  if (/Win/.test(ua)) return "win";
  return "other";
}

interface UiState {
  route: RouteKey;
  platform: Platform;
  /** 快捷键速查面板开关（E39） */
  helpOpen: boolean;
  settingsSection: string | null;
  jumpBatch: number | null;
  openBatch: (route: "tasks" | "review" | "library", batchId: number) => void;
  openSettings: (section: string) => void;

  go: (route: RouteKey) => void;
  toggleHelp: () => void;
  closeHelp: () => void;
}

export const useUiStore = create<UiState>((set) => ({
  route: "generate",
  platform: detectPlatform(),
  helpOpen: false,
  settingsSection: null,
  jumpBatch: null,
  openBatch: (route, jumpBatch) => set({ route, jumpBatch, settingsSection: null }),
  openSettings: (settingsSection) => set({ route: "settings", settingsSection }),

  go: (route) => set({ route, settingsSection: null, jumpBatch: null }),
  toggleHelp: () => set((s) => ({ helpOpen: !s.helpOpen })),
  closeHelp: () => set({ helpOpen: false }),
}));

/** 当前平台的修饰键符号（⌘ / Ctrl）。 */
export function modKeyLabel(platform: Platform): string {
  return platform === "win" ? "Ctrl" : "⌘";
}
