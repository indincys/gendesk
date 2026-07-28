import {
  type ActionFilter,
  type Row,
  buildSections,
  carryParams,
  deriveRows,
  matchAction,
  matchQuery,
  nextAction,
} from "@/features/v2v/model";
import type { ClipView, EffectiveParams, ModelInfo } from "@/lib/ipc";
import { describe, expect, it } from "vitest";

/**
 * 派生模型的测试。
 *
 * 「业务真相在 Rust，测试也在 Rust」对状态机成立，但这一份不是 UI 壳：它决定表格里
 * 「情况 · 判断依据」那一列说什么，而那句话直接指挥人按哪个按钮 —— 幽灵单说「重跑」
 * （不花钱），超时说「继续等待」（重跑要再花一份钱）。指错方向的代价是真金白银，
 * 且错了不会报错，只会安静地多扣一批额度。
 */

const NOW = 1_800_000_000;

const MODELS: ModelInfo[] = [
  {
    modelVersion: "seedance2.0fast",
    label: "2.0Fast",
    minDuration: 4,
    maxDuration: 15,
    resolutions: ["720p"],
    creditAtMin: 8,
    resPrices: [{ resolution: "720p", creditPerSec: 2 }],
    vip: false,
  },
  {
    modelVersion: "seedance2.0fast_vip",
    label: "2.0Fast VIP",
    minDuration: 4,
    maxDuration: 15,
    resolutions: ["720p"],
    creditAtMin: 44,
    resPrices: [{ resolution: "720p", creditPerSec: 11 }],
    vip: true,
  },
];

const EFF: EffectiveParams = {
  bin: "dreamina",
  resolvedBin: "/usr/local/bin/dreamina",
  modelVersion: "seedance2.0fast",
  duration: 4,
  videoResolution: "720p",
  session: null,
  usesCliDefaults: false,
  sampleCommand: "dreamina image2video …",
  error: null,
};

function clip(over: Partial<ClipView> = {}): ClipView {
  return {
    id: 1,
    workId: 1,
    groupId: 1,
    groupName: "B-Roll 冬季手袋",
    batchId: 31,
    stage: "rev",
    promptCode: "BR31-0140",
    imagePath: "/img.jpg",
    thumbPath: "/thumb.jpg",
    sourcePrompt: "生图提示词全文",
    variablePart: "可变部分",
    videoPrompt: "镜头缓慢向前推近",
    modelVersion: null,
    duration: null,
    videoResolution: null,
    submitId: "sub-1",
    creditCount: null,
    videoPath: "/clips/1.mp4",
    posterPath: "/clips/1.jpg",
    width: 720,
    height: 1280,
    fps: 24,
    durationSec: 4,
    attempt: 1,
    errorType: null,
    errorMessage: null,
    genStatus: null,
    queueIdx: null,
    polledAt: null,
    benefitType: null,
    submittedAt: null,
    firstSubmittedAt: null,
    submitCredit: null,
    submitStatus: null,
    createdAt: NOW - 7200,
    rewroteAt: NOW - 7000,
    finishedAt: NOW - 600,
    reviewedAt: null,
    autoSubmitted: false,
    submitQueuedAt: null,
    exportPath: null,
    phantomSuspect: false,
    billed: false,
    awaitingDownload: false,
    acceptedAt: NOW - 8000,
    updatedAt: NOW - 600,
    ...over,
  };
}

/**
 * `limit` 是**这一条通道**的在跑上限（0031）。测试里的 clip 都不写 modelVersion，
 * 故它们全落在 `EFF.modelVersion` 那条通道上 —— 用它做键。
 */
const derive = (cs: ClipView[], limit = 1) =>
  deriveRows(cs, MODELS, EFF, NOW, new Map([[EFF.modelVersion ?? "", limit]]));

describe("本地待发队列（0028）", () => {
  /**
   * 这一组测试守的是这次事故的核心教训：**「等你点确认提交」与「你点过了，在排队」
   * 必须是两件看得出区别的事**。
   *
   * 事故经过：选 9 条点确认 → 9 条一起砸向即梦 → 即梦只跑得下 1 条，其余 8 条回来
   * `ExceedConcurrencyLimit` 被判死进「处理异常」。而界面从头到尾没有任何一处
   * 提到过这个上限的存在。
   */
  it("已放行的条目不再算「待放行」，而是自成一档「排队中」", () => {
    const [waiting, released] = derive([
      clip({ id: 1, stage: "ready", videoPath: null }),
      clip({ id: 2, stage: "ready", videoPath: null, submitQueuedAt: NOW - 60 }),
    ]);
    expect(waiting?.action).toBe("submit");
    expect(waiting?.situation).toContain("等你点确认提交");
    expect(released?.action).toBe("queued");
    expect(released?.situation).toContain("已放行");
    // 排队中的**不该**进「需要我」：人已经做过决定了，剩下的是机器的事。
    expect(matchAction(released as Row, "mine")).toBe(false);
    expect(matchAction(waiting as Row, "mine")).toBe(true);
  });

  it("本地位次严格照放行时刻算，与后端取用顺序同源", () => {
    // 放行时刻与 id 顺序故意相反：排的是放行时刻，不是 id。
    const rows = derive([
      clip({ id: 5, stage: "ready", videoPath: null, submitQueuedAt: 300 }),
      clip({ id: 2, stage: "ready", videoPath: null, submitQueuedAt: 100 }),
      clip({ id: 9, stage: "ready", videoPath: null, submitQueuedAt: 200 }),
    ]);
    expect(rows.map((r) => [r.clip.id, r.queuePos])).toEqual([
      [5, 3],
      [2, 1],
      [9, 2],
    ]);
    // 队首那条要说「下一个就发它」，后面的才报位次 —— 否则「第 1 位」还要人自己翻译。
    expect(rows.find((r) => r.clip.id === 2)?.situation).toContain("下一个就发它");
    expect(rows.find((r) => r.clip.id === 5)?.situation).toContain("本地排第 3");
  });

  /**
   * 断言原文从「即梦同时只跑 3 条」改成「2.0Fast 通道同时只跑 3 条」（0031）。
   *
   * **改的是前提被实测推翻的那一半，不是断言的意图**：这条测试守的是
   * 「为什么只跑 N 条必须在界面上答得出」，那一半原样保留。改掉的是「即梦」这个主语
   * —— 上限是**逐通道**的（即梦回体里 `dreamina_matrix_queue_name` 逐通道不同，
   * 2026-07-27 五条不同通道同时提交全部被收下），说成账户级会让人对着一条
   * 明明能立刻发出去的 mini 干等，而那正是这一版要修的故障。
   */
  it("在跑上限写进文案里 —— 「为什么只跑 1 条」必须在界面上答得出，且点名是哪条通道", () => {
    const [r] = derive([clip({ stage: "ready", videoPath: null, submitQueuedAt: 1 })], 3);
    expect(r?.situation).toContain("同时只跑 3 条");
    expect(r?.situation).toContain("2.0Fast");
  });

  /**
   * 本地位次**按通道各排各的**（0031）—— 后端补位就是逐通道取队首的
   * （`pick_submit_queued_on`）。按全局排的话，界面会对一条马上就要发出去的 mini
   * 说「本地排第 79」，而它前面那 78 条全在另一条队上、与它毫无关系。
   */
  it("本地位次不跨通道累计：另一条通道排得再长也不占这条的位次", () => {
    const rows = deriveRows(
      [
        clip({ id: 1, stage: "ready", videoPath: null, submitQueuedAt: 100 }),
        clip({ id: 2, stage: "ready", videoPath: null, submitQueuedAt: 200 }),
        clip({
          id: 3,
          stage: "ready",
          videoPath: null,
          submitQueuedAt: 300,
          modelVersion: "seedance2.0fast_vip",
        }),
      ],
      MODELS,
      EFF,
      NOW,
      new Map([
        ["seedance2.0fast", 1],
        ["seedance2.0fast_vip", 1],
      ]),
    );
    expect(rows.map((r) => [r.clip.id, r.queuePos])).toEqual([
      [1, 1],
      [2, 2],
      [3, 1],
    ]);
    expect(rows.find((r) => r.clip.id === 3)?.situation).toContain("下一个就发它");
  });

  it("即梦的排队位次直接说人话：前面还有多少个", () => {
    const [r] = derive([
      clip({ stage: "run", videoPath: null, queueIdx: 4485, firstSubmittedAt: NOW - 600 }),
    ]);
    expect(r?.queuePos).toBe(4485);
    expect(r?.situation).toBe("即梦在排队 · 前面还有 4485 个");
  });

  /**
   * 断言从「退回本批进度」改成了「说问不到 + 报上次问的时刻」（0032）。
   *
   * **改断言而不是删**：原断言的前提被推翻了 —— 它假定「本批已出 X/Y」是一个有意义的
   * 回落，而实际上同一批次的条目分散在好几条互不相干的即梦队列上（各排各的队、上限
   * 各不相同），那个分数的分子分母都不指向任何真实的队列。更糟的是它长得像个答案，
   * 于是没人再去追问「这条到底跑完了没有」。
   *
   * 「绝不编一个数字出来」这条规则本身没变，只是现在贯彻得更彻底：问不到就说问不到。
   */
  it("问不到位次就说问不到、并报上次问的时刻，绝不编一个数字出来", () => {
    const [r] = derive([
      clip({
        stage: "run",
        videoPath: null,
        queueIdx: null,
        firstSubmittedAt: NOW - 600,
        polledAt: NOW - 240,
      }),
    ]);
    expect(r?.queuePos).toBeNull();
    expect(r?.situation).toBe("即梦在跑 · 位次问不到（4 分钟前问过）");
    expect(r?.situation).not.toContain("本批");
  });

  /**
   * 即梦做完了、卡在下载的那一格，**不能说成「即梦在跑」**。
   *
   * 实跑抓到的：一条 2.0Mini 出片后下载超时（`query_result --download_dir` 走 CLI 自己的
   * 30 秒 HTTP 超时），条目停在 `run`、`gen_status=success`、`queue_idx=0`、已扣 36 额度、
   * `video_path` 为空。而 0 不是位次（是「已出队」），于是这一行会掉进「位次问不到」
   * 那条回落 —— 显示「即梦在跑」挂好几轮，而真相是片子早就好了、钱也扣完了。
   *
   * 判定由 Rust 下发（`awaitingDownload`，读 `dreamina::classify_status`），
   * 前端不拿 `genStatus === "success"` 凑：那个枚举大小写不统一，还有 `PartialSuccess`。
   */
  it("即梦做完了卡在下载时，说的是「正在取回」而不是「即梦在跑」", () => {
    const [r] = derive([
      clip({
        stage: "run",
        videoPath: null,
        genStatus: "success",
        queueIdx: 0,
        creditCount: 36,
        awaitingDownload: true,
        firstSubmittedAt: NOW - 600,
        polledAt: NOW - 30,
      }),
    ]);
    expect(r?.situation).toBe("已出片 · 正在取回到本地，失败会自动重试");
    expect(r?.situation).not.toContain("即梦在跑");
    // 已经扣过费了，所以它绝不该被当成幽灵单（那句话会说「重跑不花钱」）。
    expect(r?.phantomLive).toBe(false);
  });

  it("一次都没问到过位次时不假装问过", () => {
    const [r] = derive([
      clip({
        stage: "run",
        videoPath: null,
        queueIdx: null,
        firstSubmittedAt: NOW - 600,
        polledAt: null,
      }),
    ]);
    expect(r?.situation).toBe("即梦在跑 · 还没问到过位次");
  });
});

describe("幽灵单判定", () => {
  /**
   * 判定**不在这里**：`clip.phantomSuspect` 由 Rust 下发（`runner::clip_looks_phantom`），
   * 它读的是全部五处计费证据（本次回体两处 + 已落库三处），也正是真会走去
   * `fail(phantom)` 那条路径的同一个函数。
   *
   * 前端曾抄过一份判据（三个字段 + 一个手抄的 15 分钟宽限期常量），而它按
   * `firstSubmittedAt` 计时、Rust 按 `submittedAt` 计时 —— 「继续等待」按过一次之后
   * 两边就会对同一条给出相反的结论。这里剩下的责任只有一条：**照它说的办**。
   */
  it("下发的结论直接决定情况、信号与额度口径", () => {
    const [r] = derive([
      clip({ stage: "run", videoPath: null, firstSubmittedAt: NOW - 9000, phantomSuspect: true }),
    ]);
    expect(r?.phantomLive).toBe(true);
    expect(r?.signals.has("phantom")).toBe(true);
    expect(r?.situation).toContain("疑幽灵单");
    expect(r?.credit).toBe(0);
  });

  it("没下发就当它在正常排队 —— 前端绝不自己补一套判据", () => {
    // 刻意摆出「三个字段全空 + 等了两个半小时」这个旧判据一定会判幽灵的形状：
    // Rust 说不是（比如它看到了提交回执或历史队列位次），这里就必须说不是。
    const [r] = derive([
      clip({
        stage: "run",
        videoPath: null,
        firstSubmittedAt: NOW - 9000,
        submitCredit: null,
        queueIdx: null,
        creditCount: null,
        phantomSuspect: false,
      }),
    ]);
    expect(r?.phantomLive).toBe(false);
    expect(r?.situation).toContain("即梦在跑");
  });
});

describe("超时与幽灵的处置必须相反", () => {
  it("超时说「继续等待 · 额度已扣」", () => {
    const [r] = derive([
      clip({ stage: "fail", errorType: "timeout", videoPath: null, submitId: "sub-9" }),
    ]);
    expect(r?.signals.has("timeout")).toBe(true);
    expect(r?.situation).toContain("继续等待");
  });

  it("判死的幽灵单说「免费重跑 · 从未计费」", () => {
    const [r] = derive([clip({ stage: "fail", errorType: "phantom", videoPath: null })]);
    expect(r?.signals.has("phantom")).toBe(true);
    expect(r?.situation).toContain("免费重跑");
    expect(r?.credit).toBe(0);
  });

  /**
   * 提交超时是第三种，且它与幽灵单恰好构成一对反例：幽灵单是**查得出没花钱**
   * （两个信号同时缺席，重跑免费），提交超时是**根本没查出话来** —— CLI 在被我们
   * 杀掉之前可能已经下过单、扣过费，submit_id 却随进程一起没了。
   * 所以这一格绝不能出现「重跑」二字，必须先说核对。
   */
  it("提交超时说「可能已扣费 · 核对后再决定」，不说免费重跑", () => {
    const [r] = derive([
      clip({ stage: "fail", errorType: "submit_timeout", videoPath: null, submitId: null }),
    ]);
    expect(r?.situation).toContain("核对");
    expect(r?.situation).not.toContain("免费");
    // 幽灵单那条信号不能沾上：它的含义是「确认没花钱」，而这里恰恰不确认。
    expect(r?.signals.has("phantom")).toBe(false);
  });
});

describe("等待异常", () => {
  /** 在跑且**确实入了队、确实计了费**的一条（否则它会先被判成幽灵单）。 */
  const running = (id: number, batchId: number, waited: number) =>
    clip({
      id,
      batchId,
      stage: "run",
      videoPath: null,
      submitCredit: 8,
      queueIdx: 4485,
      firstSubmittedAt: NOW - waited,
    });

  // 相对判据（本批中位数的 3 倍），不是拍脑袋的绝对秒数。
  it("按同批在跑条目的中位等待时长比，超 3 倍才标", () => {
    const rows = derive([
      running(1, 31, 600),
      running(2, 31, 600),
      running(3, 31, 700),
      running(4, 31, 9000),
    ]);
    expect(rows.find((r) => r.clip.id === 4)?.slow).toBe(true);
    expect(rows.find((r) => r.clip.id === 4)?.situation).toContain("等待异常");
    expect(rows.find((r) => r.clip.id === 1)?.slow).toBe(false);
  });

  it("不同批次各算各的中位数 —— 混在一起算出来的中位数谁也不代表", () => {
    const rows = derive([
      running(1, 31, 60),
      running(2, 31, 60),
      // 昨夜那一批整体就等了 9 小时，它们彼此之间并不异常。
      running(3, 30, 32400),
      running(4, 30, 32000),
    ]);
    expect(rows.every((r) => !r.slow)).toBe(true);
  });

  // 幽灵单优先：它是「没入队、没计费」，说它「等得久」会把人引到「再等等」，
  // 而正确动作是免费重跑。两个提示并存时，指错方向的那个必须让路。
  it("既没入队也没计费时判幽灵单，不判等待异常", () => {
    const rows = derive([
      running(1, 31, 600),
      running(2, 31, 600),
      clip({
        id: 3,
        batchId: 31,
        stage: "run",
        videoPath: null,
        firstSubmittedAt: NOW - 9000,
        phantomSuspect: true,
      }),
    ]);
    const ghost = rows.find((r) => r.clip.id === 3);
    expect(ghost?.phantomLive).toBe(true);
    expect(ghost?.slow).toBe(false);
    expect(ghost?.situation).toContain("疑幽灵单");
  });
});

describe("额度", () => {
  it("回执优先于预估；有回执时不标「预估」", () => {
    const [r] = derive([clip({ creditCount: 44 })]);
    expect(r?.credit).toBe(44);
    expect(r?.creditEstimated).toBe(false);
  });

  it("没回执时按单价表预估，并标明是预估", () => {
    const [r] = derive([clip({ stage: "run", videoPath: null, firstSubmittedAt: NOW - 60 })]);
    expect(r?.credit).toBe(8); // 2 额度/秒 × 4s
    expect(r?.creditEstimated).toBe(true);
  });

  it("还没提交的两列不谈钱 —— 一分都还没花", () => {
    for (const stage of ["rewrite", "ready"]) {
      const [r] = derive([clip({ stage, videoPath: null, submitId: null })]);
      expect(r?.credit).toBeNull();
    }
  });

  it("查不到单价就不猜（未实测的组合）", () => {
    const [r] = derive([
      clip({
        stage: "run",
        videoPath: null,
        modelVersion: "seedance2.0_vip",
        videoResolution: "1080p",
        firstSubmittedAt: NOW - 60,
      }),
    ]);
    expect(r?.estimate).toBeNull();
  });
});

describe("信号", () => {
  it("vip 通道按模型名后缀认，且回落到设置里的默认模型", () => {
    const [vip] = derive([clip({ modelVersion: "seedance2.0fast_vip" })]);
    expect(vip?.vip).toBe(true);
    // 简写现在由后端下发（`ModelInfo.label` ← `dreamina::short_label`），不再是
    // 前端各自 `replace(/^seedance/,"")` 的结果 —— 那份分叉的产物是同屏出现
    // 「2.0fast」「2.0Fast」两种拼法。断言随之改成下发的那个值（0031）。
    expect(vip?.modelShort).toBe("2.0Fast VIP");
    const [plain] = derive([clip({ modelVersion: null })]);
    expect(plain?.vip).toBe(false);
    expect(plain?.modelFull).toBe("seedance2.0fast");
  });

  // 成片的下游是本地输出目录，不是资产库（v0.22.0）。而拷贝失败**不回滚验收**，
  // 所以「pass 但 export_path 为空」是真实会出现的状态：片子做出来了却没落地，
  // 而在此之前界面上没有任何一处会提这件事。
  it("成片按有没有交付到输出目录说话，未交付要标红", () => {
    const [ok] = derive([
      clip({ stage: "pass", exportPath: "/out/视频/甲组/BR310140_260727.mp4" }),
    ]);
    expect(ok?.situation).toBe("已成片 · 已交付");
    expect(ok?.situationTone).toBe("t3");

    const [miss] = derive([clip({ stage: "pass", exportPath: null })]);
    expect(miss?.situation).toContain("未交付");
    expect(miss?.situationTone).toBe("er");

    // 空白串等同于没有 —— 交付路径是从文件系统回来的，别指望它只会是 null。
    expect(derive([clip({ stage: "pass", exportPath: "  " })])[0]?.situation).toContain("未交付");
  });

  it("重跑过 = 尝试次数 > 1（同一张图已经花过不止一份额度）", () => {
    expect(derive([clip({ attempt: 2 })])[0]?.signals.has("rerun")).toBe(true);
    expect(derive([clip({ attempt: 1 })])[0]?.signals.has("rerun")).toBe(false);
  });
});

describe("下一步动作", () => {
  it("七态各归其位", () => {
    expect(nextAction("rewrite", false)).toBe("rewrite");
    expect(nextAction("ready", false)).toBe("submit");
    expect(nextAction("run", false)).toBe("wait");
    expect(nextAction("rev", false)).toBe("review");
    expect(nextAction("fail", false)).toBe("fix");
    expect(nextAction("pass", false)).toBe("done");
    expect(nextAction("rej", false)).toBe("done");
  });

  // 幽灵单只存在于 `run`，而旧的「需要我」= ready|rev|fail 不含 run ——
  // 于是唯一该**免费**重跑的那一类，被默认筛选整个藏了起来。实测一次事故里
  // 18 条这样的单子挂了十几个小时无人察觉。
  it("在跑的幽灵单归「处理异常」，不归「等即梦」", () => {
    const rows = derive([
      clip({
        id: 1,
        stage: "run",
        videoPath: null,
        firstSubmittedAt: NOW - 9000,
        phantomSuspect: true,
      }),
    ]);
    const ghost = rows[0];
    if (!ghost) throw new Error("fixture");
    expect(ghost.phantomLive).toBe(true);
    expect(ghost.action).toBe("fix");
    expect(matchAction(ghost, "mine")).toBe(true);
    expect(matchAction(ghost, "wait")).toBe(false);
  });
});

describe("筛选", () => {
  const only = (rows: Row[], f: ActionFilter) => rows.filter((r) => matchAction(r, f));

  /**
   * 用户报的原症：21 条待改写时，界面同时显示「需要我 0」「待改写 21」「无待办」。
   *
   * 那 21 条**恰恰卡在人身上** —— GenDesk 已经把工单物化好，在等人去 Claude Code
   * 里跑改写。把这一步排除在「需要我」之外，等于让全流水线最大的一处阻塞显示为 0。
   */
  it("待改写算「需要我」—— 21 条待改写时这个数是 21，不是 0", () => {
    const rows = derive(
      Array.from({ length: 21 }, (_, i) =>
        clip({ id: i + 1, stage: "rewrite", videoPrompt: null, videoPath: null, submitId: null }),
      ),
    );
    expect(only(rows, "mine")).toHaveLength(21);
    expect(only(rows, "rewrite")).toHaveLength(21);
  });

  it("「需要我」= 改写 / 放行 / 验收 / 处理异常，不含机器在跑的", () => {
    const rows = derive([
      clip({ id: 1, stage: "ready", videoPath: null, submitId: null }),
      clip({ id: 2, stage: "rev" }),
      clip({ id: 3, stage: "fail", errorType: "timeout", videoPath: null }),
      clip({ id: 4, stage: "rewrite", videoPrompt: null, videoPath: null, submitId: null }),
      clip({ id: 5, stage: "run", videoPath: null, submitCredit: 8, firstSubmittedAt: NOW - 60 }),
      clip({ id: 6, stage: "pass" }),
    ]);
    expect(only(rows, "mine").map((r) => r.clip.id)).toEqual([1, 2, 3, 4]);
  });

  // 工作台回答的是「还剩多少活」。把已经定案的算进去，这个数就再也不准了 ——
  // 实测 18 条验收通过的片子一直挂在看板上，人得先在心里把它们减掉才看得出待办。
  it("「全部」= 全部在制，不含成片与未通过；但显式筛「未通过」仍能翻出来", () => {
    const rows = derive([
      clip({ id: 1, stage: "rewrite", videoPrompt: null, videoPath: null, submitId: null }),
      clip({ id: 2, stage: "run", videoPath: null, submitCredit: 8, firstSubmittedAt: NOW - 60 }),
      clip({ id: 3, stage: "fail", errorType: "timeout", videoPath: null }),
      clip({ id: 4, stage: "pass" }),
      clip({ id: 5, stage: "rej" }),
    ]);
    expect(only(rows, "all").map((r) => r.clip.id)).toEqual([1, 2, 3]);
    expect(only(rows, "rej").map((r) => r.clip.id)).toEqual([5]);
  });

  it("待改写那句既点名动作也点名工具 —— 否则没人知道该去哪儿干什么", () => {
    const [r] = derive([
      clip({ stage: "rewrite", videoPrompt: null, videoPath: null, submitId: null }),
    ]);
    expect(r?.situation).toContain("v2v-rewrite");
    expect(r?.situation).not.toContain("物化");
  });

  it("搜索一次覆盖编号/组名/提示词/submit_id —— 人并不知道自己记住的是哪一处", () => {
    const [r] = derive([clip()]);
    if (!r) throw new Error("fixture");
    expect(matchQuery(r, "br31")).toBe(true);
    expect(matchQuery(r, "冬季手袋")).toBe(true);
    expect(matchQuery(r, "推近")).toBe(true);
    expect(matchQuery(r, "sub-1")).toBe(true);
    expect(matchQuery(r, "不存在的东西")).toBe(false);
  });
});

describe("分节", () => {
  /**
   * 分组维度从批次改成了**通道**（0032）。
   *
   * 即梦按模型通道各排各的队，而一个批次的条目会分散到不同通道上 —— 于是按批次分节时
   * 每一个节内数字都不指向任何真实的队列，「全选本节」选出来的也是一堆跨通道的条目，
   * 对它们做批量动作（尤其是换通道）根本不成立。
   */
  it("按通道分节：同通道归一节，跨通道拆开", () => {
    const rows = derive([
      clip({ id: 1, modelVersion: "seedance2.0fast", stage: "rev" }),
      clip({ id: 2, modelVersion: "seedance2.0fast_vip", stage: "rev" }),
      clip({ id: 3, modelVersion: "seedance2.0fast", stage: "rev" }),
    ]);
    const secs = buildSections(rows, rows, MODELS);
    expect(new Set(secs.map((s) => s.key))).toEqual(
      new Set(["seedance2.0fast", "seedance2.0fast_vip"]),
    );
    expect(secs.find((s) => s.key === "seedance2.0fast")?.all).toHaveLength(2);
    // 简写来自 Rust 下发的 `ModelInfo.label`，不在前端另判一次后缀。
    expect(secs.find((s) => s.key === "seedance2.0fast_vip")?.label).toBe("2.0Fast VIP");
    expect(secs.find((s) => s.key === "seedance2.0fast_vip")?.vip).toBe(true);
  });

  /**
   * 没写 `model_version` 的条目归**默认通道**那一节 —— 那本来就是它们会走的通道
   * （`channelOf` 与 Rust 的 `runner::channel_of` 同口径）。另设一个「未定通道」节
   * 会把同一条真实队列在界面上劈成两半，而「全选本节 → 换通道」就只换得到一半。
   */
  it("没写 model_version 的条目与默认通道的条目落在同一节", () => {
    const rows = derive([
      clip({ id: 1, modelVersion: null, stage: "rewrite" }),
      clip({ id: 2, modelVersion: "seedance2.0fast", stage: "rev" }),
    ]);
    const secs = buildSections(rows, rows, MODELS);
    expect(secs).toHaveLength(1);
    expect(secs[0]?.key).toBe("seedance2.0fast");
    expect(secs[0]?.all).toHaveLength(2);
  });

  // v0.20.0 起语义就是：已定案的不再折叠成一行，而是**整节消失**。
  // 折叠一行也是一行 —— 做完几十条之后，那些「已定案」的行会把真正在跑的挤到屏幕
  // 外面去，而工作台要答的恰恰是「还剩多少活」。
  it("全部落在 pass/rej 的通道整节消失", () => {
    const rows = derive([
      clip({ id: 1, modelVersion: "seedance2.0fast", stage: "rev" }),
      clip({ id: 2, modelVersion: "seedance2.0fast_vip", stage: "pass" }),
      clip({ id: 3, modelVersion: "seedance2.0fast_vip", stage: "rej" }),
    ]);
    // 工作台的可见集不含已定案的两态（`matchAction(_, "all")` 只放行在制）。
    const visible = rows.filter((r) => matchAction(r, "all"));
    const secs = buildSections(rows, visible, MODELS);
    expect(secs.map((s) => s.key)).toEqual(["seedance2.0fast"]);
    expect(secs[0]?.done).toBe(false);
  });

  /**
   * v0.22.0 起这条语义反转了：**当前筛选下一条都不显示的整节消失**，无论它是否还有活。
   *
   * 旧规则只砍已定案的空节，于是筛「处理异常」时会留下一排只写着「当前筛选下没有
   * 条目」的空壳节头 —— 用户那句「筛选项随便选一个都会保留每一个分组」说的就是它。
   */
  it("当前筛选下没有可见行的通道整节消失 —— 哪怕它还有活", () => {
    const rows = derive([
      clip({ id: 1, modelVersion: "seedance2.0fast", stage: "rev" }),
      clip({ id: 2, modelVersion: "seedance2.0fast_vip", stage: "rej" }),
    ]);
    const secs = buildSections(
      rows,
      rows.filter((r) => matchAction(r, "rej")),
      MODELS,
    );
    // 只剩 vip（它有命中的行）。2.0fast 还有活，但这一屏里它一行都没有。
    expect(secs.map((s) => s.key)).toEqual(["seedance2.0fast_vip"]);
    expect(secs[0]?.done).toBe(true);
  });

  /**
   * 通道之间没有批次那样天然的时间序（id 倒序），唯一有意义的先后是「哪条还有账要算」：
   * 远端在跑 > 本地压着队 > 其余。
   */
  it("有远端在跑的通道排在只压着本地队列的前面", () => {
    const rows = derive([
      clip({
        id: 1,
        modelVersion: "seedance2.0fast",
        stage: "ready",
        submitQueuedAt: NOW - 100,
        videoPath: null,
      }),
      clip({
        id: 2,
        modelVersion: "seedance2.0fast_vip",
        stage: "run",
        videoPath: null,
        firstSubmittedAt: NOW - 100,
        submitCredit: 44,
      }),
    ]);
    expect(buildSections(rows, rows, MODELS).map((s) => s.key)).toEqual([
      "seedance2.0fast_vip",
      "seedance2.0fast",
    ]);
  });

  // 节头摘要取代了那条无图例的分段条 —— 用户问「每个分组这些进度条是什么意思」，
  // 而它唯一的图例是 title tooltip，即：没人答得上来。
  it("节头摘要例外在前 —— 被截断时先没的必须是常态，不是例外", () => {
    const rows = derive([
      clip({
        id: 1,
        stage: "run",
        videoPath: null,
        submitCredit: 8,
        firstSubmittedAt: NOW - 60,
      }),
      clip({ id: 2, stage: "rev" }),
      clip({ id: 3, stage: "ready", videoPath: null, submitId: null }),
      clip({ id: 4, stage: "fail", errorType: "timeout", videoPath: null }),
    ]);
    const sec = buildSections(rows, rows, MODELS)[0];
    // 「这一批 N 条」那个前缀去掉了（0032）：一节是一条通道，里面的条目来自若干个批次。
    expect(sec?.headline).toBe("共 4 条 · 1 条出了异常，1 条等你放行，1 条等你验收，1 条在即梦跑");
    expect(sec?.headlineTone).toBe("er");
  });

  it("全定案的通道摘要直说定案，不假装还有活", () => {
    const rows = derive([clip({ id: 1, stage: "pass" }), clip({ id: 2, stage: "rej" })]);
    const sec = buildSections(rows, rows, MODELS)[0];
    expect(sec?.headline).toBe("共 2 条 · 已全部定案");
    expect(sec?.headlineTone).toBe("t3");
    expect(sec?.counts.done).toBe(2);
  });

  it("分节标题取组名；混多组时只列前两个", () => {
    const rows = derive([
      clip({ id: 1, groupName: "甲组" }),
      clip({ id: 2, groupName: "乙组" }),
      clip({ id: 3, groupName: "丙组" }),
    ]);
    expect(buildSections(rows, rows, MODELS)[0]?.title).toBe("甲组 · 乙组 等 3 组");
  });

  it("筛选只影响列出来的行，节头摘要仍按全貌算 —— 否则「这条队做到哪了」当场失真", () => {
    const rows = derive([clip({ id: 1, stage: "rev" }), clip({ id: 2, stage: "pass" })]);
    const visible = rows.filter((r) => r.stage === "rev");
    const sec = buildSections(rows, visible, MODELS)[0];
    expect(sec?.rows).toHaveLength(1);
    expect(sec?.all).toHaveLength(2);
    // 摘要说的是这条通道的全貌（2 条），不是这一屏筛出来的 1 条。
    expect(sec?.headline).toBe("共 2 条 · 1 条等你验收");
    expect(sec?.counts.done).toBe(1);
  });
});

describe("换通道时的参数带过去（carryParams）", () => {
  /**
   * 这一组守的是一件会**静默改值**的事。
   *
   * Rust 的 `normalize_opts` 在只给模型时会把时长补成该模型的最小值、分辨率补成第一档
   * —— 于是「换条队」这个动作会顺便把 1080p/10s 降成 720p/4s，而界面上一个字都不说。
   * 「我明明选了 1080p 却不生效」正是这么来的。
   */
  const WIDE: ModelInfo = {
    modelVersion: "seedance1.5pro",
    label: "1.5Pro",
    minDuration: 5,
    maxDuration: 10,
    resolutions: ["720p", "1080p"],
    creditAtMin: 20,
    resPrices: [
      { resolution: "720p", creditPerSec: 2 },
      { resolution: "1080p", creditPerSec: 4 },
    ],
    vip: false,
  };

  it("目标通道接得住就原样带过去，不报「改过」", () => {
    const c = carryParams(WIDE, 8, "1080p");
    expect(c).toEqual({
      duration: 8,
      resolution: "1080p",
      durationChanged: false,
      resolutionChanged: false,
    });
  });

  it("超出时长区间就夹到边界，并把夹过的事实报出去", () => {
    expect(carryParams(WIDE, 15, "720p")).toMatchObject({ duration: 10, durationChanged: true });
    expect(carryParams(WIDE, 2, "720p")).toMatchObject({ duration: 5, durationChanged: true });
  });

  it("目标通道不支持的分辨率退回第一档，并报「改过」", () => {
    // MODELS[0] 只有 720p。
    const c = carryParams(MODELS[0], 6, "1080p");
    expect(c).toMatchObject({ resolution: "720p", resolutionChanged: true });
  });

  it("原本就没设过参数时不算「改过」—— 那是回落，不是被夹", () => {
    const c = carryParams(WIDE, null, "");
    expect(c).toEqual({
      duration: 5,
      resolution: "720p",
      durationChanged: false,
      resolutionChanged: false,
    });
  });
});
