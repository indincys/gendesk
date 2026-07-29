import { clusterTime } from "@/features/_shared/timeline";
import { type TrashHead, clusterFace, clusterSkill } from "@/features/trash/model";
import { assetSrc, bg } from "@/lib/img";
import type { TrashItemView } from "@/lib/ipc";
import { cn, promptLabel } from "@/lib/utils";
import { Check } from "lucide-react";
import { memo } from "react";

/**
 * 废纸篓的三种行：段头 · 网格一行卡片 · 列表一条。
 *
 * 三者都由 `TrashPage` 的虚拟化容器摆位，故这里**不管布局位置**，只管一块之内长什么样。
 * 段头两种布局共用一份 —— 换布局不该把人对时间线的记忆清零。
 *
 * ## 灰掉是有意的
 *
 * 缩略图统一压了饱和度：这一页上的每一张都已经被判过「不要了」，让它们和作品库一样
 * 鲜亮，会让人误以为自己在浏览成果。悬停 / 选中 / 焦点时恢复原色 —— 那时人正要认真看它。
 */
export function TrashHeadRow({
  head,
  sel,
  onPickCluster,
}: {
  head: TrashHead;
  sel: Set<number>;
  onPickCluster: (ids: number[]) => void;
}) {
  if (head.kind === "day") {
    return (
      <div className="trday">
        <span className="d">{head.day.label}</span>
        <span className="ln" />
        <span className="n">{head.day.count} 项</span>
      </div>
    );
  }
  const c = head.cluster;
  const ids = c.items.map((i) => i.id);
  const allIn = ids.every((id) => sel.has(id));
  const skill = clusterSkill(c);
  return (
    <div className="trclu">
      <span className="tm">{clusterTime(c)}</span>
      {c.groupKey != null && <span className="bt">批次 #{c.groupKey}</span>}
      {/* 一批图整体歪掉时，第一个要问的就是「这批词是哪个 skill 写的」。 */}
      {skill && <span className="sk">{skill}</span>}
      <span className="fc">{clusterFace(c)}</span>
      <div className="f1" />
      <button
        type="button"
        className={cn("btn xs gho", allIn && "on")}
        title="把这一簇整个选中 / 取消"
        onClick={() => onPickCluster(ids)}
      >
        {allIn ? "取消本簇" : `选中本簇 · ${ids.length}`}
      </button>
    </div>
  );
}

export interface TileHandlers {
  onPick: (idx: number, mode: "set" | "toggle" | "range") => void;
  onZoom: (idx: number) => void;
}

/** 网格一行：每格的宽与整行的高都由 `packGroups` 算好，这里只照着摆。 */
export function TrashCardsRow({
  items,
  h,
  gap,
  sel,
  focus,
  handlers,
}: {
  items: { it: TrashItemView; idx: number; w: number }[];
  h: number;
  gap: number;
  sel: Set<number>;
  focus: number;
  handlers: TileHandlers;
}) {
  return (
    <div className="trrow" style={{ gap, height: h }}>
      {items.map(({ it, idx, w }) => (
        <TrashTile
          key={it.id}
          it={it}
          idx={idx}
          w={w}
          h={h}
          selected={sel.has(it.id)}
          focused={idx === focus}
          handlers={handlers}
        />
      ))}
    </div>
  );
}

/** 单格。选中/焦点变化只重渲这一格，故回调必须由父级稳定化。 */
const TrashTile = memo(function TrashTile({
  it,
  idx,
  w,
  h,
  selected,
  focused,
  handlers,
}: {
  it: TrashItemView;
  idx: number;
  w: number;
  h: number;
  selected: boolean;
  focused: boolean;
  handlers: TileHandlers;
}) {
  const src = assetSrc(it.thumbPath) ?? assetSrc(it.imagePath);
  return (
    <div
      className={cn("trtile", selected && "sel", focused && "focus")}
      style={{ width: w, height: h }}
      role="button"
      tabIndex={-1}
      title={`${[it.sourceLabel, it.code, it.skill].filter(Boolean).join(" · ")}\n双击放大`}
      onClick={(e) =>
        handlers.onPick(idx, e.shiftKey ? "range" : e.metaKey || e.ctrlKey ? "toggle" : "set")
      }
      onDoubleClick={() => handlers.onZoom(idx)}
      onKeyDown={(e) => e.key === "Enter" && handlers.onZoom(idx)}
    >
      {src ? (
        <img className="trthumb" src={src} alt="" loading="lazy" decoding="async" />
      ) : (
        // 图已经随上一次清理走了，只剩记录。留一块占位而不是塌成 0 高 ——
        // 塌掉的话这一行会缺一格，看上去像少删了什么。
        <div className="trgone">图已清理</div>
      )}
      <span className={cn("trck", selected && "on")}>
        <Check className="ic12" />
      </span>
      {it.code && <span className="trcode">{it.code}</span>}
    </div>
  );
});

/**
 * 列表一条 —— 同一条时间线，换成可**扫读**的一行。
 *
 * 与网格互补而不是重复：网格答「这一批长什么样」（看图），列表答「这一条是什么」
 * （编号、提示词开头、skill、来源、时刻，一眼扫过去）。找一条记得住编号的图时，
 * 在网格里逐格辨认是最慢的路。
 */
export const TrashListRow = memo(function TrashListRow({
  it,
  idx,
  selected,
  focused,
  handlers,
}: {
  it: TrashItemView;
  idx: number;
  selected: boolean;
  focused: boolean;
  handlers: TileHandlers;
}) {
  return (
    <div
      className={cn("trrow2", selected && "sel", focused && "focus")}
      role="button"
      tabIndex={-1}
      onClick={(e) =>
        handlers.onPick(idx, e.shiftKey ? "range" : e.metaKey || e.ctrlKey ? "toggle" : "set")
      }
      onDoubleClick={() => handlers.onZoom(idx)}
      onKeyDown={(e) => e.key === "Enter" && handlers.onZoom(idx)}
    >
      <span className={cn("trck", selected && "on")}>
        <Check className="ic12" />
      </span>
      <span className="ph trmini" style={bg(it.thumbPath)} />
      {it.code && <span className="pid noshrink">{promptLabel(it.code, it.title)}</span>}
      <span className="mono fs11 t3 f1 nowrap ohide">{it.promptText ?? entityLabel(it)}</span>
      {it.skill && <span className="sk noshrink">{it.skill}</span>}
      <span className="bdg b-gray noshrink">{it.sourceLabel}</span>
    </div>
  );
});

/** 没有提示词可显示时，至少说清这条是个什么东西。 */
export function entityLabel(it: TrashItemView): string {
  return (
    {
      task: "验收未通过的结果",
      work: "已删除作品",
      prompt: "已删除提示词",
      ref: "已删除参考图",
      clip: "验收未通过的视频",
    }[it.entityType] ?? it.entityType
  );
}
