import { Tooltip } from "@/components/ui/Tooltip";
import { ACTION_META, WORKBENCH_ACTIONS } from "@/features/v2v/model";
import { cn } from "@/lib/utils";
import { selectActionCounts, useV2vStore } from "@/stores/v2v";
import type { CSSProperties } from "react";
import { useShallow } from "zustand/react/shallow";

/**
 * 侧栏里嵌在「视频流水线」下面的流程列（v0.24.0）。
 *
 * ## 它只有一维：**流程**
 *
 * 六行 = 一条视频依次经过的六个位置（异常 · 缺词 · 就绪 · 远端 · 队列 · 验收），
 * 顺序就是流转顺序，读下来就是这条流水线本身。
 *
 * 通道曾经也在这里（下半截九行里的后三行）。搬走了 —— 侧栏是**导航**，
 * 而通道不是流程上的一站，它是「这一条排在哪条队上」，与「它走到哪儿了」正交。
 * 两维摞在同一列里，就算中间画了分隔线，读到的仍然是「九个可以点的东西」；
 * 而它们互斥（`Filter` 一次只有一个），于是那九行里任何两行的关系都得靠人自己回想。
 * 通道现在在**任务列表顶上**那排快捷片里（`V2vList`）—— 它筛的就是下面那张表，
 * 挨着被筛的东西，关系不必解释。
 *
 * ## 这里一个字的说明文字都不留
 *
 * 侧栏是全应用最挤的一列（246px 里还压着十一条路由）。每档那句「拿它怎么办」
 * 还在，只是搬进了 `title=` —— 它是**要用时才读**的东西，常驻摆着只会把六行
 * 流程撑成一屏。
 */
export function V2vNavCards() {
  const filter = useV2vStore((s) => s.filter);
  const setAction = useV2vStore((s) => s.setAction);
  const counts = useV2vStore(useShallow(selectActionCounts));

  return (
    <div className="navfl">
      {WORKBENCH_ACTIONS.map((a) => {
        const m = ACTION_META[a];
        const on = filter.action === a;
        const n = counts[a] ?? 0;
        return (
          <Tooltip key={a} content={m.note}>
            <button
              type="button"
              className={cn("navrow", on && "on", n === 0 && !on && "zero")}
              // 色直接来自 `ACTION_META.dot`（与行内色点、摘要卡同源），经内联 style
              // 落到 `--tone` 上 —— 胶囊那一套规则只认这个变量。
              style={{ "--tone": m.dot } as CSSProperties}
              onClick={() => setAction(a)}
            >
              <span className="fpill">{m.label}</span>
              <span className="f1" />
              <span className="n">{n}</span>
            </button>
          </Tooltip>
        );
      })}
    </div>
  );
}
