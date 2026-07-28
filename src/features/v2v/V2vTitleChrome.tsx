import { fmtAgo, fmtSpan } from "@/features/v2v/model";
import { type AutofillStatus, type QueueStats, commands, unwrap } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { useV2vStore } from "@/stores/v2v";
import { useEffect, useState } from "react";
import { toast } from "sonner";

/**
 * 顶栏上属于视频流水线的那一段（v0.24.0 从页头搬上来）。
 *
 * 通道状态灯 · 刷新 · 余额 —— 三件都在回答「远端此刻是什么状况」，而页头原来还要
 * 同时装筛选片、面板入口与批量按钮，于是它们被挤成一行小字。搬到顶栏之后各归其位，
 * 页里那一屏全部让给了看片。
 *
 * **只在这一页出现**：它们是这条流水线的读数，不是应用级的。
 */
export function V2vTitleChrome() {
  const queue = useV2vStore((s) => s.queue);
  const auto = useV2vStore((s) => s.autofill);
  const balance = useV2vStore((s) => s.credit?.balance ?? null);
  // 「12 秒前」要自己走字，否则一个静止的读数比没有还误导。这个秒表只让已经收到的
  // 时间戳继续走，不去后端要数据 —— 它不是轮询。
  const [now, setNow] = useState(() => Math.floor(Date.now() / 1000));
  useEffect(() => {
    const t = setInterval(() => setNow(Math.floor(Date.now() / 1000)), 1000);
    return () => clearInterval(t);
  }, []);

  return (
    <>
      <span className="tsep" />
      {/* 通道灯的条数不定，故单独装进一条可横滚的带子（`.chstrip`）——
          刷新与余额留在带子外面：它们尺寸固定、且任何时候都得够得着。 */}
      <div className="chstrip">
        <ChannelPills queue={queue} auto={auto} />
      </div>
      <RefreshButton now={now} />
      <div className="pill bal">
        余额 <b>{balance ?? "—"}</b>
      </div>
    </>
  );
}

/**
 * 顶栏那个「刷新」—— 同一个控件回答两件事：**数据有多新** 与 **现在就去问一遍**。
 *
 * ## 它取代的那颗胶囊为什么必须消失
 *
 * 原来这里写的是「轮询中 · 2 在跑 · 3 秒前」。那个「3 秒前」是**心跳**（6 秒一次、
 * 纯内存读），不是「3 秒前问过即梦」—— 真正的查询是 5/10 分钟一次。两个时刻差着一个
 * 数量级，而胶囊把慢的那个藏起来、把快的那个摆出来，于是它最擅长的事就是让人相信
 * 屏幕上的位次和状态是新鲜的。这一格读 `tick.lastSweepAt`（`runner::last_sweep_at`，
 * 真实查询时刻），并且可以点。
 *
 * 「N 在跑」也不在脸上：旁边那排通道状态灯已经**逐通道**答了同一个问题，而求和成
 * 一个数恰恰是 0031 刚拆掉的那种表达。它挪进了 tooltip。
 *
 * ## 四种状态
 *
 * 刷新中（转圈，写「正在查 k/n」）· 后台轮询已关 · 循环卡住/上一轮出错（红）· 空闲。
 * 「循环卡住」仍按心跳判（超过 30 秒没心跳），因为那问的是**后台循环还活着吗**，
 * 与「上次查询多久前」是两件事 —— 关掉轮询开关时前者正常、后者会一直老下去。
 */
function RefreshButton({ now }: { now: number }) {
  const tick = useV2vStore((s) => s.tick);
  const refresh = useV2vStore((s) => s.refresh);
  const busy = refresh?.active === true;
  const beat = tick == null ? null : Math.max(0, now - tick.at);
  const bad = tick != null && (tick.error != null || (beat ?? 0) > 30);
  const off = tick != null && !tick.enabled;
  const swept = tick?.lastSweepAt ?? null;
  const sweptAgo = swept == null ? null : Math.max(0, now - swept);
  const running = tick?.running ?? 0;
  const error = tick?.error ?? refresh?.error ?? null;
  const done = refresh?.done ?? 0;
  const total = refresh?.total ?? 0;

  const label = busy
    ? total > 0
      ? `正在查 ${done}/${total}`
      : "正在刷新"
    : sweptAgo == null
      ? "刷新 · 还没查过"
      : `刷新 · 上次查询 ${fmtAgo(sweptAgo)}`;

  return (
    <button
      type="button"
      className={cn("refbtn", busy && "busy", off && "off", bad && "bad")}
      disabled={busy}
      onClick={() => {
        // **不走页面那把重入锁**：那是给「会改状态、不能连点」的动作用的，而刷新要跑
        // 几十秒，用它锁住整页等于刷新期间什么都干不了。命令本身立刻返回（活儿在 Rust
        // 后台），重入由 Rust 侧的 `REFRESHING` 闸挡，界面这边只把按钮置灰。
        void unwrap(commands.pollV2vNow())
          .then((n) => {
            // **进度一律只从事件来**，不在这里乐观地写一个 `active: true`。Rust 在
            // spawn 之前就发了第一帧，而命令返回值走的是另一条通道 —— 一轮很快的刷新
            // （在跑 0 条）完全可能先收到终帧、再收到这个 `.then()`，那样写下去就是把
            // 已经结束的那一轮复活成「正在刷新」，按钮从此一直转下去。
            if (n === 0) toast("即梦手上没有在跑的条目 —— 本地队列里那些它还不知道");
          })
          .catch((e) => {
            if (e instanceof Error) toast.error(e.message);
          });
      }}
      title={[
        "点一下立刻逐条问一遍即梦：队列位次、生成状态、扣费额度、已出的片，全部现取。",
        `即梦手上 ${running} 条${running > 0 ? "（本地队列里那些即梦还不知道，问不到）" : ""}`,
        off
          ? "后台轮询开关是关的 —— 不影响手动刷新，也不影响已扣额度的任务"
          : "后台自己也在扫（含 VIP 5 分钟一次、全非 VIP 10 分钟一次）",
        beat != null && beat > 30 ? `后台已 ${fmtAgo(beat)}没有心跳` : null,
        error ? `上一轮出错：${error}` : null,
      ]
        .filter(Boolean)
        .join("\n")}
    >
      <span className="dot" />
      {label}
    </button>
  );
}

/**
 * 通道状态灯（0031）—— 一条通道一格。
 *
 * ## 为什么不能再是「即梦 1/1 · 本地排队 78」
 *
 * 那个写法把六条互不相干的队列压成了一个数，而**每一位都是错的**。即梦按模型通道
 * 各排各的队 —— `query_result` 回体里 `queue_info.debug_info.dreamina_matrix_queue_name`
 * 逐通道不同，2026-07-27 五条不同通道的单子同时下出去全部被收下并计费，一条
 * `ExceedConcurrencyLimit` 都没有。于是「1/1」既不是任何一条通道的真实占用，也答不出
 * 那 6 条 2.0mini 为什么不走 —— 而真相是它们本来就该走，是我们自己按一个账户级的
 * 假上限把它们锁住了。
 *
 * ## 每格要答的两个问题
 *
 * 1. **远端此刻在替我做什么**。有排队位次就报位次（非 VIP 通道实测能排到六千多位，
 *    那才是「还要等多久」唯一有意义的信号）；问不到位次而确实有在跑的，就报
 *    「任务中 N」。**绝不把两者混成一个数**，也绝不拿 0 冒充位次（回体里的 0
 *    意思是「已出队」）。
 * 2. **本地还压着多少条同通道的**。「本地队列」只数已放行、随时会自己发出去的那些；
 *    还等着人点确认的另算（写在 title 里）—— 两者的下一步动作完全不同。
 *
 * ## 一条通道一个胶囊，闲着的一律不出现
 *
 * 显示判据是「这条通道上还有没有没走完的事」= 远端在跑 **或** 本地压着队。两者都为 0
 * 的通道在日常里纯是噪音。顶栏位置有限，留给真的还有账要算的那几条 ——
 * 要**全部**通道（含闲着的）看侧栏那张通道卡，它列的是全集。
 *
 * ## 排版：数字带色，标签不带
 *
 * 三类信息的重要性差着量级：**数字**是要读的，**通道名**是要认的，**标签词**只是给
 * 数字贴个名。所以颜色只落在数字上，且三个数各是一个颜色，因为它们是三件不同的事：
 * 前方排队（蓝，这个数要往下掉）· 任务中（绿，即梦此刻真的在生成）·
 * 本地队列（黄，还没发出去也还没花钱的存量）。
 */
function ChannelPills({
  queue,
  auto,
}: {
  queue: QueueStats | null;
  auto: AutofillStatus | null;
}) {
  // 点通道灯 = 把工作台筛到那条通道（此时按动作的那一维自动让位 —— 一次只有一个
  // 筛选，见 `Filter`）。
  //
  // 它原来点开的是参数面板 —— 而人点一条状态灯时想的是「这条队上都有些什么」，
  // 不是「我要改默认参数」。两者都还在，只是各归各的入口（参数在中栏栏头 ⌥3）。
  const setFilter = useV2vStore((s) => s.setFilter);
  const channels = (queue?.channels ?? []).filter((c) => c.running > 0 || c.queued > 0);
  if (channels.length === 0) return null;

  return (
    <>
      {channels.map((c) => {
        const queueing = c.frontQueueIdx != null && c.frontQueueIdx > 0;
        const live = c.running > 0;
        // 常驻队列只写进悬停说明，**不另占一格**：它是「谁放行的」这条元信息，
        // 与「这条通道现在什么状况」不是一个问题，挤在同一排会把后者稀释掉。
        const mine = auto?.enabled === true && c.autofill;
        return (
          <button
            key={c.modelVersion || "(default)"}
            type="button"
            className={cn("chpill", !live && "idle")}
            onClick={() => setFilter({ kind: "channel", key: c.modelVersion })}
            title={[
              `${c.label} 通道（${c.modelVersion || "设置里没指定型号，实际通道由 CLI 挑"}）。`,
              "即梦按模型通道各排各的队 —— 这条排满了，别的通道照样发得出去。",
              queueing
                ? `\n远端：最靠前那一单排在第 ${c.frontQueueIdx} 位。`
                : live
                  ? `\n远端：${c.running} 条在生成中（还没问到排队位次）。`
                  : "\n远端：这条通道上暂时没有在跑的任务。",
              c.oldestWait > 0 ? `最久那条已等 ${fmtSpan(c.oldestWait)}。` : "",
              `\n本地：${c.queued} 条已放行、正等这条通道的空位（出一条自动补一条，不必再点提交）`,
              c.ready > 0 ? `；另有 ${c.ready} 条还等着你点「确认提交」。` : "。",
              `\n同时在跑上限 ${c.limit} 条`,
              c.observedLimit != null
                ? "（本次运行实测出来的：再多发即梦会以 ExceedConcurrencyLimit 拒收）。"
                : "（可在参数面板里调整）。",
              mine
                ? `\n常驻队列配在这条通道上：目标 ${auto?.depth} 条在跑，其中 ${c.autoRunning} 条是它放的。${
                    auto?.blocked ? `当前停在「${auto.blocked}」。` : ""
                  }`
                : "",
              "\n\n点一下把工作台筛到这条通道。",
            ].join("")}
          >
            <span className="dot" />
            {/* VIP 不另挂标签：`short_label` 已经把它写进名字里（「2.0Fast VIP」），
                再挂一个「VIP」小牌子就是同一件事说两遍。 */}
            <span className="nm">{c.label}</span>
            {queueing ? (
              <>
                <span className="k">前方排队</span>
                <span className="n nque">{c.frontQueueIdx}</span>
              </>
            ) : (
              live && (
                <>
                  <span className="k">任务中</span>
                  <span className="n nrun">{c.running}</span>
                </>
              )
            )}
            {c.queued > 0 && (
              <>
                <span className="k">本地队列</span>
                <span className="n nloc">{c.queued}</span>
              </>
            )}
          </button>
        );
      })}
    </>
  );
}
