import { commands, unwrap } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { ROUTES, type RouteKey } from "@/routes";
import { useEngineStore } from "@/stores/engine";
import { modKeyLabel, useUiStore } from "@/stores/ui";
import { Search } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { toast } from "sonner";

interface PaletteItem {
  cat: string;
  label: string;
  shortcut?: string;
  run: () => void;
}

/** ⌘/Ctrl+K 命令面板（执行计划 0.5）。跳转 8 页 + 占位操作。 */
export function CommandPalette() {
  const open = useUiStore((s) => s.paletteOpen);
  const query = useUiStore((s) => s.paletteQuery);
  const setQuery = useUiStore((s) => s.setPaletteQuery);
  const closePalette = useUiStore((s) => s.closePalette);
  const go = useUiStore((s) => s.go);
  const platform = useUiStore((s) => s.platform);
  const mod = modKeyLabel(platform);

  const inputRef = useRef<HTMLInputElement>(null);
  const [active, setActive] = useState(0);

  const items = useMemo<PaletteItem[]>(() => {
    const jump: PaletteItem[] = ROUTES.map((r) => ({
      cat: "跳转",
      label: r.label,
      ...(r.shortcut === null ? {} : { shortcut: `${mod}${r.shortcut}` }),
      run: () => go(r.key as RouteKey),
    }));
    const paused = useEngineStore.getState().paused;
    const actions: PaletteItem[] = [
      { cat: "操作", label: "导入提示词 .txt", run: () => go("prompts") },
      {
        cat: "操作",
        label: paused ? "继续队列" : "暂停队列",
        run: () => {
          void unwrap(paused ? commands.resumeQueue() : commands.pauseQueue())
            .then(() => useEngineStore.getState().setPaused(!paused))
            .catch(() => {});
          toast(paused ? "已继续队列" : "已暂停队列");
        },
      },
      {
        cat: "操作",
        label: "重扫收件箱",
        run: () => {
          void unwrap(commands.rescanInbox())
            .then((r) => toast(`收件箱扫描完成：入库 ${r.ingested} · 待认领 ${r.unclaimed}`))
            .catch((e) => toast.error(String(e)));
        },
      },
      {
        cat: "操作",
        label: "生成明日任务单",
        run: () => {
          const d = new Date();
          d.setDate(d.getDate() + 1);
          const date = `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
          void unwrap(commands.generateSheet(date))
            .then(() => {
              toast.success(`已生成 ${date} 任务单草稿`);
              go("plan");
            })
            .catch((e) => toast.error(String(e)));
        },
      },
      { cat: "操作", label: "打开今日看板", run: () => go("plan") },
      {
        cat: "操作",
        label: "导入今日回执",
        run: () => {
          const d = new Date();
          const date = `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
          void unwrap(commands.getDashboard(date))
            .then(async (dash) => {
              if (dash.sheetId == null) {
                toast("今日没有任务单");
                return;
              }
              const r = await unwrap(commands.importReceipts(dash.sheetId));
              toast.success(`对账完成：已发布 ${r.published} · 失败 ${r.failed}`);
              go("plan");
            })
            .catch((e) => toast.error(String(e)));
        },
      },
      { cat: "操作", label: "检查更新", run: () => toast("应用内更新将在发布链接入（M4）") },
    ];
    const q = query.trim().toLowerCase();
    return [...jump, ...actions].filter((c) => !q || c.label.toLowerCase().includes(q));
  }, [query, mod, go]);

  // 打开时聚焦输入并复位选中项。
  useEffect(() => {
    if (open) {
      setActive(0);
      requestAnimationFrame(() => inputRef.current?.focus());
    }
  }, [open]);

  if (!open) return null;

  const onQueryChange = (value: string) => {
    setQuery(value);
    setActive(0); // 输入变化即复位选中项
  };

  const runItem = (item: PaletteItem | undefined) => {
    if (!item) return;
    closePalette();
    item.run();
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setActive((i) => Math.min(i + 1, items.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActive((i) => Math.max(i - 1, 0));
    } else if (e.key === "Enter") {
      e.preventDefault();
      runItem(items[active]);
    }
  };

  return (
    <div className="ovl ovtop" onClick={closePalette}>
      <div className="plt" onClick={(e) => e.stopPropagation()}>
        <div className="plin">
          <Search className="ic" />
          <input
            ref={inputRef}
            className="plinp"
            placeholder="跳转页面或执行操作…"
            value={query}
            onChange={(e) => onQueryChange(e.target.value)}
            onKeyDown={onKeyDown}
          />
          <span className="kbd">esc</span>
        </div>
        <div className="pllist">
          {items.map((item, i) => (
            <div
              key={`${item.cat}-${item.label}`}
              className={cn("pltit", i === active && "on")}
              onClick={() => runItem(item)}
              onMouseEnter={() => setActive(i)}
              onKeyDown={(e) => e.key === "Enter" && runItem(item)}
              role="button"
              tabIndex={-1}
            >
              <span className="plcat">{item.cat}</span>
              <span className="f1 fw5">{item.label}</span>
              {item.shortcut && <span className="kbd">{item.shortcut}</span>}
            </div>
          ))}
          {items.length === 0 && (
            <div className="pltit">
              <span className="plcat" />
              <span className="f1 t3">无匹配项</span>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
