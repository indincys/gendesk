import { type PackedBlock, packGroups } from "@/features/_shared/justified";
import {
  type TimeCluster,
  type TimeDay,
  faceOf,
  groupByTimeline,
} from "@/features/_shared/timeline";
import type { TrashItemView } from "@/lib/ipc";

/**
 * 废纸篓的派生逻辑 —— 纯函数，与 React 无关，故可单独测。
 *
 * 分段规则本身在 `_shared/timeline.ts`（与验收页共用）。这里只负责三件废纸篓自己的事：
 * 把它接到 `deletedAt` 与批次上、给每张图一个宽高比、把每一簇**各自**打成齐行。
 *
 * ## 为什么用删除时刻而不是生成时刻
 *
 * 库里对五类实体唯一共有的时间戳就是 `deleted_at`。而且它恰好是对的那个：
 * 未通过的图是**验收那一刻**进来的，也就是人真正在看这批图的时刻。
 */

export type TrashCluster = TimeCluster<TrashItemView>;
export type TrashDay = TimeDay<TrashItemView>;

/** 按删除时刻倒序（新的在上）切成日 → 任务簇。 */
export function groupTrash(rows: TrashItemView[], now: number): TrashDay[] {
  return groupByTimeline(rows, {
    at: (r) => r.deletedAt,
    clusterKey: (r) => r.batchId,
    tieBreak: (r) => r.id,
    now,
  });
}

/** 一簇里都是些什么（「验收未通过 12 · 已删除提示词 2」）。 */
export function clusterFace(c: TrashCluster): string {
  return faceOf(c.items, (i) => i.sourceLabel);
}

/**
 * 这一簇是哪个 skill 投的单。
 *
 * 一簇通常出自一次投单，故绝大多数时候只有一个答案；混了就报「多个」而不是
 * 挑第一个 —— 挑第一个会让人把整簇的锅算到一个 skill 头上。
 * 一条都没有（手动导入的提示词、旧数据）返回 null，标就不出现。
 */
export function clusterSkill(c: TrashCluster): string | null {
  const set = new Set(
    c.items.map((i) => i.skill).filter((s): s is string => s != null && s !== ""),
  );
  if (set.size === 0) return null;
  return set.size === 1 ? ([...set][0] ?? null) : `${set.size} 个 skill`;
}

/** 一张图的宽高比。缺尺寸时用 3:4（本仓库最常见的竖幅），绝不为排版猜一个假尺寸。 */
export function ratioOf(it: TrashItemView): number {
  const w = it.width ?? 0;
  const h = it.height ?? 0;
  return w > 0 && h > 0 ? w / h : 3 / 4;
}

/** 标题行的两种内容：日期头 / 任务簇头。 */
export type TrashHead = { kind: "day"; day: TrashDay } | { kind: "cluster"; cluster: TrashCluster };

/** 渲染块：标题行或一行卡片。方向键靠 `items` 认卡片行（见 `moveByRow`）。 */
export type TrashBlock = PackedBlock<TrashItemView, TrashHead>;

export interface BuildOptions {
  /** 容器可用宽度（已扣掉内边距）。 */
  width: number;
  /** 「每行大约几张」的目标值（大小滑块）。 */
  perRow: number;
  gap: number;
}

/**
 * 块高的四个常量。**它们与 `globals.css` 里的固定高度是一对，改一处必须改另一处**。
 *
 * 虚拟化按这几个数摆位，而这一页**不回头测量**（测量既是滚动时的强制同步布局，
 * 也会在测出来之前让相邻两块叠在一起）。所以 `.trblk` 的每一种都是定高的。
 */
export const TRASH_GAP = 12;
/** 日期头整块。 */
export const TRASH_DAY_H = 48;
/** 任务簇头整块。 */
export const TRASH_CLUSTER_H = 44;
/** 列表模式一行。 */
export const TRASH_LIST_ROW_H = 44;

/** 一块占多高 —— 虚拟化的 estimateSize 就是它，不是估值而是真值。 */
export function blockHeight(b: TrashBlock, mode: "grid" | "list"): number {
  if (b.kind === "head") {
    return b.head.kind === "day" ? TRASH_DAY_H : TRASH_CLUSTER_H;
  }
  return mode === "list" ? TRASH_LIST_ROW_H : b.h + TRASH_GAP;
}

/**
 * 列表模式的块：同一条时间线、同一套段头，只是一条一行。
 *
 * 与网格共用 `TrashBlock` 类型（每行 `items` 恰好一条）不是凑合 —— 这样键盘导航、
 * 选区、光标滚动三处对两种布局是**同一份代码**，换布局时光标停在原处。
 */
export function buildListBlocks(days: TrashDay[]): {
  blocks: TrashBlock[];
  cardRow: number[];
  flat: TrashItemView[];
} {
  const blocks: TrashBlock[] = [];
  const cardRow: number[] = [];
  const flat: TrashItemView[] = [];
  for (const day of days) {
    day.clusters.forEach((cluster, i) => {
      if (i === 0) blocks.push({ kind: "head", key: `${day.key}h0`, head: { kind: "day", day } });
      blocks.push({ kind: "head", key: `${cluster.key}hc`, head: { kind: "cluster", cluster } });
      for (const it of cluster.items) {
        const idx = flat.length;
        flat.push(it);
        cardRow[idx] = blocks.length;
        blocks.push({
          kind: "cards",
          key: `${cluster.key}i${it.id}`,
          items: [{ it, idx, w: 0 }],
          h: TRASH_LIST_ROW_H,
        });
      }
    });
  }
  return { blocks, cardRow, flat };
}

/**
 * 分段 + 齐行打包。每一天的第一簇多顶一个日期头，其余只顶簇头。
 *
 * 逐簇分别打包（`packGroups` 的语义）：一行绝不横跨两个任务簇，段与段之间才有
 * 看得见的断口。
 */
export function buildBlocks(
  days: TrashDay[],
  opts: BuildOptions,
): { blocks: TrashBlock[]; cardRow: number[]; flat: TrashItemView[] } {
  return packGroups(
    days.flatMap((day) =>
      day.clusters.map((cluster, i) => ({
        key: cluster.key,
        heads: [
          ...(i === 0 ? [{ kind: "day" as const, day }] : []),
          { kind: "cluster" as const, cluster },
        ],
        items: cluster.items,
      })),
    ),
    { width: opts.width, perRow: opts.perRow, gap: opts.gap, ratioOf },
  );
}
