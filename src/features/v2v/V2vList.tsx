import {
  ACTION_META,
  type Channel,
  type NextAction,
  type Row,
  SORTS,
  type SortKey,
  fmtDur,
} from "@/features/v2v/model";
import { cn } from "@/lib/utils";

/**
 * 工作台右栏 —— 这一屏有哪些条目。
 *
 * ## 为什么不再按通道分节
 *
 * 通道成了侧栏的筛选卡（`V2vNavCards`），所以这里是一条平铺的流。分节时代那些节内
 * 动作（全选本节 / 整节改投 / 看片流 N）没有消失，它们变成了底坞里的「这一档」——
 * 因为「这一档」现在就等于「这一档 × 这条通道」，与一节是同一个集合。
 *
 * ## 一行只有四样东西
 *
 * 左轨（通道色，与侧栏、堆叠条同源）· 勾选 · 编号 · 一句话 + 行尾数。
 * 型号那一列去掉了 —— 它现在是左轨的颜色和侧栏里选中的那一行；缩略图也去掉了 ——
 * 左边那一整栏就是它，而且是能动的。
 */
export function V2vList({
  rows,
  channels,
  action,
  curId,
  sel,
  sort,
  onSort,
  onPick,
  onCheck,
  onToggleAll,
}: {
  rows: Row[];
  channels: Channel[];
  action: NextAction;
  curId: number | null;
  sel: Set<number>;
  sort: SortKey;
  onSort: () => void;
  onPick: (id: number) => void;
  onCheck: (id: number) => void;
  onToggleAll: () => void;
}) {
  const meta = ACTION_META[action];
  const toneOf = new Map(channels.map((c) => [c.key, c.tone]));
  const allIn = rows.length > 0 && rows.every((r) => sel.has(r.clip.id));

  return (
    <div className="vlist">
      <div className="vlisthd">
        <span className="dot" style={{ background: meta.dot }} />
        <span className="fs13 fw6 nowrap">{meta.label}</span>
        <span className="n">{rows.length} 条</span>
        <div className="f1" />
        <button type="button" className="sortbtn" onClick={onSort} title="点击换一种排序">
          {SORTS[sort]} ▾
        </button>
      </div>

      {/* 整行都是点击目标（含「全选」那两个字）—— 一个 13px 见方的勾选框
          在一行 46 条的节奏里太难瞄准了。 */}
      <div
        className="vlistall"
        onClick={onToggleAll}
        onKeyDown={(e) => (e.key === " " || e.key === "Enter") && onToggleAll()}
        role="checkbox"
        aria-checked={allIn}
        tabIndex={0}
      >
        <span className={cn("vbox", allIn && "on")}>{allIn ? "✓" : ""}</span>
        <span className="fs11 t3">{sel.size > 0 ? `已选 ${sel.size}` : "全选"}</span>
        <div className="f1" />
        {/* 底坞的「这一档」按钮作用在**勾选或整屏**上，所以这里必须说清现在是哪一种。 */}
        <span className="fs10 t3 nowrap">
          {sel.size > 0 ? "底部动作只作用于勾选的" : "底部动作作用于这一屏"}
        </span>
      </div>

      <div className="vlistbody">
        {rows.map((r) => (
          <ClipRow
            key={r.clip.id}
            r={r}
            tone={toneOf.get(r.modelFull ?? "") ?? 0}
            cur={r.clip.id === curId}
            checked={sel.has(r.clip.id)}
            onPick={() => onPick(r.clip.id)}
            onCheck={() => onCheck(r.clip.id)}
          />
        ))}
        {rows.length === 0 && (
          <div className="vlistempty">
            这一档在这条通道上没有条目。
            <br />
            换一档，或把通道切回「全部通道」。
          </div>
        )}
      </div>
    </div>
  );
}

function ClipRow({
  r,
  tone,
  cur,
  checked,
  onPick,
  onCheck,
}: {
  r: Row;
  tone: number;
  cur: boolean;
  checked: boolean;
  onPick: () => void;
  onCheck: () => void;
}) {
  const c = r.clip;
  // 行尾那个数：还没花钱的两档报**要花多少**（那是放行前唯一要权衡的量），
  // 其余报**已经等了多久**（那是排队几小时里唯一在动的量）。
  const tail =
    r.action === "submit" || r.action === "rewrite"
      ? (r.estimate ?? r.credit ?? null)
      : r.waitSecs > 0
        ? fmtDur(r.waitSecs)
        : null;

  return (
    <div
      className={cn("vlrow", cur && "cur", checked && "sel")}
      onClick={onPick}
      onKeyDown={(e) => e.key === "Enter" && onPick()}
      role="button"
      tabIndex={-1}
    >
      <span className="rail" data-tone={tone} />
      <span
        onClick={(e) => {
          e.stopPropagation();
          onCheck();
        }}
        onKeyDown={(e) => {
          if (e.key === " ") {
            e.stopPropagation();
            onCheck();
          }
        }}
        role="checkbox"
        aria-checked={checked}
        tabIndex={-1}
      >
        <span className={cn("vbox", checked && "on")}>{checked ? "✓" : ""}</span>
      </span>
      <div className="bd">
        <span className="code">
          {c.promptCode}
          {/* 重跑过的标出来：同一张图已经花过不止一份额度。 */}
          {c.attempt > 1 && <span className="wr2"> ·{c.attempt}</span>}
        </span>
        <span className={cn("sub", toneClass(r.situationTone))} title={r.situation}>
          {r.situation}
        </span>
      </div>
      {tail != null && <span className="tail">{tail}</span>}
    </div>
  );
}

function toneClass(t: Row["situationTone"]): string {
  return t === "er" ? "terr" : t === "wr" ? "wr2" : t === "acc" ? "acc2" : "t3";
}
