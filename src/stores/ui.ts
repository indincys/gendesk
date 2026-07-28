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
  /** 命令面板开关 */
  paletteOpen: boolean;
  paletteQuery: string;
  /** 快捷键速查面板开关（E39） */
  helpOpen: boolean;
  /**
   * 正在确认的待确认工单 id（`null` = 没有）。
   *
   * 放 store 而不是各自的组件 state：这张卡有两个入口——收到 hold 事件时外壳自动弹，
   * 以及设置页点「查看并确认」。两处各存一份局部状态时，人在设置页收到事件就会
   * **同时挂载两份**同一张卡，于是同一份工单的缩略图被生成两遍。
   */
  holdJob: number | null;

  go: (route: RouteKey) => void;
  setHoldJob: (id: number | null) => void;
  openPalette: () => void;
  closePalette: () => void;
  togglePalette: () => void;
  setPaletteQuery: (q: string) => void;
  toggleHelp: () => void;
  closeHelp: () => void;
}

export const useUiStore = create<UiState>((set) => ({
  route: "generate",
  platform: detectPlatform(),
  paletteOpen: false,
  paletteQuery: "",
  helpOpen: false,
  holdJob: null,

  go: (route) => set({ route, paletteOpen: false, paletteQuery: "" }),
  setHoldJob: (holdJob) => set({ holdJob }),
  openPalette: () => set({ paletteOpen: true, paletteQuery: "" }),
  closePalette: () => set({ paletteOpen: false, paletteQuery: "" }),
  togglePalette: () => set((s) => ({ paletteOpen: !s.paletteOpen, paletteQuery: "" })),
  setPaletteQuery: (q) => set({ paletteQuery: q }),
  toggleHelp: () => set((s) => ({ helpOpen: !s.helpOpen })),
  closeHelp: () => set({ helpOpen: false }),
}));

/** 当前平台的修饰键符号（⌘ / Ctrl）。 */
export function modKeyLabel(platform: Platform): string {
  return platform === "win" ? "Ctrl" : "⌘";
}
