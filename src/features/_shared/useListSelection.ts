import { useCallback, useMemo, useState } from "react";

type RowEvent = React.MouseEvent | React.KeyboardEvent;

/**
 * 四个资料库共用的桌面式列表选择模型。
 *
 * - 单击：单选；Shift：连续范围；Cmd/Ctrl：切换
 * - Cmd/Ctrl+A：全选；方向键：移动焦点；Enter：打开；Delete：批量删除；Esc：清空
 */
export function useListSelection<T extends string | number>(
  orderedIds: readonly T[],
  actions: { onOpen?: (id: T) => void; onDelete?: (ids: T[]) => void } = {},
) {
  const [selected, setSelected] = useState<Set<T>>(() => new Set());
  const [anchor, setAnchor] = useState<T | null>(null);
  const activeId = useMemo(
    () => orderedIds.find((id) => selected.has(id)) ?? null,
    [orderedIds, selected],
  );

  const select = useCallback(
    (id: T, event?: Pick<RowEvent, "shiftKey" | "metaKey" | "ctrlKey">) => {
      setSelected((current) => {
        if (event?.shiftKey && anchor !== null) {
          const a = orderedIds.indexOf(anchor);
          const b = orderedIds.indexOf(id);
          if (a >= 0 && b >= 0) {
            const next = event.metaKey || event.ctrlKey ? new Set(current) : new Set<T>();
            for (const item of orderedIds.slice(Math.min(a, b), Math.max(a, b) + 1)) next.add(item);
            return next;
          }
        }
        if (event?.metaKey || event?.ctrlKey) {
          const next = new Set(current);
          if (next.has(id)) next.delete(id);
          else next.add(id);
          return next;
        }
        return new Set([id]);
      });
      setAnchor(id);
    },
    [anchor, orderedIds],
  );

  const onKeyDown = useCallback(
    (event: React.KeyboardEvent) => {
      const target = event.target as HTMLElement;
      if (target.matches("input, textarea, select, [contenteditable='true']")) return;
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "a") {
        event.preventDefault();
        setSelected(new Set(orderedIds));
        return;
      }
      if (event.key === "Escape") {
        setSelected(new Set());
        setAnchor(null);
        return;
      }
      const ids = [...selected].filter((id) => orderedIds.includes(id));
      if ((event.key === "Delete" || event.key === "Backspace") && ids.length > 0) {
        event.preventDefault();
        actions.onDelete?.(ids);
        return;
      }
      if (event.key === "Enter" && activeId !== null) {
        event.preventDefault();
        actions.onOpen?.(activeId);
        return;
      }
      if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
      event.preventDefault();
      if (orderedIds.length === 0) return;
      const current = anchor === null ? -1 : orderedIds.indexOf(anchor);
      const delta = event.key === "ArrowDown" ? 1 : -1;
      const nextIndex = Math.max(0, Math.min(orderedIds.length - 1, current + delta));
      const next = orderedIds[nextIndex];
      if (next !== undefined) select(next, event);
    },
    [actions, activeId, anchor, orderedIds, select, selected],
  );

  return {
    selected,
    activeId,
    count: selected.size,
    isSelected: (id: T) => selected.has(id),
    select,
    clear: () => {
      setSelected(new Set());
      setAnchor(null);
    },
    replace: (ids: Iterable<T>) => setSelected(new Set(ids)),
    containerProps: { tabIndex: 0, onKeyDown },
  };
}
