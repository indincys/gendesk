import {
  type Channel,
  type Filter,
  type NextAction,
  type Row,
  WORKBENCH_ACTIONS,
  buildChannels,
  deriveRows,
  matchFilter,
  rankRows,
  topChannels,
} from "@/features/v2v/model";
import {
  type ActivityEntry,
  type AutofillStatus,
  type ClipView,
  type CreditStats,
  type EffectiveParams,
  type HandoffStatus,
  type ModelInfo,
  type QueueStats,
  type StageCounts,
  type V2vRefresh,
  type V2vTick,
  commands,
  subscribeV2v,
  unwrap,
} from "@/lib/ipc";
import { create } from "zustand";

/**
 * 视频流水线的镜像与筛选态（Zustand 只做事件镜像与 UI 态，业务真相在 Rust）。
 *
 * ## 为什么它比「一个徽章计数」大得多（v0.24.0）
 *
 * 主轴（下一步动作）与通道筛选搬进了**侧栏**，通道状态灯、刷新与余额搬进了**顶栏**
 * —— 这三处都在页面组件之外。数据要是还只活在 `V2vPage` 的 `useState` 里，侧栏就得
 * 自己再取一份、再自己 `deriveRows` 一遍，而那两份迟早会对不上：侧栏说「待放行 9」，
 * 列表里躺着 7 条。所以镜像与筛选态都在这里，**只有一份**。
 *
 * ## 重活按路由开关
 *
 * `listV2vClips` 是全表读，每来一次 `v2v://changed` 就重取一次。在别的页面上挂着它
 * 纯是白烧 —— 那时既没有侧栏那两张卡，也没有列表。故 `enter()` 由 V2vPage 在 mount
 * 时调、unmount 时清理；`init()` 只留下徽章要用的 `counts`，那个必须在任何页面上都对。
 *
 * ## 变更动作不在这里
 *
 * 提交 / 验收 / 重跑 / 换通道 / 收录改写都留在页面：它们要串 toast、确认卡、
 * 判完之后把光标推到下一条。搬进来只会让 store 变成第二个页面。
 */

/** 轮询事件带回来的实时进度（`v2v://progress` 的载荷，去掉 clipId）。 */
export interface LiveProgress {
  genStatus: string;
  queueIdx: number | null;
  polledAt: number;
}

/** 执行日志缓冲上限。与 Rust 侧 `activity` 环形缓冲同数 —— 两边不一致会让「全部日志」
    比「这一条的日志」多出一截没人解释得清的历史。 */
const ACTIVITY_CAP = 500;

interface V2vState {
  // ── 镜像 ───────────────────────────────────────────
  counts: StageCounts;
  clips: ClipView[];
  models: ModelInfo[];
  eff: EffectiveParams | null;
  queue: QueueStats | null;
  credit: CreditStats | null;
  handoff: HandoffStatus | null;
  autofill: AutofillStatus | null;
  tick: V2vTick | null;
  refresh: V2vRefresh | null;
  activity: ActivityEntry[];
  progress: Record<number, LiveProgress>;
  /**
   * 派生用的**粗秒表**（30 秒一跳）。
   *
   * `deriveRows` 要遍历全表、逐行重算判据与情况文案，喂一个 1 秒秒表等于每秒重算
   * 一整页；而它用到的时间全是「已等多久 / 多久前查过」这类以分钟为单位读的值。
   * 顶栏那个「上次查询 12 秒前」要每秒走字，故它读自己的秒表，不读这个。
   */
  coarseNow: number;

  // ── 筛选与选择（UI 态） ─────────────────────────────
  /** **一次只有一个**（见 `Filter` 的注释）—— 动作与通道是两个维度，不做交集。 */
  filter: Filter;
  /**
   * 上一次选过的**动作**档。
   *
   * 通道筛选的入口现在是顶栏那排灯与列表顶上那排快捷片（侧栏只剩流程六档），
   * 而两维互斥 —— 所以按通道筛完之后必须有一条路走回来。再点一次那枚通道片就回到
   * 这一档：不必去侧栏找「刚才我在哪一档」，也不必凭空多一枚「全部」按钮
   * （它答不出「全部什么」）。
   */
  lastAction: NextAction;
  sel: Set<number>;
  /**
   * 「按住 Shift 选一段」的锚点 —— 上一次**单独**点中的那一行。
   *
   * 不用 `cur`：光标会被 ↑↓ 推着走，那样一段的起点会跟着键盘漂，选出来的范围与
   * 人以为的对不上。锚点只在明确点了某一行（或勾了某一行）时才移动。
   */
  anchor: number | null;
  cur: number | null;
  /** 中栏（这一条的账与进度）开着没有。窄屏下它是抽屉。 */
  ledgerOpen: boolean;

  // ── 动作 ───────────────────────────────────────────
  init: () => Promise<() => void>;
  refreshCounts: () => Promise<void>;
  /** 进入工作台：取全量 + 订阅。返回清理函数。 */
  enter: () => Promise<() => void>;
  reload: () => Promise<void>;
  reloadHandoff: () => Promise<void>;
  reloadEff: () => Promise<void>;
  setFilter: (f: Filter) => void;
  /** 通道快捷片：点已选中的那一枚回到上一次的动作档（见 `lastAction`）。 */
  toggleChannel: (key: string) => void;
  setCur: (id: number | null) => void;
  setSel: (fn: (cur: Set<number>) => Set<number>) => void;
  setAnchor: (id: number | null) => void;
  clearSel: () => void;
  toggleLedger: () => void;
}

const EMPTY: StageCounts = {
  rewrite: 0,
  ready: 0,
  run: 0,
  rev: 0,
  pass: 0,
  rej: 0,
  fail: 0,
  phantom: 0,
  actionable: 0,
  undelivered: 0,
};

export const useV2vStore = create<V2vState>((set, get) => ({
  counts: EMPTY,
  clips: [],
  models: [],
  eff: null,
  queue: null,
  credit: null,
  handoff: null,
  autofill: null,
  tick: null,
  refresh: null,
  activity: [],
  progress: {},
  coarseNow: Math.floor(Date.now() / 1000 / 30) * 30,

  // 默认落在**第一个不为空的档**上。
  //
  // v0.24.0 之前默认是「需要我」（四档的并集），而那个聚合片随主轴搬进侧栏一起去掉了。
  // 直接写死 `submit` 会让一个手上全是待验收的人一进来看到空列表 —— 所以这里存一个
  // 占位值，`enter()` 拿到数据后按 `WORKBENCH_ACTIONS` 的序落到第一个有条目的档，
  // 与旧的「一进来看到等你动手的」同义，且不会出现「默认档是空的」。
  filter: { kind: "action", key: "review" },
  lastAction: "review",
  sel: new Set(),
  anchor: null,
  cur: null,
  /**
   * 中栏（账与进度）默认开着，**除非窗口窄到摆不下三栏**。
   *
   * 三栏最小宽加起来约 1400px，而应用最小宽是 1140 —— 窄于 1400 时中栏由 CSS 变成
   * 一层覆盖在预览上的抽屉（见 globals.css 的 media query）。默认开着的话，
   * 窄屏用户一进来就被一块盖住画面的抽屉迎面撞上。
   *
   * 只在启动时取一次，之后由 ⌥\ 说了算：跟着 resize 自动开合会在人拖窗口时
   * 把他刚刚手动收起来的那一栏又弹回来。
   */
  ledgerOpen: window.innerWidth >= 1400,

  init: async () => {
    await get().refreshCounts();
    // 事件里已带全量计数 → 直接镜像，不再回查一次（铁律 4：事件驱动不轮询）。
    return subscribeV2v({ onChanged: (e) => set({ counts: e.counts }) });
  },

  refreshCounts: async () => {
    try {
      set({ counts: await unwrap(commands.v2vCounts()) });
    } catch {
      // 读不到不该影响别的启动步骤；下一次事件会纠正。
      set({ counts: EMPTY });
    }
  },

  reload: async () => {
    set({ clips: await unwrap(commands.listV2vClips([])) });
    void unwrap(commands.v2vQueueStats())
      .then((queue) => set({ queue }))
      .catch(() => {});
    // 常驻队列的状态跟着每次事件刷新：它会在无人操作时自己变（补单、被日限挡住）。
    void unwrap(commands.v2vAutofillStatus())
      .then((autofill) => set({ autofill }))
      .catch(() => {});
  },

  // 不挂在每个 tick 上：`v2v_handoff_status` 会顺手重写工单，不是只读的。
  reloadHandoff: async () => {
    try {
      set({ handoff: await unwrap(commands.v2vHandoffStatus()) });
    } catch {
      /* 交接目录读不到时横幅会自己说，不必在这里炸 */
    }
  },

  reloadEff: async () => {
    try {
      set({ eff: await unwrap(commands.v2vEffectiveParams()) });
    } catch {
      /* 拿不到就少显示一段回落信息 */
    }
  },

  enter: async () => {
    const s = get();
    await s.reload().catch(() => {});
    void unwrap(commands.v2vModels())
      .then((models) => set({ models }))
      .catch(() => set({ models: [] }));
    void s.reloadEff();
    void s.reloadHandoff();
    // 余额要跑一次 CLI（秒级），与页面主体并行加载，拉不到就少显示一段。
    void unwrap(commands.v2vCreditStats())
      .then((credit) => set({ credit }))
      .catch(() => {});
    void unwrap(commands.v2vActivity())
      .then((activity) => set({ activity }))
      .catch(() => {});

    // 默认档：只在**当前这一屏是空的**时候才自动挪（见 `filter` 的注释）。
    // 每次进页面都强行重置的话，切出去看一眼成片再回来，人刚选好的筛选就没了。
    const rows = selectRows(get());
    if (!rows.some((r) => matchFilter(r, get().filter))) {
      const first = WORKBENCH_ACTIONS.find((a) => rows.some((r) => r.action === a));
      if (first) set({ filter: { kind: "action", key: first }, lastAction: first });
    }

    const un = await subscribeV2v({
      onChanged: (e) => {
        set({ counts: e.counts });
        void get()
          .reload()
          .catch(() => {});
      },
      // 位次一起收下。只取 `genStatus` 的话，轮询刚问到的新位次要等下一次
      // `listV2vClips` 才看得见，而这两件事之间隔着整整一轮（非 VIP 600 秒）。
      onProgress: (e) =>
        set((c) => ({
          progress: {
            ...c.progress,
            [e.clipId]: { genStatus: e.genStatus, queueIdx: e.queueIdx, polledAt: e.polledAt },
          },
        })),
      onTick: (tick) => set({ tick }),
      onRefresh: (refresh) => set({ refresh }),
      // 按 seq 去重 —— 取快照与收到事件之间那一瞬产生的条目会同时出现在两边。
      onActivity: (e) =>
        set((c) =>
          c.activity.some((r) => r.seq === e.entry.seq)
            ? c
            : { activity: [...c.activity, e.entry].slice(-ACTIVITY_CAP) },
        ),
    });

    // 只在粗秒表**真的跳格**时写回：写一个相同的值也会让全体订阅者重跑一遍选择器。
    const timer = setInterval(() => {
      const t = Math.floor(Date.now() / 1000 / 30) * 30;
      if (t !== get().coarseNow) set({ coarseNow: t });
    }, 5_000);
    return () => {
      un();
      clearInterval(timer);
    };
  },

  // 换筛选就把光标、勾选与锚点清掉：它们指向的条目多半已经不在这一屏里了，
  // 留着会让底坞按钮作用在一批看不见的条目上。
  setFilter: (filter) =>
    set({
      filter,
      ...(filter.kind === "action" ? { lastAction: filter.key } : {}),
      cur: null,
      sel: new Set(),
      anchor: null,
    }),
  toggleChannel: (key) => {
    const s = get();
    const on = s.filter.kind === "channel" && s.filter.key === key;
    s.setFilter(on ? { kind: "action", key: s.lastAction } : { kind: "channel", key });
  },
  setCur: (cur) => set({ cur }),
  setSel: (fn) => set((c) => ({ sel: fn(c.sel) })),
  setAnchor: (anchor) => set({ anchor }),
  clearSel: () => set({ sel: new Set(), anchor: null }),
  toggleLedger: () => set((c) => ({ ledgerOpen: !c.ledgerOpen })),
}));

/**
 * 全部行。**只在数据真的变了时重算**（`deriveRows` 遍历全表）。
 *
 * 用一个模块级的记忆而不是 `useMemo`：侧栏与页面是两个组件，各自 `useMemo` 就是各算
 * 一遍；而它们必须拿到**同一个数组**，否则 `byId` / 光标 / 勾选会指到两份不同的对象上。
 */
let rowsMemo: { key: unknown[]; rows: Row[] } | null = null;

export function selectRows(s: V2vState): Row[] {
  const limits = new Map((s.queue?.channels ?? []).map((c) => [c.modelVersion, c.limit]));
  // 依赖里放 `queue.channels` 的构成签名而不是 queue 对象本身：后者每次心跳都是新对象，
  // 直接比引用等于每 6 秒重算一整页。
  const key = [
    s.clips,
    s.models,
    s.eff,
    s.coarseNow,
    (s.queue?.channels ?? []).map((c) => `${c.modelVersion}:${c.limit}`).join(","),
  ];
  if (
    rowsMemo &&
    rowsMemo.key.length === key.length &&
    rowsMemo.key.every((v, i) => v === key[i])
  ) {
    return rowsMemo.rows;
  }
  const rows = deriveRows(s.clips, s.models, s.eff, s.coarseNow, limits);
  rowsMemo = { key, rows };
  return rows;
}

let channelsMemo: { key: unknown[]; channels: Channel[] } | null = null;

/** 通道卡的行。同上，侧栏与页面共用一份（配色必须同源）。 */
export function selectChannels(s: V2vState): Channel[] {
  const rows = selectRows(s);
  const key = [rows, s.models, s.queue?.channels];
  if (
    channelsMemo &&
    channelsMemo.key.length === key.length &&
    channelsMemo.key.every((v, i) => v === key[i])
  ) {
    return channelsMemo.channels;
  }
  const channels = buildChannels(rows, s.models, s.queue?.channels ?? []);
  channelsMemo = { key, channels };
  return channels;
}

let topMemo: { key: Channel[]; top: Channel[] } | null = null;

/**
 * 顶栏那排状态灯与列表顶上那排快捷筛选片读的**同一份**前三通道。
 *
 * 两处显示不同的三条通道，比只有一处更糟：人会以为它们是两组不同的东西，
 * 然后花时间去找那个并不存在的区别。
 */
export function selectTopChannels(s: V2vState): Channel[] {
  const channels = selectChannels(s);
  if (topMemo && topMemo.key === channels) return topMemo.top;
  const top = topChannels(channels);
  topMemo = { key: channels, top };
  return top;
}

let countsMemo: { key: Row[]; counts: Record<NextAction, number> } | null = null;

/**
 * 每一档的计数 —— 全流水线的，与当前筛选无关。
 *
 * 筛选改成单选之后这条不再需要论证：侧栏每一行的数字**就是**点进去会看到的条数。
 * 交集时代它得刻意不跟着通道走（否则选了 2.0Fast 之后「处理异常 0」会把另一条队上
 * 那 4 条异常整个藏起来），代价是那个数字与点进去看到的条数对不上。现在两者同义。
 */
export function selectActionCounts(s: V2vState): Record<NextAction, number> {
  const rows = selectRows(s);
  if (countsMemo && countsMemo.key === rows) return countsMemo.counts;
  const counts = Object.fromEntries(WORKBENCH_ACTIONS.map((a) => [a, 0])) as Record<
    NextAction,
    number
  >;
  for (const r of rows) if (r.action in counts) counts[r.action] += 1;
  countsMemo = { key: rows, counts };
  return counts;
}

let visibleMemo: { key: unknown[]; rows: Row[] } | null = null;

/**
 * 当前这一屏（一个筛选，不是交集），已排序。
 *
 * 也要记忆：`Array.filter` + `rankRows` 每次都产出新数组，而 Zustand 的选择器按
 * `Object.is` 比较 —— 不记忆的话每一次心跳（6 秒一发）都会让整张表重渲染一遍，
 * 而重渲染会把正在播放的 `<video>` 也一起顶掉。
 *
 * 依赖里放的是 `filter` 的两个字段而不是对象本身：`setFilter` 每次都造新对象，
 * 但同一个筛选点两下不该让整张表重算。
 */
export function selectVisible(s: V2vState): Row[] {
  const all = selectRows(s);
  const key = [all, s.filter.kind, s.filter.key];
  if (
    visibleMemo &&
    visibleMemo.key.length === key.length &&
    visibleMemo.key.every((v, i) => v === key[i])
  ) {
    return visibleMemo.rows;
  }
  const rows = rankRows(all.filter((r) => matchFilter(r, s.filter)));
  visibleMemo = { key, rows };
  return rows;
}
