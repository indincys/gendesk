import { AssetsPage } from "@/features/assets/AssetsPage";
import { GeneratePage } from "@/features/generate/GeneratePage";
import { PlanPage } from "@/features/plan/PlanPage";
import { PromptsPage } from "@/features/prompts/PromptsPage";
import { RefsPage } from "@/features/refs/RefsPage";
import { ReviewPage } from "@/features/review/ReviewPage";
import { SettingsPage } from "@/features/settings/SettingsPage";
import { TasksPage } from "@/features/tasks/TasksPage";
import { TrashPage } from "@/features/trash/TrashPage";
import { WorksPage } from "@/features/works/WorksPage";
import {
  AlignLeft,
  CheckCircle2,
  Grid2x2,
  Image,
  Layers,
  ListChecks,
  Send,
  Settings2,
  Sparkles,
  Trash2,
} from "lucide-react";
import type { ComponentType } from "react";

/** 页面路由键 —— 顺序即快捷键序（⌘1–8 制作/资产，⌘9 资产库，⌘0 发布计划）。 */
export type RouteKey =
  | "generate"
  | "tasks"
  | "review"
  | "library"
  | "prompts"
  | "refs"
  | "assets"
  | "plan"
  | "trash"
  | "settings";

export type NavGroup = "make" | "asset" | "publish" | "system";

export interface RouteDef {
  key: RouteKey;
  label: string;
  /** ⌘/Ctrl+N 数字（1–8） */
  shortcut: number;
  group: NavGroup;
  icon: ComponentType<{ className?: string }>;
  component: ComponentType;
}

/** 路由注册表 —— 命令面板、侧栏、快捷键均从此单一来源派生。 */
export const ROUTES: readonly RouteDef[] = [
  {
    key: "generate",
    label: "图片生成",
    shortcut: 1,
    group: "make",
    icon: Sparkles,
    component: GeneratePage,
  },
  {
    key: "tasks",
    label: "任务队列",
    shortcut: 2,
    group: "make",
    icon: ListChecks,
    component: TasksPage,
  },
  {
    key: "review",
    label: "图片验收",
    shortcut: 3,
    group: "make",
    icon: CheckCircle2,
    component: ReviewPage,
  },
  {
    key: "library",
    label: "作品库",
    shortcut: 4,
    group: "asset",
    icon: Grid2x2,
    component: WorksPage,
  },
  {
    key: "prompts",
    label: "提示词库",
    shortcut: 5,
    group: "asset",
    icon: AlignLeft,
    component: PromptsPage,
  },
  { key: "refs", label: "参考图库", shortcut: 6, group: "asset", icon: Image, component: RefsPage },
  {
    key: "assets",
    label: "资产库",
    shortcut: 9,
    group: "asset",
    icon: Layers,
    component: AssetsPage,
  },
  {
    key: "plan",
    label: "发布计划",
    shortcut: 0,
    group: "publish",
    icon: Send,
    component: PlanPage,
  },
  {
    key: "trash",
    label: "废纸篓",
    shortcut: 7,
    group: "system",
    icon: Trash2,
    component: TrashPage,
  },
  {
    key: "settings",
    label: "设置",
    shortcut: 8,
    group: "system",
    icon: Settings2,
    component: SettingsPage,
  },
] as const;

export const ROUTE_BY_KEY: Record<RouteKey, RouteDef> = Object.fromEntries(
  ROUTES.map((r) => [r.key, r]),
) as Record<RouteKey, RouteDef>;

/** ⌘1–8 数字 → 路由键（快捷键分发用） */
export const ROUTE_BY_SHORTCUT: Record<number, RouteKey> = Object.fromEntries(
  ROUTES.map((r) => [r.shortcut, r.key]),
);
