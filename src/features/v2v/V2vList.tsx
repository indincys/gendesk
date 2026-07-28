import { type Channel, type Filter, type FilterFace, type Row, fmtDur } from "@/features/v2v/model";
import { cn } from "@/lib/utils";
import type { CSSProperties, MouseEvent } from "react";

/** 点一行时按住了什么键 —— 决定这一下是移光标、加选一条，还是选一整段。 */
export type PickMode = "set" | "toggle" | "range";

export function pickMode(e: { shiftKey: boolean; metaKey: boolean; ctrlKey: boolean }): PickMode {
  if (e.shiftKey) return "range";
  if (e.metaKey || e.ctrlKey) return "toggle";
  return "set";
}

/**
 * 工作台右栏 —— 这一屏有哪些条目。
 *
 * ## 顶上那排通道片
 *
 * 通道从侧栏搬到了这里（v0.24.0 修订）。它筛的就是下面这张表，所以它该长在表上面 ——
 * 在侧栏时它和流程六档摞成一列九行，而那九行里任何两行的关系都得靠人自己回想。
 *
 * 只留**用得最多的三条**（`topChannels`，与顶栏那排状态灯同一份）：格数固定，
 * 位置就稳得住；要全部通道去顶栏那排灯的悬停说明，或者干脆按流程看。
 * 再点一次已选中的那一枚 = 回到上一次的流程档（`toggleChannel`），
 * 所以不需要一枚答不出「全部什么」的「全部」按钮。
 *
 * ## 一行只有四样东西
 *
 * 左轨（通道色，与快捷片、堆叠条同源）· 勾选 · 编号 · 一句话 + 行尾数。
 * 型号那一列去掉了 —— 它现在是左轨的颜色；缩略图也去掉了 ——
 * 左边那一整栏就是它，而且是能动的。
 *
 * ## 排序没有开关
 *
 * 顺序恒是「在跑的最前，然后等得最久的」（`rankRows`）。从前栏头右边有个三档循环的
 * 排序按钮，三档里只有一档真被用过，而它最擅长的事是被误点之后让人看着一个自己
 * 没意识到的顺序。
 */
export function V2vList({
  rows,
  channels,
  top,
  filter,
  face,
  curId,
  sel,
  onChannel,
  onPick,
  onCheck,
  onToggleAll,
}: {
  rows: Row[];
  channels: Channel[];
  /** 快捷筛选的前三条通道。空数组 = 一条通道都没有，那排片子整个不出现。 */
  top: Channel[];
  filter: Filter;
  face: FilterFace;
  curId: number | null;
  sel: Set<number>;
  onChannel: (key: string) => void;
  onPick: (id: number, mode: PickMode) => void;
  onCheck: (id: number) => void;
  onToggleAll: () => void;
}) {
  const toneOf = new Map(channels.map((c) => [c.key, c.tone]));
  const allIn = rows.length > 0 && rows.every((r) => sel.has(r.clip.id));

  return (
    <div className="vlist">
      <div
        className="vlisthd"
        {...(face.tone == null ? {} : { "data-tone": face.tone })}
        style={{ "--tone": face.color === "" ? undefined : face.color } as CSSProperties}
      >
        <span className="dot" />
        <span className="fs13 fw6 nowrap">{face.label}</span>
        <span className="n">{rows.length} 条</span>
        <div className="f1" />
      </div>

      {top.length > 0 && (
        <div className="chq">
          {top.map((c) => {
            const on = filter.kind === "channel" && filter.key === c.key;
            return (
              <button
                key={c.key || "(default)"}
                type="button"
                className={cn("chqi", on && "on", c.live === 0 && !on && "zero")}
                data-tone={c.tone}
                title={[
                  c.key === "" ? "设置里没写默认型号，走 CLI 默认" : c.key,
                  c.note,
                  c.headline,
                  c.title,
                  on ? "\n再点一次回到刚才那一档" : "\n只看这条通道上还没走完的条目",
                ]
                  .filter((s) => s !== "")
                  .join("\n")}
                onClick={() => onChannel(c.key)}
              >
                <i />
                <span className="nm">{c.label}</span>
                {/* 数的是**还没走完的**，不是这条通道历史上的全部 —— 点进去看到的就是这些。 */}
                <span className="n">{c.live}</span>
              </button>
            );
          })}
        </div>
      )}

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
        <span className="fs11 t3">{sel.size > 0 ? `已选 ${sel.size}` : "全选 ⌘A"}</span>
        <div className="f1" />
        {/* 底坞的「这一屏」按钮作用在**勾选或整屏**上，所以这里必须说清现在是哪一种。 */}
        <span className="fs10 t3 nowrap">
          {sel.size > 0 ? "底部动作只作用于勾选的" : "⇧ 选一段 · ⌘ 点加选"}
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
            onPick={(mode) => onPick(r.clip.id, mode)}
            onCheck={() => onCheck(r.clip.id)}
          />
        ))}
        {/* 空屏必须说清是**哪一个**筛选空了 —— 一次只有一个条件，
            不必让人回想是哪两个条件叠出来的。 */}
        {rows.length === 0 && (
          <div className="vlistempty">
            {filter.kind === "action"
              ? "这一档现在没有条目 —— 左边换一档。"
              : "这条通道上已经没有在制的条目了 —— 再点一次那枚通道片回到刚才那一档。"}
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
  onPick: (mode: PickMode) => void;
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

  // 「缺词」这一档里每一行的 `situation` 都是同一句「等你跑 v2v-rewrite · 工单已就绪」
  // —— 那句话在这一屏上已经由摘要卡说过，逐行再抄 40 遍不带任何一行独有的信息。
  // 换成组名：派工单时真正要认的就是「这是哪一组的词」。
  const sub = r.action === "rewrite" ? c.groupName || "未分组" : r.situation;

  return (
    <div
      className={cn("vlrow", cur && "cur", checked && "sel")}
      onClick={(e: MouseEvent) => onPick(pickMode(e))}
      onKeyDown={(e) => e.key === "Enter" && onPick("set")}
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
        <span
          className={cn("sub", r.action === "rewrite" ? "t3" : toneClass(r.situationTone))}
          title={r.situation}
        >
          {sub}
        </span>
      </div>
      {tail != null && <span className="tail">{tail}</span>}
    </div>
  );
}

function toneClass(t: Row["situationTone"]): string {
  return t === "er" ? "terr" : t === "wr" ? "wr2" : t === "acc" ? "acc2" : "t3";
}
