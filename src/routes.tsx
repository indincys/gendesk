import { GeneratePage } from "@/features/generate/GeneratePage";
import { ReviewPage } from "@/features/review/ReviewPage";
import { SettingsPage } from "@/features/settings/SettingsPage";
import { TasksPage } from "@/features/tasks/TasksPage";
import { TrashPage } from "@/features/trash/TrashPage";
import { V2vClipsPage } from "@/features/v2v/V2vClipsPage";
import { V2vPage } from "@/features/v2v/V2vPage";
import { WorksPage } from "@/features/works/WorksPage";
import type { ComponentType } from "react";

/** 页面路由键。 */
export type RouteKey =
  | "generate"
  | "tasks"
  | "review"
  | "v2v"
  | "clips"
  | "library"
  | "trash"
  | "settings";

export type NavGroup = "make" | "asset" | "system";

/**
 * 一条路由。
 *
 * **没有 `icon`**：侧栏的行内图标在 v0.24.0 去掉了（新侧栏是纯文字 + 分组色轨），
 * 而全仓只有侧栏读过那个字段 —— 留一个没人读的字段，等于让下一个人以为哪儿还在画图标。
 */
export interface RouteDef {
  key: RouteKey;
  label: string;
  /**
   * ⌘/Ctrl+N 数字；`null` = 无数字快捷键（十个数字已用尽，新页只能从侧栏进）。
   *
   * 不为了给新页腾位而重排既有数字：那会把每个人的肌肉记忆一次性作废，
   * 代价远大于「少一个快捷键」。
   */
  shortcut: number | null;
  group: NavGroup;
  component: ComponentType;
}

/** 路由注册表 —— 命令面板、侧栏、快捷键均从此单一来源派生。 */
export const ROUTES: readonly RouteDef[] = [
  {
    key: "generate",
    label: "图片生成",
    shortcut: 1,
    group: "make",
    component: GeneratePage,
  },
  {
    key: "tasks",
    label: "任务队列",
    shortcut: 2,
    group: "make",
    component: TasksPage,
  },
  {
    key: "review",
    label: "图片验收",
    shortcut: 3,
    group: "make",
    component: ReviewPage,
  },
  {
    key: "v2v",
    label: "视频生成",
    shortcut: null,
    group: "make",
    component: V2vPage,
  },
  {
    key: "clips",
    label: "视频成片",
    shortcut: null,
    group: "asset",
    component: V2vClipsPage,
  },
  {
    key: "library",
    label: "作品库",
    shortcut: 4,
    group: "asset",
    component: WorksPage,
  },
  // ⌘5（原提示词库）、⌘6（原参考图库）、⌘9、⌘0（原素材库与任务单）已随页面移除。
  // **空着不复用、不把后面的数字往前挪**：重排会把每个人的肌肉记忆一次性作废。
  {
    key: "trash",
    label: "废纸篓",
    shortcut: 7,
    group: "system",
    component: TrashPage,
  },
  {
    key: "settings",
    label: "设置",
    shortcut: 8,
    group: "system",
    component: SettingsPage,
  },
] as const;

export const ROUTE_BY_KEY: Record<RouteKey, RouteDef> = Object.fromEntries(
  ROUTES.map((r) => [r.key, r]),
) as Record<RouteKey, RouteDef>;

/** ⌘1–8 数字 → 路由键（快捷键分发用）。无数字快捷键的页面不进此表。 */
export const ROUTE_BY_SHORTCUT: Record<number, RouteKey> = Object.fromEntries(
  ROUTES.filter((r) => r.shortcut !== null).map((r) => [r.shortcut, r.key]),
);
