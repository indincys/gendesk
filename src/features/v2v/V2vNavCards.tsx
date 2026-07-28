import { ACTION_META, WORKBENCH_ACTIONS } from "@/features/v2v/model";
import { cn } from "@/lib/utils";
import { selectActionCounts, selectChannels, useV2vStore } from "@/stores/v2v";
import type { CSSProperties } from "react";
import { useShallow } from "zustand/react/shallow";

/**
 * 侧栏里嵌在「视频流水线」下面的筛选列（v0.24.0）。
 *
 * ## 为什么主轴在侧栏而不在页里
 *
 * 「下一步动作」原来是页内一排筛选片，占掉页头整整两行。把它挪到侧栏之后，
 * 工作台那一屏才腾得出地方给大预览 —— 而这一页真正费眼睛的事恰恰是看片。
 * 更重要的是：它在侧栏时每一档的计数**常驻可见**，不必先点进去才知道
 * 「处理异常」那里躺着 4 条。
 *
 * ## 一次只选一个
 *
 * 上半截按动作切（拿它怎么办），下半截按通道切（它排在哪条队上）—— 两个**维度**，
 * 不是两个条件。做成交集之后侧栏会同时亮两行，而那两行的高亮长得一模一样：
 * 人读到的是「选了两样」，读不出「其中一样正在削另一样」。单选之后每一行的数字
 * 就是点进去会看到的条数，一个不多一个不少。判据单点在 `matchFilter`。
 *
 * ## 这里一个字的说明文字都不留
 *
 * 侧栏是全应用最挤的一列（246px 里还压着十一条路由）。每档那句「拿它怎么办」
 * 与每条通道那句「此刻堵没堵」都还在，只是搬进了 `title=` ——
 * 它们是**要用时才读**的东西，常驻摆着只会把九行筛选撑成一屏半。
 */
export function V2vNavCards() {
  const filter = useV2vStore((s) => s.filter);
  const setFilter = useV2vStore((s) => s.setFilter);
  const counts = useV2vStore(useShallow(selectActionCounts));
  const channels = useV2vStore(selectChannels);

  return (
    <div className="navfl">
      {WORKBENCH_ACTIONS.map((a) => {
        const m = ACTION_META[a];
        const on = filter.kind === "action" && filter.key === a;
        const n = counts[a] ?? 0;
        return (
          <button
            key={a}
            type="button"
            className={cn("navrow", on && "on", n === 0 && !on && "zero")}
            // 动作的色直接来自 `ACTION_META.dot`（与行内色点、摘要卡同源）；
            // 通道的色走 `data-tone`。两者最后都落在 `--tone` 上，故胶囊只有一套规则。
            style={{ "--tone": m.dot } as CSSProperties}
            title={m.note}
            onClick={() => setFilter({ kind: "action", key: a })}
          >
            <span className="fpill">{m.label}</span>
            <span className="f1" />
            <span className="n">{n}</span>
          </button>
        );
      })}

      {channels.length > 0 && <span className="navsep" />}

      {channels.map((c) => {
        const on = filter.kind === "channel" && filter.key === c.key;
        return (
          <button
            key={c.key || "(default)"}
            type="button"
            className={cn("navrow", on && "on", c.live === 0 && !on && "zero")}
            data-tone={c.tone}
            title={[
              c.key === "" ? "设置里没写默认型号，走 CLI 默认" : c.key,
              c.note,
              c.headline,
              c.title,
            ]
              .filter((s) => s !== "")
              .join("\n")}
            onClick={() => setFilter({ kind: "channel", key: c.key })}
          >
            <span className="fpill">{c.label}</span>
            <span className="f1" />
            {/* 数的是**还没走完的**，不是这条通道历史上的全部 —— 点进去看到的就是这些。 */}
            <span className="n">{c.live}</span>
          </button>
        );
      })}
    </div>
  );
}
