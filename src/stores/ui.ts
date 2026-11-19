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

type QueueLocation = { filter: string; error: string | null; search: string; top: number };

interface UiState {
  route: RouteKey;
  platform: Platform;
  /** 快捷键速查面板开关（E39） */
  helpOpen: boolean;
  onboardingOpen: boolean;
  setOnboardingOpen: (open: boolean) => void;
  settingsSection: string | null;
  jumpBatch: number | null;
  jumpTask: number | null;
  queueLocation: QueueLocation | null;
  returnToQueue: boolean;
  rememberQueue: (location: QueueLocation) => void;
  goBackToQueue: () => void;
  jumpClip: number | null;
  workStyle: { style: string; product: string } | null;
  jumpWork: number | null;
  jumpTrash: number | null;
  openTask: (route: "tasks" | "review", batchId: number, id: number) => void;
  openClip: (route: "v2v" | "clips", id: number) => void;
  openStyleWorks: (style: string, product: string) => void;
  openWork: (id: number) => void;
  openTrash: (id: number) => void;
  openBatch: (route: "tasks" | "review" | "library", batchId: number) => void;
  openSettings: (section: string) => void;

  go: (route: RouteKey) => void;
  openHelp: () => void;
  toggleHelp: () => void;
  closeHelp: () => void;
}

const resetTarget = {
  jumpBatch: null,
  jumpTask: null,
  jumpClip: null,
  jumpWork: null,
  workStyle: null,
  jumpTrash: null,
  settingsSection: null,
};

export const useUiStore = create<UiState>((set) => ({
  route: "generate",
  platform: detectPlatform(),
  helpOpen: false,
  onboardingOpen: false,
  queueLocation: null,
  returnToQueue: false,
  rememberQueue: (queueLocation) => set({ queueLocation, returnToQueue: true }),
  goBackToQueue: () => set({ ...resetTarget, route: "tasks", returnToQueue: false }),
  setOnboardingOpen: (onboardingOpen) => set({ onboardingOpen, helpOpen: false }),
  ...resetTarget,
  openBatch: (route, jumpBatch) => set({ ...resetTarget, route, jumpBatch }),
  openTask: (route, jumpBatch, jumpTask) => set({ ...resetTarget, route, jumpBatch, jumpTask }),
  openClip: (route, jumpClip) => set({ ...resetTarget, route, jumpClip }),
  openStyleWorks: (style, product) =>
    set({ ...resetTarget, route: "library", workStyle: { style, product } }),
  openWork: (jumpWork) => set({ ...resetTarget, route: "library", jumpWork }),
  openTrash: (jumpTrash) => set({ ...resetTarget, route: "trash", jumpTrash }),
  openSettings: (settingsSection) => set({ ...resetTarget, route: "settings", settingsSection }),

  go: (route) => set({ ...resetTarget, route, returnToQueue: false }),
  openHelp: () => set({ helpOpen: true }),
  toggleHelp: () => set((s) => ({ helpOpen: !s.helpOpen })),
  closeHelp: () => set({ helpOpen: false }),
}));

/** 当前平台的修饰键符号（⌘ / Ctrl）。 */
export function modKeyLabel(platform: Platform): string {
  return platform === "win" ? "Ctrl" : "⌘";
}
