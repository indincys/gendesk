import {
  type ActionFilter,
  PHANTOM_GRACE_SECS,
  type Row,
  buildSections,
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
    minDuration: 4,
    maxDuration: 15,
    resolutions: ["720p"],
    creditAtMin: 8,
    resPrices: [{ resolution: "720p", creditPerSec: 2 }],
    vip: false,
  },
  {
    modelVersion: "seedance2.0fast_vip",
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
    assetPackId: null,
    exportPath: null,
    acceptedAt: NOW - 8000,
    updatedAt: NOW - 600,
    ...over,
  };
}

const derive = (cs: ClipView[]) => deriveRows(cs, MODELS, EFF, NOW);

describe("幽灵单判定", () => {
  // 与 Rust 侧 `runner::is_phantom` 同构：两个信号**同时**缺席，且过了宽限期。
  it("队列位次与扣费回执双双缺席、且过了宽限期才算", () => {
    const [r] = derive([
      clip({
        stage: "run",
        videoPath: null,
        firstSubmittedAt: NOW - PHANTOM_GRACE_SECS - 60,
      }),
    ]);
    expect(r?.phantomLive).toBe(true);
    expect(r?.signals.has("phantom")).toBe(true);
    expect(r?.situation).toContain("疑幽灵单");
    expect(r?.credit).toBe(0);
  });

  it("宽限期内不判 —— 实测健康单 25 秒内才拿到位次，早判会把正常单说成事故", () => {
    const [r] = derive([clip({ stage: "run", videoPath: null, firstSubmittedAt: NOW - 60 })]);
    expect(r?.phantomLive).toBe(false);
    expect(r?.situation).toContain("即梦在跑");
  });

  it("有扣费回执就不是幽灵单 —— 那条是决定性的信号，钱已经扣了", () => {
    const [r] = derive([
      clip({
        stage: "run",
        videoPath: null,
        submitCredit: 8,
        firstSubmittedAt: NOW - PHANTOM_GRACE_SECS - 3600,
      }),
    ]);
    expect(r?.phantomLive).toBe(false);
  });

  it("有队列位次也不算 —— 只看一个信号会在即梦不下发 queue_info 时误判", () => {
    const [r] = derive([
      clip({
        stage: "run",
        videoPath: null,
        queueIdx: 4485,
        firstSubmittedAt: NOW - PHANTOM_GRACE_SECS - 3600,
      }),
    ]);
    expect(r?.phantomLive).toBe(false);
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
    expect(vip?.modelShort).toBe("2.0fast_vip");
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
        firstSubmittedAt: NOW - PHANTOM_GRACE_SECS - 60,
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
  // v0.20.0 起语义变了：已定案的批次不再折叠成一行，而是**整节消失**。
  // 折叠一行也是一行 —— 几十批做完之后，那些「已定案」的行会把真正在跑的两批
  // 挤到屏幕外面去，而工作台要答的恰恰是「还剩多少活」。
  it("按批次倒序；全部落在 pass/rej 的批次整节消失", () => {
    const rows = derive([
      clip({ id: 1, batchId: 31, stage: "rev" }),
      clip({ id: 2, batchId: 30, stage: "pass" }),
      clip({ id: 3, batchId: 30, stage: "rej" }),
    ]);
    // 工作台的可见集不含已定案的两态（`matchAction(_, "all")` 只放行在制）。
    const visible = rows.filter((r) => matchAction(r, "all"));
    const secs = buildSections(rows, visible);
    expect(secs.map((s) => s.batchId)).toEqual([31]);
    expect(secs[0]?.done).toBe(false);
  });

  /**
   * v0.22.0 起这条语义反转了：**当前筛选下一条都不显示的批次整节消失**，
   * 无论它是否还有活。
   *
   * 旧规则只砍已定案的空节，于是筛「处理异常」时几十个还在跑的批次留下几十个
   * 只写着「当前筛选下这一批没有条目」的空壳节头 —— 用户那句「筛选项随便选一个
   * 都会保留每一个分组」说的就是它。旧规则的理由是「分段条正是这一批做到哪了的
   * 答案，空节也该留着」；分段条已经删了，理由也就没了。
   */
  it("当前筛选下没有可见行的批次整节消失 —— 哪怕它还有活", () => {
    const rows = derive([
      clip({ id: 1, batchId: 31, stage: "rev" }),
      clip({ id: 2, batchId: 30, stage: "rej" }),
    ]);
    const secs = buildSections(
      rows,
      rows.filter((r) => matchAction(r, "rej")),
    );
    // 只剩 30（它有命中的行）。31 还有活，但这一屏里它一行都没有 —— 留一个空壳
    // 节头不回答任何问题，只会把真正命中的那一节挤下去。
    expect(secs.map((s) => s.batchId)).toEqual([30]);
    expect(secs[0]?.done).toBe(true);
  });

  it("无批次的历史条目垫底，不占最新那一节的位置", () => {
    const rows = derive([
      clip({ id: 1, batchId: null, stage: "rewrite" }),
      clip({ id: 2, batchId: 12, stage: "rev" }),
    ]);
    expect(buildSections(rows, rows).map((s) => s.batchId)).toEqual([12, null]);
  });

  // 节头摘要取代了那条无图例的分段条 —— 用户问「每个分组这些进度条是什么意思」，
  // 而它唯一的图例是 title tooltip，即：没人答得上来。
  it("节头摘要例外在前 —— 被截断时先没的必须是常态，不是例外", () => {
    const rows = derive([
      clip({
        id: 1,
        batchId: 31,
        stage: "run",
        videoPath: null,
        submitCredit: 8,
        firstSubmittedAt: NOW - 60,
      }),
      clip({ id: 2, batchId: 31, stage: "rev" }),
      clip({ id: 3, batchId: 31, stage: "ready", videoPath: null, submitId: null }),
      clip({ id: 4, batchId: 31, stage: "fail", errorType: "timeout", videoPath: null }),
    ]);
    const sec = buildSections(rows, rows)[0];
    expect(sec?.headline).toBe(
      "这一批 4 条 · 1 条出了异常，1 条等你放行，1 条等你验收，1 条在即梦排队",
    );
    expect(sec?.headlineTone).toBe("er");
  });

  it("全定案的批次摘要直说定案，不假装还有活", () => {
    const rows = derive([
      clip({ id: 1, batchId: 31, stage: "pass" }),
      clip({ id: 2, batchId: 31, stage: "rej" }),
    ]);
    const sec = buildSections(rows, rows)[0];
    expect(sec?.headline).toBe("这一批 2 条 · 已全部定案");
    expect(sec?.headlineTone).toBe("t3");
    expect(sec?.counts.done).toBe(2);
  });

  it("分节标题取组名；混多组时只列前两个", () => {
    const rows = derive([
      clip({ id: 1, batchId: 7, groupName: "甲组" }),
      clip({ id: 2, batchId: 7, groupName: "乙组" }),
      clip({ id: 3, batchId: 7, groupName: "丙组" }),
    ]);
    expect(buildSections(rows, rows)[0]?.title).toBe("甲组 · 乙组 等 3 组");
  });

  it("筛选只影响列出来的行，节头摘要仍按全貌算 —— 否则「这一批做到哪了」当场失真", () => {
    const rows = derive([
      clip({ id: 1, batchId: 31, stage: "rev" }),
      clip({ id: 2, batchId: 31, stage: "pass" }),
    ]);
    const visible = rows.filter((r) => r.stage === "rev");
    const sec = buildSections(rows, visible)[0];
    expect(sec?.rows).toHaveLength(1);
    expect(sec?.all).toHaveLength(2);
    // 摘要说的是这一批的全貌（2 条），不是这一屏筛出来的 1 条。
    expect(sec?.headline).toBe("这一批 2 条 · 1 条等你验收");
    expect(sec?.counts.done).toBe(1);
  });
});
