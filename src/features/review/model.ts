import type { PackGroup } from "@/features/_shared/justified";
import { clusterTime, dayLabel, groupByTimeline } from "@/features/_shared/timeline";
import type { ReviewItemView } from "@/lib/ipc";

export type ReviewMode = "requirement" | "batch" | "time" | "ref" | "group" | "key";

export type ReviewHead =
  | { kind: "day"; label: string; count: number }
  | { kind: "cluster"; name: string; detail: string; ids: number[] };

type ReviewSection = PackGroup<ReviewItemView, ReviewHead>;
const nameOrder = new Intl.Collator("zh-CN", { numeric: true });

// CLI 工单标识和参考图内容哈希仍可用于追溯，界面标题只读可辨认的名字。
export function readableName(value: string | null): string {
  const name = value?.trim() ?? "";
  return /^(?:cli-[a-f0-9]{32}|[a-f0-9]{40,64}(?:\.[a-z0-9]+)?)$/i.test(name) ? "" : name;
}

export function refLabel(item: ReviewItemView): string {
  return readableName(item.refName) || `参考图 #${item.refId}`;
}

export function itemLabel(item: ReviewItemView): string {
  return (
    readableName(item.itemName) || readableName(item.refName) || item.promptCode || "未命名图片"
  );
}

function organizationOf(item: ReviewItemView | undefined) {
  const org = item?.organization;
  return org && (org.product || org.style || org.requirement) ? org : null;
}

function requirementKey(item: ReviewItemView): string {
  const org = organizationOf(item);
  if (org) return JSON.stringify([org.product, org.requirement, org.style]);
  return readableName(item.groupName) && item.groupId != null
    ? `group:${item.groupId}`
    : `batch:${item.batchId}`;
}

function groupLabel(item: ReviewItemView): string {
  return readableName(item.groupName) || `提交 #${item.batchId} 的提示词`;
}

function submissionLabel(items: readonly ReviewItemView[]): string {
  const note = readableName(items[0]?.batchNote ?? null);
  if (note) return note;
  const labels = new Set(
    items
      .map((item) =>
        [item.organization?.product, item.organization?.requirement].filter(Boolean).join(" · "),
      )
      .filter(Boolean),
  );
  if (labels.size === 1) return [...labels].join("");
  if (labels.size > 1) return `含 ${labels.size} 项内容的提交`;
  const groups = new Set(items.map((item) => readableName(item.groupName)).filter(Boolean));
  return groups.size === 1 ? [...groups].join("") : "未命名提交";
}

function sectionHead(
  items: readonly ReviewItemView[],
  mode: ReviewMode,
  now: number,
): Extract<ReviewHead, { kind: "cluster" }> {
  const first = items[0];
  const ids = items.map((item) => item.id);
  if (!first) return { kind: "cluster", name: "", detail: "", ids };
  if (mode === "requirement") {
    const { product, style, requirement } = first.organization ?? {};
    const name = [requirement || product, style].filter(Boolean).join(" · ");
    const submissions = new Set(items.map((item) => item.batchId)).size;
    return {
      kind: "cluster",
      name: name || submissionLabel(items),
      detail: [requirement && product, `${submissions} 次提交`].filter(Boolean).join(" · "),
      ids,
    };
  }
  const times = items.map((item) => item.createdAt);
  const from = Math.min(...times);
  const to = Math.max(...times);
  const time = `${dayLabel(to, now)} ${clusterTime({ from, to })}`;
  const name =
    mode === "ref"
      ? refLabel(first)
      : mode === "group"
        ? groupLabel(first)
        : mode === "key"
          ? (first.keyAlias ?? "未标注 Key")
          : submissionLabel(items);
  let detail = time;
  if (mode === "batch" || mode === "time") {
    const styles = [...new Set(items.map((item) => item.organization?.style).filter(Boolean))];
    const names = styles.slice(0, 3).join("、");
    const contents = styles.length > 3 ? `${names}等 ${styles.length} 款` : names;
    detail = [`提交 #${first.batchId}`, time, contents].filter(Boolean).join(" · ");
  }
  return { kind: "cluster", name, detail, ids };
}

export function buildSections(
  items: readonly ReviewItemView[],
  mode: ReviewMode,
  now: number,
): ReviewSection[] {
  if (mode === "time") {
    return groupByTimeline(items, {
      at: (item) => item.createdAt,
      clusterKey: (item) => item.batchId,
      tieBreak: (item) => item.id,
      now,
    }).flatMap((day) =>
      day.clusters.map((cluster, index) => ({
        key: cluster.key,
        heads: [
          ...(index === 0 ? [{ kind: "day" as const, label: day.label, count: day.count }] : []),
          sectionHead(cluster.items, mode, now),
        ],
        items: cluster.items,
      })),
    );
  }
  const keyOf = {
    requirement: requirementKey,
    batch: (item: ReviewItemView) => String(item.batchId),
    ref: (item: ReviewItemView) => String(item.refId),
    group: (item: ReviewItemView) => String(item.groupId ?? `batch:${item.batchId}`),
    key: (item: ReviewItemView) => JSON.stringify(item.keyAlias),
  }[mode];
  const groups = new Map<string, ReviewItemView[]>();
  for (const item of [...items].sort((a, b) => b.batchId - a.batchId || a.id - b.id)) {
    const key = keyOf(item);
    const group = groups.get(key);
    if (group) group.push(item);
    else groups.set(key, [item]);
  }
  const sections = Array.from(groups, ([key, group]) => ({
    key: `${mode}:${key}`,
    heads: [sectionHead(group, mode, now)],
    items: group,
  }));
  if (mode !== "requirement") return sections;

  // 内容分组的顺序跟着归属走，处理完最近一次提交后整组不会跳位。
  return sections.sort((left, right) => {
    const a = organizationOf(left.items[0]);
    const b = organizationOf(right.items[0]);
    if (!a) return b ? 1 : 0;
    if (!b) return -1;
    return (
      nameOrder.compare(a.product, b.product) ||
      nameOrder.compare(a.requirement, b.requirement) ||
      nameOrder.compare(a.style, b.style)
    );
  });
}
