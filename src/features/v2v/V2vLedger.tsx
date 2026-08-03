import { DescriptionHint, Tooltip } from "@/components/ui/Tooltip";
import { V2vQueueTrail } from "@/features/v2v/V2vQueueTrail";
import {
  type Filter,
  type FilterFace,
  type Row,
  type SliceSummary,
  type TrailStep,
  fmtClock,
  fmtDur,
  trailOf,
} from "@/features/v2v/model";
import type { ActivityEntry, HandoffStatus } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { FolderOpen, ScrollText, SlidersHorizontal } from "lucide-react";
import { type CSSProperties, useMemo, useState } from "react";

/**
 * 工作台中栏 —— 「这一条的账与进度」。
 *
 * 存在的理由是那句反复出现的怀疑：**这条到底花没花钱、走的哪条队、等了多久**。
 * 原来只能开弹窗一条一条看，而看片流里每秒就要判一条 —— 弹窗会把节奏整个打断。
 *
 * 六段，顺序本身就在说话：
 *
 * 1. **摘要卡** —— 先答「这一屏是个什么局面」（这一档 × 这条通道有多少条、花了多少）。
 * 2. **方向性提示** —— 只在处置方向会搞反时出现（超时该等、幽灵该重跑，反了就是钱）。
 * 3. **这一条的账** —— 花了多少、走的什么规格、排在第几。
 * 4. **进度** —— 走到哪一步了，下一步归谁。
 * 5. **这一条的日志** —— 机器刚才替它做了什么。
 * 6. **提示词 / 失败原文** —— 要读全文时才往下翻。
 */
export function V2vLedger({
  row,
  slice,
  filter,
  face,
  handoff,
  rewriteTotal,
  channelLive,
  activity,
  badGroup,
  busy,
  onRewriteGroup,
  onLog,
  onParams,
  onOpenHandoff,
}: {
  row: Row | null;
  slice: SliceSummary;
  filter: Filter;
  face: FilterFace;
  handoff: HandoffStatus | null;
  /**
   * **全流水线**的待改写条数（不受动作/通道筛选影响）。
   *
   * 交接工单是一次物化**全部**待改写条目，所以对账只能跟这个数比。拿当前这一屏的
   * 条数去比，一选通道就会跳出「工单里 2 条、这一档 0 条」这种凭空的警报 ——
   * 而这条警报存在的全部意义是「收了一半」，谎报一次它就再也没人信了。
   */
  rewriteTotal: number;
  /** 当前钉住那条通道上还没走完的总条数（含别的档）。没钉通道时无意义。 */
  channelLive: number;
  activity: ActivityEntry[];
  /** 这条通道上毙得最狠的那个组（≥3 条不通过才报）。 */
  badGroup: { name: string; rejected: number; ids: number[] } | null;
  busy: boolean;
  onRewriteGroup: (ids: number[]) => void;
  onLog: () => void;
  onParams: () => void;
  onOpenHandoff: () => void;
}) {
  return (
    <div className="vled">
      <div className="vledhd">
        <span className="fs13 fw6">这一条的账与进度</span>
        <div className="f1" />
        <Tooltip content="全部执行日志 · 快捷键 ⌥2">
          <button type="button" className="btn xs gho" onClick={onLog}>
            <ScrollText className="ic12" />
            日志
          </button>
        </Tooltip>
        <Tooltip content="默认参数、会话、并发上限与常驻队列 · 快捷键 ⌥3">
          <button type="button" className="btn xs gho" onClick={onParams}>
            <SlidersHorizontal className="ic12" />
            参数
          </button>
        </Tooltip>
      </div>

      <div className="vledbody">
        <SliceCard
          slice={slice}
          face={face}
          filter={filter}
          handoff={handoff}
          rewriteTotal={rewriteTotal}
          channelLive={channelLive}
          badGroup={badGroup}
          busy={busy}
          onRewriteGroup={onRewriteGroup}
          onOpenHandoff={onOpenHandoff}
        />

        {row == null ? (
          <div className="fs11 t3" style={{ padding: "18px 2px", lineHeight: 1.8 }}>
            选择任务查看账与进度。
          </div>
        ) : (
          <ClipLedger row={row} activity={activity} onLog={onLog} />
        )}
      </div>
    </div>
  );
}

/**
 * 摘要卡 —— 这一屏是个什么局面。
 *
 * 三个小格里最要紧的是第一格：**已扣与未扣分开报**。合成一个数会把「已经花掉的」和
 * 「打算花的」混成一笔糊涂账，而人看这一格恰恰是为了决定「还要不要再花」。
 *
 * ## 它不再是一块有色底的卡
 *
 * 从前整张卡按语气刷底色（琥珀 / 红 / 蓝）。那层底色是**面积最大、信息量最小**的
 * 一块颜色：它说的事（这一档是什么语气）标题、色点、计数三处都已经在说，而它把
 * 中栏最靠上的一屏整个染了一遍，下面那些真的要读的字反而被压住。现在改成
 * 「标题与计数用强调色 + 一条分隔线收边」—— 颜色回到字上，卡的边界交给留白与线。
 */
function SliceCard({
  slice,
  face,
  filter,
  handoff,
  rewriteTotal,
  channelLive,
  badGroup,
  busy,
  onRewriteGroup,
  onOpenHandoff,
}: {
  slice: SliceSummary;
  face: FilterFace;
  filter: Filter;
  handoff: HandoffStatus | null;
  rewriteTotal: number;
  channelLive: number;
  badGroup: { name: string; rejected: number; ids: number[] } | null;
  busy: boolean;
  onRewriteGroup: (ids: number[]) => void;
  onOpenHandoff: () => void;
}) {
  const ge = slice.unpriced > 0 ? "≥ " : "";
  const err = handoff?.error ?? null;
  const isRewrite = filter.action === "rewrite";
  // 工单条数与待改写条数对不上，是「收了一半」唯一的可见症状 —— 在此之前没有任何
  // 一处会说这件事，而它的后果是有几条永远不会被改写。
  const mismatch = isRewrite && err == null && handoff != null && handoff.items !== rewriteTotal;
  // 构成条只答一个问题：**这一档分散在哪几条队上**。
  // 钉了通道之后它退化成一条纯色（就那一条队），那时整块不画 —— 一根单色条不回答
  // 任何问题，只是把下面真要读的东西往下推。
  const comp = filter.channel == null ? slice.channels : [];

  return (
    <div className="vsum" data-mood={face.mood} style={{ "--tone": face.color } as CSSProperties}>
      <div className="hd">
        <span className="dot" />
        <span className="ttl">{face.label}</span>
        {face.sub !== "" && (
          <DescriptionHint label={`${face.label}说明`}>{face.sub}</DescriptionHint>
        )}
        <div className="f1" />
        <span className="n">{slice.count} 条</span>
      </div>

      <div className="tiles">
        <div className="tile">
          <span className="k">
            {slice.mixed ? "已扣 / 未计费" : slice.billed > 0 ? "已扣额度" : "预估额度"}
          </span>
          <span className="v">
            {slice.mixed
              ? `${slice.billed} / ${slice.unbilled}`
              : `${ge}${slice.billed > 0 ? slice.billed : slice.unbilled}`}
          </span>
        </div>
        <div className="tile">
          <span className="k">最久已等</span>
          <span className="v">{slice.oldestWait === 0 ? "—" : fmtDur(slice.oldestWait)}</span>
        </div>
        {/* 钉了通道之后「占用通道」恒是 1，那一格就白占了。换成这条队的全貌
            （含别的档），因为那正是钉住它时想知道的下一件事。 */}
        <div className="tile">
          <span className="k">{filter.channel == null ? "占用通道" : "通道未完成"}</span>
          <span className="v">
            {filter.channel == null ? `${slice.channels.length} 条` : `${channelLive} 条`}
          </span>
        </div>
      </div>

      {comp.length > 0 && (
        <div className="comp">
          <div className="bar">
            {comp.map((c) => (
              <span key={c.key || "(default)"} data-tone={c.tone} style={{ flex: c.n }} />
            ))}
          </div>
          <div className="chips">
            {comp.map((c) => (
              <span key={c.key || "(default)"} className="cchip" data-tone={c.tone}>
                <i />
                {c.label} {c.n}
              </span>
            ))}
          </div>
        </div>
      )}

      {slice.unpriced > 0 && (
        <div className="fs10 t3">
          其中 {slice.unpriced} 条没实测过单价，未计入 —— 实际只会更高。
        </div>
      )}

      {/* 交接对账 —— **只在对不上或写不出去时出现**。
          从前这里常驻一句「工单已写到交接目录（2 组 · 2 条）· 上次收录 30 分钟前」外加
          一个「打开交接目录」按钮。那句话在一切正常时不改变任何决定，而按钮在底坞里
          原样有一个；两者合起来把这一档最要紧的那条例外（工单里 N 条、视频生成中 M 条
          对不上，差出来的那几条永远不会被改写）挤成了同一段落里的一小截。
          现在这一格空着就是「对得上」。 */}
      {isRewrite && (err != null || mismatch) && (
        <div className={cn("hoff", err && "er")}>
          {err ? (
            <>工单没能写出去：{err} —— 先确认交接目录还在、可写。</>
          ) : (
            <span className="wr2">
              工单里 {handoff?.items} 条、流水线里 {rewriteTotal} 条缺词 ——
              点「收录改写结果」对一次账。
            </span>
          )}
          <button type="button" className="btn xs gho" onClick={onOpenHandoff}>
            <FolderOpen className="ic12" />
            打开交接目录
          </button>
        </div>
      )}

      {/* 连续毙掉三条以上多半不是「没抽中」，而是这一组的提示词本身有问题 ——
          那时该做的是退回改写整组，不能直接用原提示词再次提交。 */}
      {badGroup && (
        <div className="hoff er">
          「{badGroup.name}」这一组已有 {badGroup.rejected} 条不通过 —— 多半不是没抽中，
          而是提示词本身有问题。
          <button
            type="button"
            className="btn xs gho"
            disabled={busy}
            onClick={() => onRewriteGroup(badGroup.ids)}
          >
            退回改写整组
          </button>
        </div>
      )}
    </div>
  );
}

/** 这一条自己的账 / 进度 / 日志 / 原文。 */
function ClipLedger({
  row,
  activity,
  onLog,
}: {
  row: Row;
  activity: ActivityEntry[];
  onLog: () => void;
}) {
  const c = row.clip;
  const hint = hintFor(row);
  const steps = trailOf(row);
  const logs = useMemo(() => activity.filter((a) => a.clipId === c.id), [activity, c.id]);
  /**
   * 提示词默认折叠。
   *
   * 它是这一栏里最长的一块（视频提示词是一整段叙事），而**判片时不看它** ——
   * 要看它的时候人是在查「为什么出成这样」，那是少数几次。摊开摆着的代价是
   * 进度与日志被推到折叠线以下，于是每选一条都要先滚一屏。
   *
   * 状态**不随换条复位**：开着它多半是因为正在逐条对提示词，那时每换一条都要
   * 重点一次展开，等于把这个开关变成一次性的。
   */
  const [promptOpen, setPromptOpen] = useState(false);

  // 左边那个大数字：回执优先，没回执时退回预估。两者都没有才是「—」。
  const credit = row.credit ?? row.estimate;

  return (
    <>
      {hint && <div className={cn("vhint", hint.tone)}>{hint.text}</div>}

      <div className="vsec">这一条的账</div>
      {/* **Bento**：一格多宽由它装得下什么决定，不是九个一样大的方块。
          九格等分时「第 1 次」「4h58m」这类三四个字符的值各占半行，把这一栏最靠上的
          一屏撑掉一半，于是进度和日志每选一条都要先滚一屏。
          现在按内容定档（六列栅格）：数字类 2 列一行摆三个 · 型号名 3 列 ·
          submit_id 独占一整行（它是唯一真需要整行的值，截断了就没法对账）。
          主次仍靠**颜色**不靠体积：预估蓝（还能改主意），已扣琥珀（钱已经出去了）。 */}
      <div className="vbento mt5">
        <Fact
          k={c.billed ? "额度 · 已扣" : "额度 · 预估"}
          v={credit == null ? "—" : String(credit)}
          tone={c.billed ? "wr" : "acc"}
          span={2}
          strong
        />
        {/* 还没提交出去的只有「什么时候进的队」可说 —— 那时「已等」按定义是 0，
            摆一个 0 出来会被读成「刚刚才排上」。 */}
        <Fact
          k={row.waitSecs === 0 ? "入队" : "已等"}
          v={row.waitSecs === 0 ? fmtClock(c.createdAt) : fmtDur(row.waitSecs)}
          span={2}
        />
        <Fact
          k="上次查询"
          v={row.polledAgo == null ? "—" : `${fmtDur(row.polledAgo)}前`}
          tone={row.polledAgo != null && row.polledAgo > 1800 ? "wr" : undefined}
          span={2}
        />
        <Fact
          k="规格"
          v={
            row.resolution && row.duration
              ? `${row.resolution} · ${row.duration}s`
              : (row.resolution ?? "CLI 默认")
          }
          span={2}
        />
        {/* 两种位次的标签必须说清是谁的队 —— 本地排第 3 和即梦排第 4485 是完全不同的
            两件事，混成一个「第 N 位」会让人以为快轮到了。 */}
        <Fact
          k={row.action === "queued" ? "本地队列" : "即梦队列"}
          v={row.queuePos == null ? "—" : `第 ${row.queuePos} 位`}
          span={2}
        />
        <Fact
          k="尝试"
          v={`第 ${Math.max(1, c.attempt)} 次`}
          tone={c.attempt > 1 ? "wr" : undefined}
          span={2}
        />
        <Fact k="通道（我们发的）" v={row.modelFull ?? "跟随 CLI 默认"} span={3} />
        <Fact k="计费型号（回执）" v={c.benefitType ?? "—"} span={3} />
        <Fact k="submit_id" v={c.submitId ?? "—"} span={6} />
      </div>

      {/* 「进度」而不是「历程」：这一列要答的是「走到哪儿了、还剩几步」，
          而「历程」听着像一份读完才有用的流水账。
          走完的一步是一枚**绿勾**，不是一个带色的圆点 —— 圆点分四种颜色时，
          「蓝点和琥珀点差在哪」这个问题没有答案（它们都只是「走过了」）；
          一枚勾则不需要图例。剩下的颜色只描述例外：红=出事了，空心=还没轮到。 */}
      <div className="vsec">进度</div>
      <div className="vtl">
        {steps.map((s, i) => (
          <div key={s.key} className={cn("vtlrow", s.state)}>
            <div className="rail">
              {s.state === "done" ? (
                <span className={cn("ck", TONE_CLASS[s.tone])}>✓</span>
              ) : (
                <span className={cn("dot", TONE_CLASS[s.tone])} />
              )}
              {i < steps.length - 1 && <span className="ln" />}
            </div>
            <div className="bd">
              <div className="hd">
                <span className="at">{s.at}</span>
                <span className="what">{s.what}</span>
              </div>
              {s.sub !== "" && <span className="sub">{s.sub}</span>}
            </div>
          </div>
        ))}
      </div>
      {/* 位次的**轨迹**：一个静止的「第 4485 位」答不出「今晚能不能出片」，
          而那正是看这一栏时真正要决定的事。只在即梦手上的条目才有队可排。 */}
      {row.stage === "run" && row.action !== "queued" && (
        <V2vQueueTrail clipId={c.id} queueIdx={c.queueIdx} />
      )}

      <div className="vsec">
        这一条的日志
        <DescriptionHint label="日志保留说明">
          执行日志仅保留本次运行最近 500 条；任务状态会持续保存在数据库中。
        </DescriptionHint>
        <button type="button" className="lnk" onClick={onLog}>
          全部日志
        </button>
      </div>
      <div className="vlog">
        {logs.length === 0 ? (
          // **空的时候说实话**。执行日志是进程内的环形缓冲（500 条、重启即清空），
          // 所以老条目在这里空着是常态而不是故障 —— 摆一个空框会让人以为「这条什么
          // 都没发生过」，而它的状态明明好好地存在库里。
          <div className="none">暂无本次运行日志。</div>
        ) : (
          logs.map((l) => (
            <div key={l.seq} className={cn("lrow", l.level !== "info" && l.level)}>
              <span className="rail" />
              <span className="at">{fmtLogClock(l.at)}</span>
              <span className="tag">{PHASE_LABEL[l.phase] ?? l.phase}</span>
              <span className="msg">{l.message}</span>
            </div>
          ))
        )}
      </div>

      <div className="vsec">
        视频提示词
        <button type="button" className="lnk" onClick={() => setPromptOpen((v) => !v)}>
          {promptOpen ? "收起" : "展开"}
        </button>
      </div>
      {promptOpen && <div className="vprompt">{c.videoPrompt ?? "（还没有，等 skill 写回）"}</div>}

      {c.errorMessage && (
        <>
          <div className="vsec">失败原文</div>
          <div className="vprompt terr">{c.errorMessage}</div>
        </>
      )}
      <div style={{ height: 10 }} />
    </>
  );
}

/**
 * 进度节点的类名。
 *
 * 写成一张显式的表而不是 `` `t-${tone}` `` —— `classnames.test.ts` 的反向检查
 * （「定义了的 class 必须有人用」）扫的是源码里的字面量，模板拼出来的类名它看不见，
 * 于是这五条规则会被当成死类报出来。显式表顺带让「有哪几种色」可以直接搜到。
 */
const TONE_CLASS: Record<TrailStep["tone"], string> = {
  ok: "t-ok",
  acc: "t-acc",
  rev: "t-rev",
  er: "t-er",
  dim: "t-dim",
};

const PHASE_LABEL: Record<string, string> = {
  cli: "CLI",
  submit: "提交",
  poll: "轮询",
  media: "落盘",
  handoff: "交接",
};

/** unix 秒 → 本地 HH:mm:ss。日志的时间粒度是秒级。 */
function fmtLogClock(unix: number): string {
  const d = new Date(unix * 1000);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}

/**
 * 一格账。`span` 是它在六列栅格里占几列（bento 的全部机关就是这一个数）；
 * `strong` 的那格靠字号与颜色（不是靠体积）把自己顶出来。
 */
function Fact({
  k,
  v,
  tone,
  span,
  strong,
}: {
  k: string;
  v: string;
  tone?: "wr" | "acc" | undefined;
  span: number;
  strong?: boolean;
}) {
  return (
    <div className="vfact" style={{ gridColumn: `span ${span}` }}>
      <div className="k">{k}</div>
      <div
        className={cn("v", strong && "hero", tone === "wr" && "wr2", tone === "acc" && "acc2")}
        title={v}
      >
        {v}
      </div>
    </div>
  );
}

/**
 * 上下文提示 —— 只在**处置方向会搞反**的时候出现。
 *
 * 超时与幽灵单是一对相反的事：一个额度已扣（该等），一个从未计费（该重跑）。
 * 指错方向的代价是真金白银，所以这两句必须写在按钮旁边，而不是躺在文档里。
 */
export function hintFor(row: Row): { text: string; tone: "wr" | "er" } | null {
  if (row.signals.has("timeout")) {
    return {
      tone: "wr",
      text: "超时只是本地停止等待：额度已扣、即梦仍在运行。使用「继续等待」沿用原提交单；恢复提交可能再次计费。",
    };
  }
  if (row.signals.has("phantom")) {
    return {
      tone: "er",
      text: "无队列位次且无扣费回执：即梦未入队、未计费，可以恢复。",
    };
  }
  // 幽灵单的反面：那个是「查得出没花钱」，这个是「根本没查出话来」。
  // CLI 在超时被杀之前可能已经下过单，submit_id 却随进程一起没了。
  if (row.clip.errorType === "submit_timeout") {
    return {
      tone: "er",
      text: "提交时 CLI 超时且没有 submit_id，但任务可能已经提交并计费。恢复前请先在即梦核对；再次提交可能重复计费。",
    };
  }
  if (row.clip.errorType === "submit_interrupted") {
    return {
      tone: "er",
      text: "应用在提交回执落库前中断，远端是否接单和扣费无法确认，因此没有自动重提。请先在即梦任务记录中核对。",
    };
  }
  if (row.slow) {
    return {
      tone: "wr",
      text: "这一条已超同通道中位等待时长的 3 倍。退避轮询已放缓到十分钟一次，不必手动催。",
    };
  }
  if (row.vip) {
    return {
      tone: "wr",
      text: `走的是 vip 通道${row.estimate == null ? "" : `：约 ${row.estimate} 额度/条`}，非 vip 同规格实测只要 8。vip 买到的只是不排队。`,
    };
  }
  return null;
}

/** 这条通道上毙得最狠的那个组（≥3 条不通过才报）。返回可退回改写的条目 id。 */
export function worstGroup(rows: Row[]): { name: string; rejected: number; ids: number[] } | null {
  const byGroup = new Map<string, Row[]>();
  for (const r of rows) {
    const k = r.clip.groupName || "未分组";
    const b = byGroup.get(k);
    if (b) b.push(r);
    else byGroup.set(k, [r]);
  }
  let best: { name: string; rejected: number; ids: number[] } | null = null;
  for (const [name, list] of byGroup) {
    const rejected = list.filter((r) => r.stage === "rej").length;
    if (rejected < 3) continue;
    if (best && best.rejected >= rejected) continue;
    best = {
      name,
      rejected,
      // pass 的不动（`requeue_for_rewrite` 也拒），已经在改写队列里的重发一次无害。
      ids: list.filter((r) => r.stage !== "pass").map((r) => r.clip.id),
    };
  }
  return best;
}
