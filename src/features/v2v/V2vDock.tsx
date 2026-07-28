import { type NextAction, type Row, WORKBENCH_ACTIONS } from "@/features/v2v/model";
import { cn } from "@/lib/utils";
import { FolderOpen, RefreshCw, Send } from "lucide-react";

/**
 * 底坞 —— 动作全在这一条上，分成**这一条**与**这一屏**两组。
 *
 * ## 为什么必须分组
 *
 * 「通过」判的是光标那一条，「进看片流」进的是整屏 —— 两者长得一样却差着 46 倍。
 * 此前它们混在同一排里，唯一的区别是按钮上那个数字，而那个数字恰恰是最容易看漏的东西。
 *
 * ## 两组按钮都按**行**派生，不按筛选派生
 *
 * 「处理异常」这一档里就混着代价完全相反的几种：幽灵单重跑不花钱，超时重跑是第二份钱，
 * 提交超时连花没花都不知道。一套按钮套在整屏上必然对其中一类说错话，所以「这一条」
 * 读的是行本身（`row.signals` / `clip.billed` / `errorType`），与详情栏那几句提示同源。
 *
 * 「这一屏」同理：筛选改成单选之后，按通道筛出来的一屏里混着好几种下一步动作，
 * 而批量动作按定义是**逐动作**的（放行 / 验收 / 重跑各是一件事）。所以这里先看
 * 作用域里有几种动作 —— 只有一种才摆那个按钮，混着就说清「勾选同一类」。
 * 摆一个「放行这 83 条」而实际只有 12 条放得出去，是这一栏最不能犯的错。
 *
 * ## 「这一屏」的按钮把**作用域写进标签**
 *
 * 勾了就作用于勾选的，没勾就作用于整屏 —— 两种都合理，但按钮必须说出自己是哪一种，
 * 否则「放行 46 条」点下去只放行了 3 条（或反过来）都不会有人察觉。
 */
export interface DockHandlers {
  onSubmit: (ids: number[]) => void;
  onReview: (id: number, pass: boolean) => void;
  onRerun: (ids: number[]) => void;
  onRequeueRewrite: (ids: number[]) => void;
  onResume: (ids: number[]) => void;
  onUnqueue: (ids: number[]) => void;
  onEnterReview: () => void;
  onIngest: () => void;
  onOpenHandoff: () => void;
  onPollNow: () => void;
  onSwitchChannel: (ids: number[]) => void;
  onEditParams: (ids: number[]) => void;
  onUndo: () => void;
}

export function V2vDock({
  row,
  visible,
  sel,
  running,
  busy,
  undoLabel,
  h,
}: {
  row: Row | null;
  /** 当前这一屏（一个筛选，不是交集），已排序。 */
  visible: Row[];
  sel: Set<number>;
  /** 即梦手上在跑几条 —— 「立刻问一遍」实际会去问的就是这些。 */
  running: number;
  busy: boolean;
  undoLabel: string | null;
  h: DockHandlers;
}) {
  // 作用域恒是**这一屏的子集**：勾选过的条目可能因为状态变化离开了这一屏，
  // 把它们算进来会让按钮作用在一批看不见的条目上。
  const scope = sel.size > 0 ? visible.filter((r) => sel.has(r.clip.id)) : visible;
  const scoped = sel.size > 0 ? `选中 ${scope.length}` : `这 ${scope.length}`;
  const ids = scope.map((r) => r.clip.id);
  // 作用域里有哪几种下一步动作。批量按钮只在**恰好一种**时出现（见文件头注释）。
  const kinds = WORKBENCH_ACTIONS.filter((a) => scope.some((r) => r.action === a));
  const only = kinds.length === 1 ? kinds[0] : null;

  const switchable = scope.filter(
    (r) => r.stage === "ready" || r.stage === "rewrite" || r.stage === "run",
  );
  // 镜像 `repo::set_params` 的 `WHERE stage IN ('rewrite','ready')` —— 已提交的改了
  // 不会重新生效（那条视频用的是提交那一刻的参数），却会让详情栏显示的参数与它对不上。
  const editable = scope.filter((r) => r.stage === "ready" || r.stage === "rewrite");

  return (
    <div className="vdock">
      <span className="gl">这一条</span>
      {row == null ? (
        <span className="fs11 t3 nowrap">没有选中的条目</span>
      ) : (
        <RowButtons row={row} busy={busy} h={h} />
      )}

      <span className="sep" />
      <span className="gl">这一屏</span>
      {only ? (
        <BatchButton
          action={only}
          scope={scope}
          scoped={scoped}
          running={running}
          busy={busy}
          h={h}
        />
      ) : (
        <span className="fs11 t3 nowrap ohide">
          混着 {kinds.length} 种下一步动作 —— 勾选同一类，或左边按动作筛一次
        </span>
      )}

      <button
        type="button"
        className="btn sm"
        disabled={busy || switchable.length === 0}
        title={
          switchable.length === 0
            ? "这一屏里没有能改投的条目（已出片或已定案的换了也没有意义）"
            : `把 ${switchable.length} 条改投到另一条即梦队列。还在本地排队的换起来一分钱不花 —— 即梦对它们一无所知。`
        }
        onClick={() => h.onSwitchChannel(switchable.map((r) => r.clip.id))}
      >
        换通道
      </button>
      <button
        type="button"
        className="btn sm"
        disabled={busy || editable.length === 0}
        title={
          editable.length === 0
            ? "这一屏里没有还能改参数的条目 —— 已提交的改了不会重新生效"
            : `改这 ${editable.length} 条的模型 / 时长 / 分辨率`
        }
        onClick={() => h.onEditParams(editable.map((r) => r.clip.id))}
      >
        改参数
      </button>

      <div className="f1" />
      {undoLabel ? (
        <span className="vundo">
          <span className="ohide">{undoLabel}</span>
          <button type="button" onClick={h.onUndo}>
            撤销 U
          </button>
        </span>
      ) : (
        // 只报**这一屏真的能用**的键。`⌘⏎ 确认提交` 挂在一屏待改写上时，按下去
        // 得到的是一句「请先选中待放行的条目」—— 一个照着提示按却没反应的键，
        // 比不写这条提示更伤。混着几种动作时按光标那一条报，因为这些键判的就是它。
        <span className="fs11 t3 nowrap ohide">
          {ids.length === 0
            ? "↑↓ 换条 · ⌥\\ 账与历程"
            : (only ?? row?.action) === "review"
              ? "↑↓ 换条 · 空格 通过 · X 不通过 · ⏎ 全屏看片"
              : (only ?? row?.action) === "submit"
                ? "↑↓ 换条 · ⌘⏎ 确认提交 · F 对照首帧"
                : "↑↓ 换条 · ⌥\\ 账与历程 · ⌥1/2/3 观测·日志·参数"}
        </span>
      )}
    </div>
  );
}

/**
 * 「这一条」的按钮 —— 读行本身，不读筛选。
 *
 * 读 `row.action` 而不是当前筛的那一档：按通道筛出来的一屏里两者根本不是同一件事，
 * 而这几个按钮判的自始至终都是光标那一条。
 */
function RowButtons({ row, busy, h }: { row: Row; busy: boolean; h: DockHandlers }) {
  const action = row.action;
  const id = row.clip.id;
  const one = [id];
  const back = (
    <button
      key="back"
      type="button"
      className="btn sm gho"
      disabled={busy}
      title="清掉视频提示词，退回待改写让 skill 重写"
      onClick={() => h.onRequeueRewrite(one)}
    >
      退回改写 <span className="kh">E</span>
    </button>
  );

  if (action === "rewrite") {
    // 这一步只可能由人在 Claude Code / Codex 里推动，GenDesk 这边工单早已备好 ——
    // 所以「这一条」上没有任何真的动作，与其摆一个点了没反应的按钮，不如说清楚。
    return <span className="fs11 t3 nowrap">等 skill 写回改写结果 · 动作在右边这一档</span>;
  }

  if (action === "submit") {
    return (
      <>
        <button
          type="button"
          className="btn sm pri"
          disabled={busy}
          onClick={() => h.onSubmit(one)}
        >
          <Send className="ic12" />
          放行这一条{row.estimate != null && ` · ${row.estimate} 额度`}{" "}
          <span className="kh">⌘⏎</span>
        </button>
        {back}
      </>
    );
  }

  if (action === "queued") {
    return (
      <>
        <button
          type="button"
          className="btn sm gho"
          disabled={busy}
          title="它还没发出去、一分钱没扣 —— 撤回后退回「等你点确认提交」"
          onClick={() => h.onUnqueue(one)}
        >
          撤回放行
        </button>
        {back}
      </>
    );
  }

  if (action === "review") {
    return (
      <>
        <button
          type="button"
          className="btn sm okb"
          disabled={busy}
          onClick={() => h.onReview(id, true)}
        >
          通过 <span className="kh">空格</span>
        </button>
        <button
          type="button"
          className="btn sm dngo"
          disabled={busy}
          onClick={() => h.onReview(id, false)}
        >
          不通过 <span className="kh">X</span>
        </button>
        <button
          type="button"
          className="btn sm gho"
          disabled={busy}
          title="用同一条视频提示词再抽一次（回到待提交，确认后重新扣额度）"
          onClick={() => h.onRerun(one)}
        >
          重跑 <span className="kh">R</span>
        </button>
        {back}
      </>
    );
  }

  if (action === "fix") {
    const phantom = row.signals.has("phantom");
    const timeout = row.signals.has("timeout");
    const risky = row.clip.errorType === "submit_timeout";
    return (
      <>
        {timeout ? (
          <button
            type="button"
            className="btn sm pri"
            disabled={busy}
            title="沿用原提交单放回轮询，不重新提交、不再扣额度"
            onClick={() => h.onResume(one)}
          >
            继续等待 <span className="kh">W</span>
          </button>
        ) : null}
        <button
          type="button"
          className={cn("btn sm", phantom ? "pri" : risky || row.clip.billed ? "dngo" : "gho")}
          disabled={busy}
          title={
            phantom
              ? "从未计费，重跑不花钱"
              : risky
                ? "提交时 CLI 超时被杀，可能已经下过单 —— 先核对再重跑"
                : row.clip.billed
                  ? "这一条即梦已经扣过费了，重跑是第二份钱"
                  : "回到待提交，确认后重新扣额度"
          }
          onClick={() => h.onRerun(one)}
        >
          {phantom ? "免费重跑" : "重跑"} <span className="kh">R</span>
        </button>
        {back}
      </>
    );
  }

  // wait：这一条在即梦手上。**没有**「只问这一条」的命令（`poll_v2v_now` 是全量的），
  // 所以那个按钮只出现在「这一档」，标签也写明它会去问全部在跑的。
  return (
    <>
      <button
        type="button"
        className={cn("btn sm", row.clip.billed ? "dngo" : "gho")}
        disabled={busy}
        title={
          row.clip.billed
            ? "即梦已经扣过这一条的费了 —— 重跑会丢弃那一单，是第二份钱。想换条队用「换通道」。"
            : "回到待提交，确认后重新扣额度"
        }
        onClick={() => h.onRerun(one)}
      >
        重跑 <span className="kh">R</span>
      </button>
      {back}
    </>
  );
}

/** 「这一档」——一个按钮，作用域写在标签上。 */
function BatchButton({
  action,
  scope,
  scoped,
  running,
  busy,
  h,
}: {
  action: NextAction;
  scope: Row[];
  scoped: string;
  running: number;
  busy: boolean;
  h: DockHandlers;
}) {
  const ids = scope.map((r) => r.clip.id);

  if (action === "rewrite") {
    return (
      <>
        <button type="button" className="btn sm pri" disabled={busy} onClick={h.onIngest}>
          <RefreshCw className="ic12" />
          写完了 · 收录改写结果
        </button>
        <button type="button" className="btn sm gho" onClick={h.onOpenHandoff}>
          <FolderOpen className="ic12" />
          打开交接目录
        </button>
      </>
    );
  }

  if (action === "submit") {
    // 有一条查不到单价就不摆合计数（`estimate` 为 null）—— 半真半假的钱数比没有更糟。
    const cost = scope.reduce<number | null>(
      (a, r) => (a == null || r.estimate == null ? null : a + r.estimate),
      0,
    );
    return (
      <button
        type="button"
        className="btn sm pri"
        disabled={busy || ids.length === 0}
        onClick={() => h.onSubmit(ids)}
      >
        <Send className="ic12" />
        放行{scoped} 条{cost != null && ` · ${cost} 额度`}
      </button>
    );
  }

  if (action === "queued") {
    return (
      <button
        type="button"
        className="btn sm gho"
        disabled={busy || ids.length === 0}
        title="它们还没发出去、一分钱没扣 —— 撤回后退回「等你点确认提交」"
        onClick={() => h.onUnqueue(ids)}
      >
        撤回放行{scoped} 条
      </button>
    );
  }

  if (action === "review") {
    return (
      <button
        type="button"
        className="btn sm okb"
        disabled={ids.length === 0}
        onClick={h.onEnterReview}
      >
        进看片流判{scoped} 条
      </button>
    );
  }

  if (action === "fix") {
    // 只重跑**没扣过费**的：扣过费的重跑会丢弃一份已付费的任务，那种事不该由一个
    // 批量按钮顺手做掉。它们仍可以逐条重跑（那条路上有确认卡）。
    const free = scope.filter((r) => !r.clip.billed);
    return (
      <button
        type="button"
        className="btn sm pri"
        disabled={busy || free.length === 0}
        title={
          free.length === 0
            ? "这一屏里的异常条目都已经扣过费了 —— 重跑要逐条来，那条路上有确认卡"
            : "这些从未计费（幽灵单、被并发上限弹回的），重跑不花钱"
        }
        onClick={() => h.onRerun(free.map((r) => r.clip.id))}
      >
        免费重跑可重跑的 {free.length} 条
      </button>
    );
  }

  // wait：`poll_v2v_now` 是**全量**的 —— 它去问即梦手上所有在跑的条目，不认这一屏
  // 勾了谁。所以标签绝不能写「刷新这 18 条」，那是在按钮上说谎。
  return (
    <button
      type="button"
      className="btn sm gho"
      disabled={busy}
      title="逐条问一遍即梦：队列位次、生成状态、扣费额度、已出的片，全部现取。本地队列里那些即梦还不知道，问不到。"
      onClick={h.onPollNow}
    >
      <RefreshCw className="ic12" />
      立刻问一遍即梦（在跑 {running} 条）
    </button>
  );
}
