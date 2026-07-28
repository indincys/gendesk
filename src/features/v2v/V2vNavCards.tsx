import { ACTION_META, WORKBENCH_ACTIONS } from "@/features/v2v/model";
import { cn } from "@/lib/utils";
import { selectActionCounts, selectChannels, useV2vStore } from "@/stores/v2v";
import { useShallow } from "zustand/react/shallow";

/**
 * 侧栏里嵌在「视频流水线」下面的两张筛选卡（v0.24.0）。
 *
 * ## 为什么主轴在侧栏而不在页里
 *
 * 「下一步动作」原来是页内一排筛选片，占掉页头整整两行。把它挪到侧栏之后，
 * 工作台那一屏才腾得出地方给大预览 —— 而这一页真正费眼睛的事恰恰是看片。
 * 更重要的是：动作卡在侧栏时，每一档的计数**常驻可见**，不必先点进去才知道
 * 「处理异常」那里躺着 4 条。
 *
 * ## 两张卡是**叠加**的，不是二选一
 *
 * 动作答「拿它怎么办」，通道答「它排在哪条队上」—— 两个正交的问题。叠加之后
 * 「2.0Fast 这条队上还有几条等我放行」才是一个问得出来的问题，而那正是换通道
 * 之前要看的数。
 *
 * ## 计数不跟着通道走
 *
 * 动作卡的数字是**全流水线**的，不受通道筛选影响（`selectActionCounts`）。
 * 跟着走的话，选了 2.0Fast 之后「处理异常 0」会把另一条队上那 4 条异常整个藏起来
 * —— 而那正是最该被看见的东西。
 */
export function V2vNavCards() {
  const action = useV2vStore((s) => s.action);
  const channel = useV2vStore((s) => s.channel);
  const setAction = useV2vStore((s) => s.setAction);
  const setChannel = useV2vStore((s) => s.setChannel);
  const counts = useV2vStore(useShallow(selectActionCounts));
  const channels = useV2vStore(selectChannels);

  return (
    <>
      <div className="navcard act">
        <div className="hd">下一步动作</div>
        {WORKBENCH_ACTIONS.map((a) => {
          const m = ACTION_META[a];
          const on = action === a;
          const n = counts[a] ?? 0;
          return (
            <button
              key={a}
              type="button"
              className={cn("navrow", on && "on", n === 0 && !on && "zero")}
              onClick={() => setAction(a)}
            >
              <span className={cn("dot", on && "ring")} style={{ background: m.dot }} />
              <span className="bd">
                <span className="lb">{m.label}</span>
                {/* 副行只在选中时展开 —— 六行同时挂着说明会把这张卡撑到一屏半，
                    而人一次只在一档里干活。「处理异常」是唯一的例外：它那句说的是
                    「重跑之前先看花没花钱」，而那件事**不该等人点进去才被告知**。 */}
                {(on || (a === "fix" && n > 0)) && m.note !== "" && (
                  <span className="nt">{m.note}</span>
                )}
              </span>
              <span className="n">{n}</span>
            </button>
          );
        })}
      </div>

      <div className="navcard chn">
        <div className="hd">
          通道
          <span className="f1" />
          <span className="hint">与动作叠加</span>
        </div>
        <button
          type="button"
          className={cn("navrow all", channel == null && "on")}
          onClick={() => setChannel(null)}
        >
          <span className="bd">
            <span className="lb">全部通道</span>
          </span>
          <span className="n">{channels.length}</span>
        </button>
        {channels.map((c) => {
          const on = channel === c.key;
          return (
            <button
              key={c.key || "(default)"}
              type="button"
              className={cn("navrow ch", on && "on")}
              // 摘要挂在 title 上：它是「这条队做到哪了」的全貌，一行副行装不下，
              // 而副行那格已经让给了「此刻堵没堵」——后者才是会改变决策的那句。
              title={`${c.key === "" ? "设置里没写默认型号，走 CLI 默认" : c.key}\n${c.title}\n${c.headline}`}
              onClick={() => setChannel(on ? null : c.key)}
            >
              <span className="rail" data-tone={c.tone} />
              <span className="bd">
                <span className={cn("cpill", c.vip && "vip")} data-tone={c.tone}>
                  {c.label}
                </span>
                {(on || c.note !== "") && <span className="nt">{c.note || c.title}</span>}
              </span>
              <span className="n">{c.rows.length}</span>
            </button>
          );
        })}
        {channels.length === 0 && <div className="empty">流水线上还没有条目</div>}
      </div>
    </>
  );
}
