import { AssetsPage } from "@/features/assets/AssetsPage";
import { GeneratePage } from "@/features/generate/GeneratePage";
import { PlanPage } from "@/features/plan/PlanPage";
import { RefsPage } from "@/features/refs/RefsPage";
import { ReviewPage } from "@/features/review/ReviewPage";
import { SettingsPage } from "@/features/settings/SettingsPage";
import { TasksPage } from "@/features/tasks/TasksPage";
import { TrashPage } from "@/features/trash/TrashPage";
import { V2vClipsPage } from "@/features/v2v/V2vClipsPage";
import { V2vPage } from "@/features/v2v/V2vPage";
import { WorksPage } from "@/features/works/WorksPage";
import type { ComponentType } from "react";

/** 页面路由键 —— 顺序即快捷键序（⌘1–8 制作/资产，⌘9 资产库，⌘0 发布计划）。 */
export type RouteKey =
  | "generate"
  | "tasks"
  | "review"
  | "v2v"
  | "clips"
  | "library"
  | "refs"
  | "assets"
  | "plan"
  | "trash"
  | "settings";

export type NavGroup = "make" | "asset" | "publish" | "system";

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
  // 「提示词库」（原 ⌘5）已整页移除：提示词是消耗品，跑完即随批次一起删掉，
  // 没有可长期浏览的库。导入仍在生成页内完成，或由 skill 投工单送进来。
  // **⌘5 就此空着，不把后面的数字往前挪**：重排会把每个人的肌肉记忆一次性作废
  // （同 v0.15.0 给新页不排数字的理由，代价远大于少一个快捷键）。
  { key: "refs", label: "参考图库", shortcut: 6, group: "asset", component: RefsPage },
  {
    key: "assets",
    label: "资产库",
    shortcut: 9,
    group: "asset",
    component: AssetsPage,
  },
  {
    key: "plan",
    label: "发布计划",
    shortcut: 0,
    group: "publish",
    component: PlanPage,
  },
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
