import { ConfirmModal, Modal } from "@/components/ui/Modal";
import { type DockHandlers, V2vDock } from "@/features/v2v/V2vDock";
import { V2vLedger, worstGroup } from "@/features/v2v/V2vLedger";
import { V2vList } from "@/features/v2v/V2vList";
import { V2vLogPanel } from "@/features/v2v/V2vLogPanel";
import { type Params, V2vParamPicker } from "@/features/v2v/V2vParamPicker";
import { V2vParamsPanel } from "@/features/v2v/V2vParamsPanel";
import { V2vPreview } from "@/features/v2v/V2vPreview";
import { V2vQueuePanel } from "@/features/v2v/V2vQueuePanel";
import { V2vCreditDaily, V2vQueueTrend } from "@/features/v2v/V2vQueueTrend";
import { V2vReviewFlow } from "@/features/v2v/V2vReviewFlow";
import {
  type Row,
  SORTS,
  type SortKey,
  carryParams,
  creditPerSec,
  fmtAgo,
  sliceSummary,
} from "@/features/v2v/model";
import {
  type CreditStats,
  type ModelInfo,
  type SubmitPreview,
  type V2vTick,
  type V2vUndoEntry,
  commands,
  subscribeV2v,
  unwrap,
} from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { useUiStore } from "@/stores/ui";
import { selectChannels, selectVisible, useV2vStore } from "@/stores/v2v";
import { Clapperboard, RefreshCw, Send } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { toast } from "sonner";

/**
 * 视频流水线工作台（v0.24.0 按 Claude Design 原型重做）。
 *
 * ## 三处结构性判断
 *
 * 1. **主轴与通道搬进侧栏**（`V2vNavCards`）。「下一步动作」原来是页内一排筛选片，
 *    占掉页头两整行；通道原来是把表格切成若干节。搬走之后这一屏才腾得出地方给
 *    大预览 —— 而这一页真正费眼睛的事恰恰是看片（判色差与形变）。
 *    两者**叠加**：动作答「拿它怎么办」，通道答「它排在哪条队上」，正交。
 * 2. **通道状态灯 / 刷新 / 余额搬进顶栏**（`V2vTitleChrome`）。它们回答的都是
 *    「远端此刻是什么状况」，与页里那三栏不是一回事。
 * 3. **三栏 + 底坞**：预览 ｜ 这一条的账与历程 ｜ 列表；底坞把「这一条」与「这一档」
 *    的动作分成两组 —— 此前它们混在同一排，唯一的区别是按钮上那个数字。
 *
 * ## 仍然成立的老判断
 *
 * - **主轴是「下一步动作」不是阶段**：阶段就写在每一条脸上，是最不缺的信息；
 *   真正没人回答的是「所以我现在该干嘛」。派生在 `model.ts` 的 `nextAction`，
 *   故筛选、摘要、行内色点三者同源。
 * - **详情常驻**：「这一条花没花钱」放不进一行，而开弹窗一条条看太慢。
 *
 * ## 键盘
 *
 * ↑/↓（或 ←/→、J/K）换条 · 空格 通过 · X 不通过 · R 重跑 · E 退回改写 · W 继续等待 ·
 * U 撤销 · F 对照首帧 · ⏎ 全屏看片 · ⌘⏎ 确认提交 · ⌥\ 账与历程栏 · ⌥1/2/3 观测/日志/参数。
 */

/**
 * 换通道要写下去的一套参数。
 *
 * 与 `Params` 的区别是 `duration` **不可为 null** —— 即梦只接受「三者都不给」或
 * 「一套完整组合」，而换通道属于后者。用 `Required<Params>` 顶不了这个：它只去掉
 * 可选性，`number | null` 里的 null 照样留着。
 */
type ChannelParams = { modelVersion: string; duration: number; videoResolution: string };

export function V2vPage() {
  // ── 镜像（store 持有，侧栏与顶栏读的是同一份） ──────────
  const enter = useV2vStore((s) => s.enter);
  const reload = useV2vStore((s) => s.reload);
  const reloadHandoff = useV2vStore((s) => s.reloadHandoff);
  const reloadEff = useV2vStore((s) => s.reloadEff);
  const clips = useV2vStore((s) => s.clips);
  const models = useV2vStore((s) => s.models);
  const handoff = useV2vStore((s) => s.handoff);
  const autofill = useV2vStore((s) => s.autofill);
  const credit = useV2vStore((s) => s.credit);
  const queue = useV2vStore((s) => s.queue);
  const tick = useV2vStore((s) => s.tick);
  const activity = useV2vStore((s) => s.activity);
  const coarseNow = useV2vStore((s) => s.coarseNow);

  const action = useV2vStore((s) => s.action);
  const channel = useV2vStore((s) => s.channel);
  const sort = useV2vStore((s) => s.sort);
  const sel = useV2vStore((s) => s.sel);
  const cur = useV2vStore((s) => s.cur);
  const ledgerOpen = useV2vStore((s) => s.ledgerOpen);
  // 换档由侧栏那张动作卡负责（`V2vNavCards`），页里不再有第二个入口。
  const setSort = useV2vStore((s) => s.setSort);
  const setCur = useV2vStore((s) => s.setCur);
  const setSel = useV2vStore((s) => s.setSel);
  const clearSel = useV2vStore((s) => s.clearSel);
  const toggleLedger = useV2vStore((s) => s.toggleLedger);

  const visible = useV2vStore(selectVisible);
  const channels = useV2vStore(selectChannels);

  // ── 界面态 ───────────────────────────────────────────
  const [screen, setScreen] = useState<"list" | "review">("list");
  const [showFrame, setShowFrame] = useState(false);
  const [showLog, setShowLog] = useState(false);
  const [showParams, setShowParams] = useState(false);
  const [showObserve, setShowObserve] = useState(false);
  const [cmdPreview, setCmdPreview] = useState<{ ids: number[]; data: SubmitPreview } | null>(null);
  /** 换通道面板要处置的行。 */
  const [switching, setSwitching] = useState<Row[] | null>(null);
  /** 「改参数」面板要处置的行。 */
  const [editing, setEditing] = useState<Row[] | null>(null);
  /**
   * 重跑前的确认：只在选中里**确实有已扣费的在跑条目**时才出现。
   *
   * 幽灵单与未计费的在跑条目不弹 —— 它们重跑本来就免费，多一次确认只会训练出
   * 盲点头的习惯，等真正要花钱的那次弹出来时就没人读了。
   */
  const [confirmRerun, setConfirmRerun] = useState<{
    ids: number[];
    paid: number;
    credit: number;
    advanceFrom?: number;
  } | null>(null);
  const [busy, setBusy] = useState(false);
  /** 撤销令牌由 Rust 造，前端只当信封（见 `V2vAction` 的注释）。 */
  const [undo, setUndo] = useState<{ label: string; entries: V2vUndoEntry[] } | null>(null);
  /** 本轮（进入看片流后）判了多少 —— 顶部那句「已过 N · 已毙 M」。 */
  const [tally, setTally] = useState({ passed: 0, killed: 0 });

  // 重入锁用 ref 而非 state：useState 要等下一次渲染才生效，挡不住同一帧内的连点。
  const busyRef = useRef(false);

  // 取全量 + 订阅事件。重活按路由开关（见 store 的注释）：别的页面上挂着
  // `listV2vClips` 纯是白烧，那时既没有侧栏那两张卡，也没有列表。
  useEffect(() => {
    let cleanup: (() => void) | undefined;
    void enter()
      .then((fn) => {
        cleanup = fn;
      })
      // 订阅建立失败（事件通道没起来）时这个 promise 会 reject。不接住的话是一条
      // 未处理的 rejection —— 而它唯一的症状是「这一页从此不自己刷新了」，
      // 一句报错都没有。
      .catch((e) =>
        toast.error(`视频流水线没能开始监听事件：${e instanceof Error ? e.message : e}`),
      );
    return () => cleanup?.();
  }, [enter]);

  // 出片刷新跑完了给一句回执 —— 那个按钮点下去要跑几十秒，没有终帧提示就像没点上。
  useEffect(() => {
    let un: (() => void) | undefined;
    void subscribeV2v({
      onRefresh: (e) => {
        if (e.active) return;
        if (e.error) toast.error(`刷新出错：${e.error}`);
        else toast(e.finished > 0 ? `取回 ${e.finished} 条成片` : "已刷新，暂无新出片");
      },
    }).then((f) => {
      un = f;
    });
    return () => un?.();
  }, []);

  const byId = useMemo(() => new Map(visible.map((r) => [r.clip.id, r])), [visible]);
  const curRow = (cur == null ? null : byId.get(cur)) ?? visible[0] ?? null;
  const curId = curRow?.clip.id ?? null;
  const curIndex = curId == null ? 0 : visible.findIndex((r) => r.clip.id === curId) + 1;

  /** 待验收序列（受当前筛选影响）—— 看片流走的就是它。 */
  const revList = useMemo(() => visible.filter((r) => r.stage === "rev"), [visible]);
  const revIndex = curId == null ? -1 : revList.findIndex((r) => r.clip.id === curId);

  // 交接状态跟着**待改写条数**刷新。
  //
  // 它只在 mount 与手动「收录改写」之后取一次的话，watcher 会在无人操作时自己收录
  // —— 于是那个「21 条」在改写落地之后还挂着，恰好在它最要紧的时候是错的。
  // 不挂在每个 tick 上：`v2v_handoff_status` 会顺手重写工单，不是只读的。
  const rewriteN = useV2vStore((s) => s.counts.rewrite);
  // biome-ignore lint/correctness/useExhaustiveDependencies: 依赖的是「待改写条数变了」这个信号
  useEffect(() => {
    void reloadHandoff();
  }, [rewriteN]);

  // 看片流里光标必须落在待验收序列内：否则画面放的是 list[0]、按钮判的是光标那一条，
  // 两者不是同一个片子 —— 而这里每一次按键都在花钱或毙片。
  useEffect(() => {
    if (screen !== "review" || revList.length === 0) return;
    if (revIndex >= 0) return;
    const first = revList[0];
    if (first) setCur(first.clip.id);
  }, [screen, revList, revIndex, setCur]);

  const slice = useMemo(() => sliceSummary(visible, channels), [visible, channels]);
  // 「一组连毙 3 条」按**当前通道的全貌**算，不按这一屏算：毙掉的条目归 `done`，
  // 而 `done` 在工作台里一档都不占 —— 只看这一屏的话这条提示永远不会出现。
  const badGroup = useMemo(() => {
    const pool =
      channel == null
        ? channels.flatMap((c) => c.rows)
        : (channels.find((c) => c.key === channel)?.rows ?? []);
    return worstGroup(pool);
  }, [channels, channel]);

  // ── 动作 ─────────────────────────────────────────────
  const guard = useCallback(async (fn: () => Promise<void>) => {
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    try {
      await fn();
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  }, []);

  /**
   * 判完自动跳下一条 —— 看片流一秒一条，手动再点一次「下一条」等于把节奏折半。
   *
   * 走**哪一条序列**取决于当前在哪个屏：看片流里只该在待验收之间走，跳到一条
   * 「已提交」上会让大播放器当场变成空画面；工作台上则按当前这一屏的顺序走。
   * 判完的那条会离开原序列，故取原序列里它后面的一条（没有就退回前一条）。
   */
  const advance = useCallback(
    (fromId: number) => {
      const list = screen === "review" ? revList : visible;
      const at = list.findIndex((r) => r.clip.id === fromId);
      if (at < 0) return;
      const next = list[at + 1] ?? list[at - 1] ?? null;
      if (next) setCur(next.clip.id);
    },
    [screen, revList, visible, setCur],
  );

  const review = useCallback(
    (ids: number[], pass: boolean, advanceFrom?: number) =>
      guard(async () => {
        if (ids.length === 0) return;
        const res = await unwrap(commands.reviewV2vClips(ids, pass));
        if (res.changed === 0) {
          toast("这些条目不在待验收阶段");
          return;
        }
        setUndo({ label: res.label, entries: res.undo });
        setTally((t) =>
          pass
            ? { ...t, passed: t.passed + res.changed }
            : { ...t, killed: t.killed + res.changed },
        );
        toast(pass ? res.label : `${res.label}（成片进废纸篓）`);
        if (advanceFrom != null) advance(advanceFrom);
        else clearSel();
        await reload();
      }),
    [guard, advance, reload, clearSel],
  );

  const requeue = useCallback(
    (ids: number[], mode: "run" | "rewrite" | "wait", advanceFrom?: number) =>
      guard(async () => {
        if (ids.length === 0) return;
        const res = await unwrap(commands.requeueV2vClips(ids, mode));
        if (res.changed === 0) {
          toast(
            mode === "wait"
              ? "没有可继续等待的条目（只有判了超时且提交单还在的才行）"
              : "这些条目当前阶段不允许该操作",
          );
          return;
        }
        setUndo({ label: res.label, entries: res.undo });
        toast(
          mode === "wait"
            ? `${res.label}（沿用原提交单，不再扣额度）`
            : mode === "run"
              ? `${res.label}（需再次确认提交，会重新扣费）`
              : res.label,
        );
        if (advanceFrom != null) advance(advanceFrom);
        else clearSel();
        await reload();
      }),
    [guard, advance, reload, clearSel],
  );

  /**
   * 重跑的入口 —— 已扣费的在跑条目先弹确认。
   *
   * `requeue_for_run` 会把 `submit_id` 与 `credit_count` 一起清掉，此后 `list_running`
   * 再也认不出那一单：即梦还在跑、钱已经扣了，片子却永远取不回来，下次提交是第二份钱。
   *
   * 判据用 Rust 下发的 `clip.billed`（`Evidence::billed`，读五处证据），不在这里拿
   * `creditCount != null` 凑：前端只看得见其中两个字段，少读一处的后果是对着一条
   * 已经扣过钱的单子说「重跑不花钱」。没扣过费的（幽灵单、被并发上限弹回的）
   * **不弹** —— 它们重跑本来就免费。
   */
  const rerun = useCallback(
    (ids: number[], advanceFrom?: number) => {
      const paid = ids
        .map((id) => byId.get(id))
        .filter((r): r is Row => r != null && r.stage === "run" && r.clip.billed);
      if (paid.length === 0) {
        void requeue(ids, "run", advanceFrom);
        return;
      }
      setConfirmRerun({
        ids,
        paid: paid.length,
        credit: paid.reduce((a, r) => a + (r.clip.creditCount ?? r.clip.submitCredit ?? 0), 0),
        ...(advanceFrom == null ? {} : { advanceFrom }),
      });
    },
    [byId, requeue],
  );

  /** 提交确认：把**即将执行的真实命令行**与这一下要花的额度摆在点确认之前。 */
  const openSubmit = useCallback(
    (ids: number[]) =>
      guard(async () => {
        // 已经放行、正在本地队列里等空位的不算 —— 对它们再点一次确认什么也不会发生，
        // 而确认卡会把它们算进「共 N 条 · 预估 M 额度」，两个数字当场就不准了。
        const ready = ids.filter((id) => {
          const c = byId.get(id);
          return c?.clip.stage === "ready" && c.clip.submitQueuedAt == null;
        });
        if (ready.length === 0) {
          toast("请先选中「待放行」的条目（排队中的已经放行过了，不必再点）");
          return;
        }
        setCmdPreview({ ids: ready, data: await unwrap(commands.previewV2vCommands(ready)) });
      }),
    [guard, byId],
  );

  /**
   * 把一套参数写进指定条目。
   *
   * `refreshPreview` 在提交确认卡里为真 —— 改完参数必须**当场重取命令行与预估额度**，
   * 否则那张卡会继续摆着上一套参数算出来的数字，而人正要照着它按下确认。
   */
  const applyParams = useCallback(
    (ids: number[], p: Params, refreshPreview: boolean) =>
      guard(async () => {
        if (ids.length === 0) return;
        const n = await unwrap(
          commands.setV2vClipParams(
            ids,
            p.modelVersion.trim() === "" ? null : p.modelVersion.trim(),
            p.duration,
            p.videoResolution.trim() === "" ? null : p.videoResolution.trim(),
          ),
        );
        toast.success(n > 0 ? `已改写 ${n} 条的生成参数` : "没有可改参数的条目（已提交的改不动）");
        setEditing(null);
        await reload();
        if (refreshPreview) {
          setCmdPreview({ ids, data: await unwrap(commands.previewV2vCommands(ids)) });
        }
      }),
    [guard, reload],
  );

  const doSubmit = useCallback(() => {
    const p = cmdPreview;
    if (!p) return;
    void guard(async () => {
      const sum = await unwrap(commands.submitV2vClips(p.ids));
      if (sum.submitted > 0) {
        toast.success(
          sum.queued > 0
            ? `已提交 ${sum.submitted} 条到即梦 · 另 ${sum.queued} 条在本地排队，出一条自动补一条`
            : `已提交 ${sum.submitted} 条到即梦`,
        );
      } else if (sum.queued > 0) {
        // 即梦已经跑满时一条都发不出去 —— 这不是失败，但必须说清楚，
        // 否则点了确认什么都没发生。
        toast(`即梦已跑满，${sum.queued} 条已排进本地队列，有空位就自动发`);
      }
      if (sum.failed > 0) toast.error(`${sum.failed} 条提交失败：${sum.firstError ?? ""}`);
      setCmdPreview(null);
      clearSel();
      await reload();
    });
  }, [cmdPreview, guard, reload, clearSel]);

  /** 撤回放行：本地队列 → 等你点确认提交。没发出去所以不涉及钱。 */
  const unqueue = useCallback(
    (ids: number[]) =>
      guard(async () => {
        const n = await unwrap(commands.unqueueV2vClips(ids));
        toast(n > 0 ? `已撤回放行 ${n} 条（未产生额度消耗）` : "这些条目已经发出去了，撤不回来");
        clearSel();
        await reload();
      }),
    [guard, reload, clearSel],
  );

  /** 换通道。免费的那些与要丢弃提交单的那些，由面板上的复选框分开。 */
  const switchChannel = useCallback(
    (ids: number[], p: ChannelParams, abandon: boolean) =>
      guard(async () => {
        const res = await unwrap(
          commands.switchV2vChannel(ids, p.modelVersion, p.duration, p.videoResolution, abandon),
        );
        setSwitching(null);
        if (res.switched + res.abandoned === 0) {
          toast("这些条目当前阶段不允许换通道");
          return;
        }
        // 撤销令牌只含被丢弃的那些（纯参数改动撤不回来，换回去本来就免费）——
        // 所以没有丢弃时不摆那个「撤销」pill，免得人以为它能把通道换回去。
        if (res.undo.length > 0) setUndo({ label: res.label, entries: res.undo });
        toast(
          res.abandoned > 0
            ? `${res.label}（丢弃 ${res.abandoned} 条已提交单，其中已扣 ${res.abandonedCredit} 额度）`
            : `${res.label}（还没提交出去，未产生额度消耗）`,
        );
        clearSel();
        await reload();
      }),
    [guard, reload, clearSel],
  );

  const doUndo = useCallback(() => {
    const u = undo;
    if (!u || u.entries.length === 0) return;
    void guard(async () => {
      const n = await unwrap(commands.undoV2v(u.entries));
      setUndo(null);
      setTally({ passed: 0, killed: 0 });
      toast(n > 0 ? `已撤销 ${n} 条` : "已无法撤销（这些条目之后又被改动过）");
      await reload();
    });
  }, [undo, guard, reload]);

  const ingest = useCallback(
    () =>
      guard(async () => {
        const sum = await unwrap(commands.ingestV2vRewrites());
        if (sum.applied > 0) toast.success(`已收录 ${sum.applied} 条改写结果`);
        else if (sum.unmatched > 0 || sum.stale > 0)
          toast(`未收录：认不出 ${sum.unmatched} 条、已越过待提交 ${sum.stale} 条`);
        else toast("交接目录里没有新的改写结果");
        await reloadHandoff();
        await reload();
      }),
    [guard, reload, reloadHandoff],
  );

  /**
   * 立刻问一遍即梦。
   *
   * **不走 `guard`**：那把锁是给「会改状态、不能连点」的动作用的，而刷新要跑几十秒，
   * 用它锁住整页等于刷新期间什么都干不了。命令本身立刻返回（活儿在 Rust 后台），
   * 重入由 Rust 侧的 `REFRESHING` 闸挡。
   */
  const pollNow = useCallback(() => {
    void unwrap(commands.pollV2vNow())
      .then((n) => {
        if (n === 0) toast("即梦手上没有在跑的条目 —— 本地队列里那些它还不知道");
      })
      .catch((e) => {
        if (e instanceof Error) toast.error(e.message);
      });
  }, []);

  const openHandoff = useCallback(() => {
    void unwrap(commands.openHandoffDir()).catch((e) => toast.error(String(e)));
  }, []);

  // ── 看片流的单条判定（键盘与按钮共用） ────────────────
  const judgeCurrent = useCallback(
    (kind: "pass" | "rej" | "rerun" | "rewrite" | "wait") => {
      const r = curRow;
      if (!r) return;
      const id = r.clip.id;
      if (kind === "pass") {
        if (r.stage !== "rev") return;
        void review([id], true, id);
      } else if (kind === "rej") {
        if (r.stage !== "rev") return;
        void review([id], false, id);
      } else if (kind === "rerun") {
        // `R` 键与按钮走同一条路 —— 护栏若只挂在按钮上，最快的那个入口就是敞开的。
        rerun([id], id);
      } else if (kind === "rewrite") {
        void requeue([id], "rewrite", id);
      } else {
        void requeue([id], "wait", id);
      }
    },
    [curRow, review, requeue, rerun],
  );

  const move = useCallback(
    (d: 1 | -1) => {
      if (visible.length === 0) return;
      const at = curId == null ? -1 : visible.findIndex((r) => r.clip.id === curId);
      const next = visible[Math.max(0, Math.min(visible.length - 1, (at < 0 ? 0 : at) + d))];
      if (next) setCur(next.clip.id);
    },
    [visible, curId, setCur],
  );

  const modalOpen =
    cmdPreview != null ||
    showLog ||
    showParams ||
    showObserve ||
    editing != null ||
    switching != null;

  // ── 键盘 ─────────────────────────────────────────────
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable)) return;
      // 命令面板 / 速查面板打开时整页让路：不然在 ⌘K 里打字会顺手判掉一条视频。
      const ui = useUiStore.getState();
      if (ui.paletteOpen || ui.helpOpen) return;
      if (e.metaKey || e.ctrlKey) {
        // ⌘⏎ 确认提交：勾选的待提交条目，或（没勾时）当前光标那一条。
        if (e.key === "Enter") {
          e.preventDefault();
          const ids = sel.size > 0 ? [...sel] : curId != null ? [curId] : [];
          void openSubmit(ids);
        }
        return;
      }
      // ⌥1/2/3 与 ⌥\ 用 e.code：macOS 上 Alt 会把 key 改写成 ¡ / « 之类的符号。
      if (e.altKey) {
        if (e.code === "Digit1") {
          e.preventDefault();
          setShowObserve(true);
        } else if (e.code === "Digit2") {
          e.preventDefault();
          setShowLog(true);
        } else if (e.code === "Digit3") {
          e.preventDefault();
          setShowParams(true);
        } else if (e.code === "Backslash") {
          e.preventDefault();
          toggleLedger();
        }
        return;
      }
      if (modalOpen || confirmRerun != null) return;

      if (e.key === "Escape") {
        if (screen === "review") {
          e.preventDefault();
          setScreen("list");
        }
        return;
      }
      if (e.key === "Enter") {
        e.preventDefault();
        // 全屏看片只对**待验收**成立：别的阶段要么没有成片，要么已经定案，
        // 进去会得到一块空画面加一排点不动的按钮。
        if (screen === "list" && curRow?.stage === "rev") {
          setTally({ passed: 0, killed: 0 });
          setScreen("review");
        }
        return;
      }
      // 四个方向键**全部**是「换一条」。看片流底部那条胶片条是横向的，于是 ←/→ 在
      // 那里最顺手；工作台的列表是纵向的，↑/↓ 更顺手。逐帧仍留在播放条的按钮上：
      // 判形变是停下来慢慢看的事，与「一秒一条地过片」不该抢同一组键。
      if (e.key === "j" || e.key === "J" || e.key === "ArrowDown" || e.key === "ArrowRight") {
        e.preventDefault();
        move(1);
        return;
      }
      if (e.key === "k" || e.key === "K" || e.key === "ArrowUp" || e.key === "ArrowLeft") {
        e.preventDefault();
        move(-1);
        return;
      }
      if (e.key === " ") {
        e.preventDefault();
        judgeCurrent("pass");
        return;
      }
      if (e.key === "x" || e.key === "X") {
        judgeCurrent("rej");
        return;
      }
      if (e.key === "r" || e.key === "R") {
        judgeCurrent("rerun");
        return;
      }
      if (e.key === "e" || e.key === "E") {
        judgeCurrent("rewrite");
        return;
      }
      if (e.key === "w" || e.key === "W") {
        judgeCurrent("wait");
        return;
      }
      if (e.key === "u" || e.key === "U") {
        doUndo();
        return;
      }
      if (e.key === "f" || e.key === "F") {
        setShowFrame((v) => !v);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [
    screen,
    modalOpen,
    confirmRerun,
    move,
    judgeCurrent,
    doUndo,
    curRow,
    curId,
    sel,
    openSubmit,
    toggleLedger,
  ]);

  const handlers: DockHandlers = {
    onSubmit: (ids) => void openSubmit(ids),
    onReview: (id, pass) => void review([id], pass, id),
    onRerun: (ids) => rerun(ids, ids.length === 1 ? ids[0] : undefined),
    onRequeueRewrite: (ids) => void requeue(ids, "rewrite", ids.length === 1 ? ids[0] : undefined),
    onResume: (ids) => void requeue(ids, "wait", ids.length === 1 ? ids[0] : undefined),
    onUnqueue: (ids) => void unqueue(ids),
    onEnterReview: () => {
      setTally({ passed: 0, killed: 0 });
      setScreen("review");
    },
    onIngest: () => void ingest(),
    onOpenHandoff: openHandoff,
    onPollNow: pollNow,
    onSwitchChannel: (ids) =>
      setSwitching(ids.map((id) => byId.get(id)).filter((r): r is Row => r != null)),
    onEditParams: (ids) =>
      setEditing(ids.map((id) => byId.get(id)).filter((r): r is Row => r != null)),
    onUndo: doUndo,
  };

  const panels = (
    <>
      {showLog && <V2vLogPanel onClose={() => setShowLog(false)} />}
      {showParams && (
        <V2vParamsPanel
          models={models}
          queue={queue}
          onClose={() => {
            setShowParams(false);
            void reloadEff();
            void reload();
          }}
        />
      )}
      {showObserve && (
        <ObserveModal
          tick={tick}
          now={coarseNow}
          credit={credit}
          onClose={() => setShowObserve(false)}
        />
      )}
    </>
  );

  if (clips.length === 0) {
    return (
      <div className="col f1 ohide">
        <div className="bigempty">
          <Clapperboard className="ic" style={{ width: 26, height: 26, opacity: 0.5 }} />
          <div className="fs13 fw5 t2">流水线是空的</div>
          <div className="fs12 t3" style={{ maxWidth: 460, lineHeight: 1.7 }}>
            给提示词组标上用途「图生视频」（导入 txt 时就能选），该组的图
            <b>验收通过即自动入队</b>，不需要回作品库找出来再点导出。
          </div>
        </div>
        {panels}
      </div>
    );
  }

  return (
    <div className={cn("vwb", !ledgerOpen && "noled")}>
      <div className="vwbrow">
        <V2vPreview
          row={curRow}
          index={curIndex}
          total={visible.length}
          showFrame={showFrame}
          onToggleFrame={setShowFrame}
        />
        {ledgerOpen && (
          <V2vLedger
            row={curRow}
            slice={slice}
            channels={channels}
            action={action}
            channel={channel}
            handoff={handoff}
            rewriteTotal={rewriteN}
            activity={activity}
            now={coarseNow}
            badGroup={badGroup}
            busy={busy}
            onRewriteGroup={(ids) => void requeue(ids, "rewrite")}
            onObserve={() => setShowObserve(true)}
            onLog={() => setShowLog(true)}
            onParams={() => setShowParams(true)}
            onOpenHandoff={openHandoff}
          />
        )}
        <V2vList
          rows={visible}
          channels={channels}
          action={action}
          curId={curId}
          sel={sel}
          sort={sort}
          onSort={() => {
            const ks = Object.keys(SORTS) as SortKey[];
            setSort(ks[(ks.indexOf(sort) + 1) % ks.length] ?? "wait");
          }}
          onPick={setCur}
          onCheck={(id) =>
            setSel((old) => {
              const n = new Set(old);
              if (n.has(id)) n.delete(id);
              else n.add(id);
              return n;
            })
          }
          onToggleAll={() =>
            setSel((old) => {
              const allIn = visible.length > 0 && visible.every((r) => old.has(r.clip.id));
              if (allIn) return new Set();
              return new Set(visible.map((r) => r.clip.id));
            })
          }
        />
      </div>

      <V2vDock
        row={curRow}
        action={action}
        visible={visible}
        sel={sel}
        running={tick?.running ?? 0}
        busy={busy}
        undoLabel={undo == null ? null : undo.label}
        h={handlers}
      />

      {screen === "review" && (
        <V2vReviewFlow
          list={revList}
          index={revIndex < 0 ? 0 : revIndex}
          passedCount={tally.passed}
          killedCount={tally.killed}
          undoLabel={undo?.label ?? null}
          busy={busy}
          onSeek={setCur}
          onPass={() => judgeCurrent("pass")}
          onReject={() => judgeCurrent("rej")}
          onRerun={() => judgeCurrent("rerun")}
          onRewrite={() => judgeCurrent("rewrite")}
          onUndo={doUndo}
          onExit={() => setScreen("list")}
        />
      )}

      {cmdPreview && (
        <SubmitConfirm
          preview={cmdPreview.data}
          ids={cmdPreview.ids}
          models={models}
          busy={busy}
          onApplyParams={(p) => applyParams(cmdPreview.ids, p, true)}
          onClose={() => setCmdPreview(null)}
          onConfirm={doSubmit}
        />
      )}

      {editing && editing.length > 0 && (
        <ParamEditModal
          rows={editing}
          models={models}
          busy={busy}
          onClose={() => setEditing(null)}
          onApply={(p) =>
            void applyParams(
              editing.map((r) => r.clip.id),
              p,
              false,
            )
          }
        />
      )}

      {switching && switching.length > 0 && (
        <ChannelSwitchModal
          rows={switching}
          models={models}
          autofillOn={autofill?.enabled === true}
          busy={busy}
          onClose={() => setSwitching(null)}
          // 「改投并去提交」在这里串，而不是塞进 `switchChannel` 里 ——
          // 它要调 `openSubmit`，而那个 hook 定义在 `switchChannel` 之后（依赖 `byId`）。
          // 接的是**提交确认卡**而不是直接放行：提交即扣费且不可撤回，而这张卡通篇在说
          // 「换通道免费」，在它后面直接放行等于让人在一张写着「不花钱」的卡上花掉钱。
          onConfirm={(p, abandon, andSubmit) => {
            const ids = switching.map((r) => r.clip.id);
            const holding = switching.filter((r) => r.action === "submit").map((r) => r.clip.id);
            void switchChannel(ids, p, abandon).then(() => {
              if (andSubmit && holding.length > 0) void openSubmit(holding);
            });
          }}
        />
      )}

      {confirmRerun && (
        <ConfirmModal
          title={`重跑 ${confirmRerun.ids.length} 条 · 其中 ${confirmRerun.paid} 条已经花过钱`}
          desc={`这 ${confirmRerun.paid} 条即梦已经收下并扣了 ${confirmRerun.credit} 额度，且不可撤回。重跑会丢弃原提交单 —— 那几条视频即梦还在跑，但我们此后再也取不回来，下次确认提交是第二份钱。想换条队而不是重抽的话，用「换通道」。`}
          confirmLabel="仍要重跑"
          danger
          onConfirm={() => {
            const c = confirmRerun;
            setConfirmRerun(null);
            void requeue(c.ids, "run", c.advanceFrom);
          }}
          onClose={() => setConfirmRerun(null)}
        />
      )}

      {panels}
    </div>
  );
}

/** 选中条目的预估额度合计；有一条查不到单价就返回 null（不给半真半假的数）。 */
function estimateOf(rows: Row[]): number | null {
  let sum = 0;
  for (const r of rows) {
    if (r.estimate == null) return null;
    sum += r.estimate;
  }
  return sum;
}

/**
 * 「改参数」——把一套模型 / 时长 / 分辨率写进选中的条目。
 *
 * 只吃 `rewrite` / `ready` 两阶段（镜像 `repo::set_params` 的 `WHERE`）：已提交的改了
 * 不会重新生效，那条视频用的是提交那一刻的参数，而界面上却会显示新的 —— 两者对不上
 * 之后，「我明明设了 1080p」这类怀疑就再也说不清了。
 *
 * 初值取选中项**自己写死**的覆写（不是逐级回落后的结果）：`set_v2v_clip_params` 的
 * `None` 是**清空**不是**保持**，一律以空值开场的话，选中一批已经设过 vip/1080p 的
 * 条目、只想改个时长，按下「应用」就会把模型和分辨率一起抹掉。
 */
function ParamEditModal({
  rows,
  models,
  busy,
  onClose,
  onApply,
}: {
  rows: Row[];
  models: ModelInfo[];
  busy: boolean;
  onClose: () => void;
  onApply: (p: Params) => void;
}) {
  const key = (r: Row) =>
    `${r.clip.modelVersion ?? ""}|${r.clip.duration ?? ""}|${r.clip.videoResolution ?? ""}`;
  const first = rows[0];
  const uniform = first != null && rows.every((r) => key(r) === key(first));
  const [p, setP] = useState<Params>(
    uniform && first
      ? {
          modelVersion: first.clip.modelVersion ?? "",
          duration: first.clip.duration,
          videoResolution: first.clip.videoResolution ?? "",
        }
      : { modelVersion: "", duration: null, videoResolution: "" },
  );

  return (
    <Modal
      title={`改参数 · ${rows.length} 条`}
      width="w700"
      onClose={onClose}
      footer={
        <>
          <span className="fs11 t3">
            提交之后再改不会重新生效 —— 那条视频用的是提交那一刻的参数。
          </span>
          <div className="f1" />
          <button type="button" className="btn sm gho" onClick={onClose}>
            取消
          </button>
          <button type="button" className="btn sm pri" disabled={busy} onClick={() => onApply(p)}>
            应用到这 {rows.length} 条
          </button>
        </>
      }
    >
      <div style={{ padding: 4 }}>
        <div className="parambar mb8">
          <span className="fs11 fw6 t3 nowrap">改成</span>
          <V2vParamPicker models={models} value={p} onChange={setP} disabled={busy} />
        </div>
        {!uniform && (
          <div className="fs11 t3 mb8" style={{ lineHeight: 1.8 }}>
            选中的条目原参数不一致 —— 上面填的这一套会**整体覆盖**它们，不是逐条保持。
            留空的那一项表示「跟随全局默认」，同样会覆盖掉原来写死的值。
          </div>
        )}
        <div className="costbar">
          <div className="fs12">
            这 {rows.length} 条都还没提交出去，所以改参数**不涉及任何额度**。
          </div>
          <div className="fs11 t3">
            模型留空 = 跟随设置里的全局默认（也就是常驻队列捡得走的那一档）；
            写死型号之后这些条目就不再被自动补单捡走了。
          </div>
        </div>
      </div>
    </Modal>
  );
}

/**
 * 提交确认卡：真实命令行 + 这一下要花多少额度 + **就地改参数**，全摆在按钮之前。
 *
 * 参数编辑放在这里而不是设置页，是因为「提交前」正是唯一一个人一定会看的时刻，
 * 也是唯一一个改了还来得及的时刻 —— 提交即扣费，之后再改只影响下一次。
 * 改完当场重算这一下要花多少：模型之间差 5.5 倍，那个数字必须随选择一起变，
 * 否则「改了参数」与「这一下花多少钱」还是两件对不上的事。
 *
 * ## 这张卡上的两处「别让人干等」
 *
 * 1. **余额自己去取**（`v2vBalance`）。它原来长在 `previewV2vCommands` 里，于是点
 *    「提交」之后整个界面要等一次 CLI 网络往返才看得到这张卡 —— 期间一声不吭，
 *    人以为没点上。现在卡先出来，余额那一格显示「读取中…」。
 * 2. **提交过程有进度**。后端本来就在逐条记 `submit` 阶段的执行日志（「提交中 i/total」），
 *    但那些字只有开执行日志才看得到，而这张卡此刻正挡着它。所以直接订阅同一条流，
 *    把最新一句摆在按钮旁边 —— 不新增事件，也就不会与日志分叉。
 */
function SubmitConfirm({
  preview,
  ids,
  models,
  busy,
  onApplyParams,
  onClose,
  onConfirm,
}: {
  preview: SubmitPreview;
  ids: number[];
  models: ModelInfo[];
  busy: boolean;
  /** 把参数写进这些条目并重取预览。 */
  onApplyParams: (p: Params) => void;
  onClose: () => void;
  onConfirm: () => void;
}) {
  const short = preview.estimatedCredits;
  /** `undefined` = 还在读；`null` = 读不到（掉线/未登录，不拦人）。 */
  const [balance, setBalance] = useState<number | null | undefined>(undefined);
  useEffect(() => {
    let alive = true;
    void unwrap(commands.v2vBalance())
      .then((b) => alive && setBalance(b))
      .catch(() => alive && setBalance(null));
    return () => {
      alive = false;
    };
  }, []);

  // 提交进度：订阅执行日志里 `submit` 阶段的那条流。
  const [step, setStep] = useState<string | null>(null);
  useEffect(() => {
    if (!busy) return;
    let un: (() => void) | undefined;
    void subscribeV2v({
      onActivity: (e) => {
        if (e.entry.phase === "submit") setStep(e.entry.message);
      },
    }).then((f) => {
      un = f;
    });
    return () => un?.();
  }, [busy]);

  // 秒表：这里要回答的是「它还在动吗」，而后端两条日志之间可能隔着一整次网络往返。
  const [elapsed, setElapsed] = useState(0);
  useEffect(() => {
    if (!busy) {
      setElapsed(0);
      return;
    }
    const t = setInterval(() => setElapsed((s) => s + 1), 1000);
    return () => clearInterval(t);
  }, [busy]);
  // 即梦同时只跑得下这么多条，其余留在本地排队自动接上。**这个数必须出现在按下确认
  // 之前**：这一版之前，选 9 条点确认得到的是「已提交 9 条」，而实际只有 1 条入队、
  // 8 条被即梦以 ExceedConcurrencyLimit 弹回来判死 —— 界面从头到尾没提过有这个上限。
  //
  // 由 Rust 逐通道算好（`SubmitPreview.lanes`）：空位是按通道的，前端拿一个全局上限减
  // 一个全局在跑数，会在一批横跨两条通道时给出一个谁也不对的数（0031）。
  const goesNow = preview.lanes.reduce((a, l) => a + l.goesNow, 0);
  const waits = preview.commands.length - goesNow;
  const [p, setP] = useState<Params>({ modelVersion: "", duration: null, videoResolution: "" });
  const [dirty, setDirty] = useState(false);
  return (
    <Modal
      title="确认提交到即梦"
      width="w700"
      onClose={onClose}
      headerExtra={<span className="chip">{preview.commands.length} 条</span>}
      footer={
        <>
          <span className="fs12" style={{ fontWeight: 600 }}>
            预计消耗 {preview.unpriced.length > 0 ? "≥ " : ""}
            {short} 额度
          </span>
          <span className="fs11 t3">
            {waits > 0 ? `先发 ${goesNow} 条 · 其余排队时才扣费` : "提交即扣费，无法撤回"}
          </span>
          <div className="f1" />
          {/* 提交进行中：这一行是「它还在动吗」的全部答案。取消按钮此刻让位给它 ——
              发出去的那一条撤不回来，一个点了没用的「取消」只会让人更慌。 */}
          {busy && (
            <span className="subprog">
              <RefreshCw className="ic12 spin" />
              <span className="ohide nowrap">{step ?? "正在提交…"}</span>
              <span className="mono nowrap">{elapsed}s</span>
            </span>
          )}
          {!busy && (
            <button type="button" className="btn sm gho" onClick={onClose}>
              取消
            </button>
          )}
          <button
            type="button"
            className="btn sm pri"
            disabled={busy || dirty}
            title={dirty ? "参数改了还没应用 —— 先点「应用到这 N 条」" : undefined}
            onClick={onConfirm}
          >
            <Send className="ic12" />
            {busy ? "提交中…" : "确认提交"}
          </button>
        </>
      }
    >
      <div style={{ padding: 4 }}>
        {/* 参数条放在最上面：它是这张卡里唯一还能改的东西，而下面那串命令行是它的结果。 */}
        <div className="parambar mb8">
          <span className="fs11 fw6 t3 nowrap">这些条目的参数</span>
          <V2vParamPicker
            models={models}
            value={p}
            disabled={busy}
            onChange={(v) => {
              setP(v);
              setDirty(true);
            }}
          />
          <div className="f1" />
          <button
            type="button"
            className={cn("btn xs", dirty && "pri")}
            disabled={busy || !dirty}
            onClick={() => {
              onApplyParams(p);
              setDirty(false);
            }}
          >
            应用到这 {ids.length} 条
          </button>
        </div>
        <div className="costbar mb8">
          <div className="fs12">
            <b>{preview.commands.length}</b> 条 · 预计消耗{" "}
            <b>
              {preview.unpriced.length > 0 ? "≥ " : ""}
              {short}
            </b>{" "}
            额度
            {balance === undefined && <span className="t3">｜余额读取中…</span>}
            {balance !== undefined && balance !== null && (
              <>
                ｜余额 <b>{balance}</b> → 提交后约 <b>{balance - short}</b>
              </>
            )}
          </div>
          {preview.unpriced.length > 0 && (
            <div className="twarn">
              {preview.unpriced.join("、")} 没实测过单价，未计入 —— 实际只会更高。
            </div>
          )}
          {balance != null && balance < short && (
            <div className="terr">
              余额不足：即梦逐条扣费，会提交到一半开始报错，而前面扣掉的退不回来。
            </div>
          )}
        </div>
        <div className="costbar mb8">
          <div className="fs12">
            即梦<b>按模型通道各排各的队</b>，每条通道各有一个在跑上限，所以现在{" "}
            <b>先发 {goesNow} 条</b>
            {waits > 0 && (
              <>
                ，其余 <b>{waits} 条排在本地</b>，出一条自动补一条 —— 不必再来点一次
              </>
            )}
            。
          </div>
          {/* 逐通道分账。这一次提交横跨两条通道时，一个合计数答不出「卡的是哪条」——
              而那正是「我这 6 条 mini 为什么不走」当初没人答得上来的原因。 */}
          <div className="fs11 t3" style={{ lineHeight: 1.8 }}>
            {preview.lanes.map((l) => (
              <span key={l.label} style={{ marginRight: 12 }}>
                <b className="t1">{l.label}</b> 共 {l.total} 条 · 先发 {l.goesNow} 条（该通道上限{" "}
                {l.limit}）
              </span>
            ))}
          </div>
          <div className="fs11 t3">
            排队的那些<b>还没扣费</b>：额度是在真正发出去的那一刻扣的，所以下面那个预估是
            这些条目全部跑完的总数，不是现在就要花掉的。
          </div>
        </div>
        <div className="fs12 t2 mb8" style={{ lineHeight: 1.7 }}>
          下面是<b>即将执行的完整命令行</b>（与真正 exec 的参数同源）。
          「我设了却没生效」这类怀疑只能靠把真实请求摆在确认之前来消除。
        </div>
        <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
          {preview.commands.map((line, i) => (
            <div key={`${i}-${line.slice(0, 24)}`} className="cmdwell">
              {line}
            </div>
          ))}
        </div>
      </div>
    </Modal>
  );
}

/**
 * 换通道面板 —— 把选中的条目改投到另一条即梦队列。
 *
 * ## 这张卡存在的全部理由：两种「排队中」的代价差着一整份额度
 *
 * 一条通道同时只有上限那么几条真在即梦手上（2.0fast 非 VIP 实测 = 1），**其余同通道的
 * 全压在本地**（`stage='ready' AND submit_queued_at IS NOT NULL`）—— 即梦对它们一无所知，
 * `submit_id` 为空、一分钱没扣。所以「排在慢通道上的那 78 条」换通道是**免费**的。
 *
 * 而人此前找得到的唯一按钮是「重跑」，那个按钮对已提交的条目会丢弃一份已付费的任务。
 * 一个免费、一个是第二份钱，长得却一模一样 —— 这张卡的职责就是把这两件事**分开说**，
 * 并且在按钮之前把数字摆出来。
 *
 * 已提交的那些默认**不动**：那个复选框不勾，它们就原样留在原通道上继续跑。
 */
function ChannelSwitchModal({
  rows,
  models,
  autofillOn,
  busy,
  onClose,
  onConfirm,
}: {
  rows: Row[];
  models: ModelInfo[];
  /** 常驻队列开着没有 —— 决定要不要提示「这一换就退出候选池」。 */
  autofillOn: boolean;
  busy: boolean;
  onClose: () => void;
  onConfirm: (p: ChannelParams, abandon: boolean, andSubmit: boolean) => void;
}) {
  // ── 按「换完之后会怎样」分堆 ────────────────────────────
  //
  // 这张卡原来只按「花不花钱」分，于是它答得出「换通道免费」，却答不出人真正在问的
  // 那句：**换完它就自己跑起来了吗**。而答案逐堆完全不同 —— 已放行的换完自动接着排、
  // 待放行的还要点确认提交、待改写的连提示词都还没有，怎么换都发不出去。
  //
  // 少了这一层，一个人选中 6 条待改写换了通道，会得到一个「已改投 6 条」的成功提示，
  // 然后盯着一个永远不会自己动的列表。
  const queued = rows.filter((r) => r.action === "queued"); // 已放行，在本地排队
  const holding = rows.filter((r) => r.action === "submit"); // 待放行，等人点确认
  const noPrompt = rows.filter((r) => r.stage === "rewrite"); // 还没有视频提示词
  const free = rows.filter((r) => r.stage === "ready" || r.stage === "rewrite");
  const live = rows.filter((r) => r.stage === "run");
  const paid = live.filter((r) => r.clip.billed);
  const locked = rows.length - free.length - live.length;
  const paidCredit = paid.reduce((a, r) => a + (r.clip.creditCount ?? r.clip.submitCredit ?? 0), 0);
  // 换通道 = 把型号写死，而 `AUTOFILL_POOL` 只捡型号为空的（「指定过参数的不给补单器
  // 捡走」）。于是一个叫「换通道」的动作会顺带把这些条目**永久移出常驻队列的候选池**
  // —— 补单器开着、水位线又快见底时，这是个查半天查不出原因的静默后果。
  const leavingPool = rows.filter((r) => (r.clip.modelVersion ?? "").trim() === "").length;

  const [abandon, setAbandon] = useState(false);
  // 带过去的原值取第一条 —— 混选时下面会标「多条不一致」，而一个「保持不变」的选项在
  // 这里不存在：即梦只接受一套完整组合。
  const first = rows[0];
  const wantDur = first?.duration ?? null;
  const wantRes = first?.resolution ?? "";
  const mixed = rows.some((r) => r.duration !== wantDur || r.resolution !== wantRes);

  const [p, setP] = useState<Params>({ modelVersion: "", duration: null, videoResolution: "" });
  const target = models.find((m) => m.modelVersion === p.modelVersion);
  const carried = carryParams(target, wantDur, wantRes);

  const willMove = free.length + (abandon ? live.length : 0);
  const perSec = creditPerSec(models, p.modelVersion, p.videoResolution);
  const after = perSec != null && p.duration != null ? perSec * p.duration * willMove : null;
  const before = estimateOf(rows.filter((r) => free.includes(r) || (abandon && live.includes(r))));

  return (
    <Modal
      title={`换通道 · ${rows.length} 条`}
      width="w700"
      onClose={onClose}
      footer={
        <>
          <span className="fs11 t3">
            {willMove === 0
              ? "当前选择下没有可改投的条目"
              : `将改投 ${willMove} 条${abandon && paid.length > 0 ? `，其中 ${paid.length} 条要丢弃已付费的提交单` : ""}`}
          </span>
          <div className="f1" />
          <button type="button" className="btn sm gho" onClick={onClose}>
            取消
          </button>
          <button
            type="button"
            className={cn("btn sm", abandon && paid.length > 0 ? "dngo" : "pri")}
            disabled={busy || target == null || willMove === 0}
            onClick={() => {
              if (!target || p.duration == null) return;
              onConfirm(
                {
                  modelVersion: p.modelVersion,
                  duration: p.duration,
                  videoResolution: p.videoResolution,
                },
                abandon,
                false,
              );
            }}
          >
            改投 {willMove} 条
          </button>
          {/* 「换完就发」——人换通道多半就是这个意思。但它接的是**提交确认卡**而不是
              直接放行：提交即扣费且不可撤回，而这张卡通篇在说「换通道免费」，
              在它上面挂一个会安静扣钱的按钮是最坏的一种连贯。
              只在确实有东西可发时出现（待放行、且已经有视频提示词）。 */}
          {holding.length > 0 && (
            <button
              type="button"
              className="btn sm pri"
              disabled={busy || target == null}
              onClick={() => {
                if (!target || p.duration == null) return;
                onConfirm(
                  {
                    modelVersion: p.modelVersion,
                    duration: p.duration,
                    videoResolution: p.videoResolution,
                  },
                  abandon,
                  true,
                );
              }}
            >
              改投并去提交 {holding.length} 条
            </button>
          )}
        </>
      }
    >
      <div style={{ padding: 4 }}>
        <div className="parambar mb8">
          <span className="fs11 fw6 t3 nowrap">改投到</span>
          <V2vParamPicker
            models={models}
            value={p}
            disabled={busy}
            onChange={(next) => {
              // 换模型时**把原值带过去**，而不是像别处那样清空 —— 这张卡的语义是
              // 「换条队」，不是「重设参数」。夹过的会在下面报出来。
              if (next.modelVersion !== p.modelVersion) {
                const c = carryParams(
                  models.find((m) => m.modelVersion === next.modelVersion),
                  wantDur,
                  wantRes,
                );
                setP({
                  modelVersion: next.modelVersion,
                  duration: c.duration,
                  videoResolution: c.resolution,
                });
              } else {
                setP(next);
              }
            }}
          />
        </div>

        {target && (carried.durationChanged || carried.resolutionChanged) && (
          <div className="fs11 wr2 mb8" style={{ lineHeight: 1.8 }}>
            这条通道接不了原来的规格，已经夹到最近的合法值：
            {carried.durationChanged && ` 时长 ${wantDur}s → ${carried.duration}s`}
            {carried.resolutionChanged && ` 分辨率 ${wantRes} → ${carried.resolution}`}
            。上面还能再改。
          </div>
        )}
        {mixed && (
          <div className="fs11 t3 mb8">
            选中的条目原参数不一致 —— 上面填的这一套会**整体覆盖**它们，不是逐条保持。
          </div>
        )}

        {/* 「换完之后会怎样」——这是人点这个按钮时真正在问的问题，而它逐堆答案完全不同。
            放在花钱那一段**之前**：先答「它会不会跑起来」，再答「要花多少」。 */}
        <div className="costbar mb8">
          <div className="fs11 fw6 t3">换完之后</div>
          {queued.length > 0 && (
            <div className="fs12">
              <b>{queued.length} 条已放行的 → 直接排到新通道上，什么都不用再点</b>
              ，出空位自动发。它们带着原来的放行时刻插队，不会被罚到队尾。
            </div>
          )}
          {holding.length > 0 && (
            <div className="fs12">
              <b>{holding.length} 条待放行的 → 仍然停在「等你点确认提交」</b>
              。用下面那个「改投并去提交」可以换完直接进确认卡（那一步才扣费）。
            </div>
          )}
          {noPrompt.length > 0 && (
            <div className="fs12 wr2" style={{ lineHeight: 1.8 }}>
              <b>{noPrompt.length} 条待改写的 → 换完还是发不出去</b>
              ：它们连视频提示词都还没有，而即梦要的就是提示词。得先去 Claude Code / Codex 里跑
              v2v-rewrite 把提示词写回来，那之后才谈得上提交。 换通道这一步对它们仍然有效 ——
              只是生效要等到提交那一刻。
            </div>
          )}
        </div>

        <div className="costbar mb8">
          {free.length > 0 && (
            <div className="fs12">
              <b>{free.length} 条还在本地队列 / 待放行 / 待改写</b> —— 即梦对它们一无所知，
              <b>一分钱没扣</b>，换通道免费。
            </div>
          )}
          {live.length > 0 && (
            <div className={cn("fs12", paid.length > 0 && "terr")} style={{ lineHeight: 1.8 }}>
              <b>{live.length} 条已经提交给即梦</b>
              {paid.length > 0 ? (
                <>
                  ，其中 {paid.length} 条<b>确实扣了 {paidCredit} 额度且不可撤回</b>
                  。改投等于丢弃它们 —— 那几条视频即梦还在跑，但我们此后再也取不回来， 新通道上要
                  <b>再花一份钱</b>。
                </>
              ) : (
                <>，但一处计费证据都没有（幽灵单 / 被并发上限弹回的），改投不花钱。</>
              )}
              <label className="fx ac gap6 mt5" style={{ cursor: "pointer" }}>
                <input
                  type="checkbox"
                  checked={abandon}
                  disabled={busy}
                  onChange={(e) => setAbandon(e.target.checked)}
                />
                <span className="fs11">连这 {live.length} 条一起改投</span>
              </label>
            </div>
          )}
          {locked > 0 && (
            <div className="fs11 t3">
              另有 {locked} 条已出片或已定案，换通道对它们没有意义，会跳过。
            </div>
          )}
          {autofillOn && leavingPool > 0 && (
            <div className="fs11 wr2" style={{ lineHeight: 1.8 }}>
              其中 {leavingPool} 条现在跟随全局默认通道，换过去等于**给它们写死型号** ——
              常驻队列只捡没写死型号的，所以这 {leavingPool} 条从此不会再被自动补单捡走。
              要放回去：底坞「改参数」里把模型选回「跟随全局默认」。
            </div>
          )}
          {target && (
            <div className="fs11 t3">
              额度预估：{before == null ? "?" : before} → <b className="t1">{after ?? "?"}</b>
              {target.vip && " · vip 同规格贵 5.5 倍，买到的只是不排队"}
            </div>
          )}
        </div>
      </div>
    </Modal>
  );
}

/** 观测面板（⌥1）：队列进度 + 心跳 + 额度分账。 */
function ObserveModal({
  tick,
  now,
  credit,
  onClose,
}: {
  tick: V2vTick | null;
  now: number;
  credit: CreditStats | null;
  onClose: () => void;
}) {
  return (
    <Modal
      title="队列观测"
      width="w700"
      onClose={onClose}
      headerExtra={
        // 「上次查询」读 `lastSweepAt`（真实问过即梦的时刻），不是心跳时刻 ——
        // 同顶栏那个按钮的理由：两者差一个数量级，混用就是在说数据比实际新鲜。
        <span className="chip">
          {tick == null
            ? "等待首轮心跳"
            : `${tick.running} 在跑 · 上次查询 ${
                tick.lastSweepAt == null ? "还没查过" : fmtAgo(Math.max(0, now - tick.lastSweepAt))
              }`}
        </span>
      }
      footer={
        <>
          <span className="fs11 t3">
            排队位次按轮询节拍采样落库，保留 30 天；额度快照一天一条。
          </span>
          <div className="f1" />
          <button type="button" className="btn sm pri" onClick={onClose}>
            完成
          </button>
        </>
      }
    >
      <div style={{ padding: 4 }}>
        <V2vQueuePanel tick={tick} now={now} always />

        {/* 排产用的两块。放在出片统计**之前**：那些是「已经花掉的」，
            而这两块是「下一批什么时候发」——后者才是打开这个面板时要决定的事。 */}
        <div className="vsec mt10">非 VIP 排队观测（近 7 天）</div>
        <div className="mt5">
          <V2vQueueTrend hours={7 * 24} />
        </div>

        <div className="vsec mt10">每日额度（近 14 天）</div>
        <div className="mt5">
          <V2vCreditDaily />
        </div>

        <div className="vsec mt10">额度分账</div>
        <div className="statgrid mt5">
          <Stat label="账户余额" value={credit?.balance == null ? "—" : String(credit.balance)} />
          <Stat label="累计已用" value={String(credit?.spentTotal ?? 0)} />
          <Stat label="近 7 天" value={String(credit?.spentWeek ?? 0)} />
          <Stat label="近 24 小时" value={String(credit?.spentDay ?? 0)} />
        </div>
        <div className="statgrid mt8">
          <Stat label="成片（值回票价）" value={String(credit?.spentPass ?? 0)} tone="ok" />
          <Stat label="未通过（白花的）" value={String(credit?.spentRej ?? 0)} tone="er" />
          <Stat label="待验收（未定论）" value={String(credit?.spentPending ?? 0)} />
          <Stat label="计入条数" value={String(credit?.countedClips ?? 0)} />
        </div>
        {credit?.balanceError && (
          <div className="fs11 mt8" style={{ color: "var(--wr)", lineHeight: 1.7 }}>
            查不到余额：{credit.balanceError}
          </div>
        )}
      </div>
    </Modal>
  );
}

function Stat({ label, value, tone }: { label: string; value: string; tone?: "ok" | "er" }) {
  return (
    <div className="statcell">
      <div className="fs10 t3 nowrap ohide">{label}</div>
      <div
        className="fs16 fw6"
        style={{ color: tone === "ok" ? "var(--ok)" : tone === "er" ? "var(--er)" : undefined }}
      >
        {value}
      </div>
    </div>
  );
}
