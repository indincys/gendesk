import {
  PHANTOM_GRACE_SECS,
  buildSections,
  deriveRows,
  matchQuery,
  matchStage,
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
    inAssetLib: false,
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
    expect(r?.situation).toContain("排队中");
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

  it("未入资产库只对成片成立，且入库后消失", () => {
    const [out] = derive([clip({ stage: "pass", inAssetLib: false })]);
    expect(out?.signals.has("noasset")).toBe(true);
    expect(out?.situation).toContain("尚未入库");
    const [inLib] = derive([clip({ stage: "pass", inAssetLib: true })]);
    expect(inLib?.signals.has("noasset")).toBe(false);
    expect(inLib?.situation).toBe("已入资产库");
  });

  it("重跑过 = 尝试次数 > 1（同一张图已经花过不止一份额度）", () => {
    expect(derive([clip({ attempt: 2 })])[0]?.signals.has("rerun")).toBe(true);
    expect(derive([clip({ attempt: 1 })])[0]?.signals.has("rerun")).toBe(false);
  });
});

describe("筛选", () => {
  it("「需要我」= 待提交 / 待验收 / 失败，不含机器在跑与等 skill 的", () => {
    expect(matchStage("ready", "need")).toBe(true);
    expect(matchStage("rev", "need")).toBe(true);
    expect(matchStage("fail", "need")).toBe(true);
    expect(matchStage("run", "need")).toBe(false);
    expect(matchStage("rewrite", "need")).toBe(false);
    expect(matchStage("pass", "need")).toBe(false);
  });

  // 工作台回答的是「还剩多少活」。把已经定案的算进去，这个数就再也不准了 ——
  // 实测 18 条验收通过的片子一直挂在看板上，人得先在心里把它们减掉才看得出待办。
  it("「全部」= 全部在制，不含成片与未通过", () => {
    expect(matchStage("rewrite", "all")).toBe(true);
    expect(matchStage("run", "all")).toBe(true);
    expect(matchStage("fail", "all")).toBe(true);
    expect(matchStage("pass", "all")).toBe(false);
    expect(matchStage("rej", "all")).toBe(false);
    // 但显式点「未通过」还是能看到它们 —— 不是删掉，是不默认占位。
    expect(matchStage("rej", "rej")).toBe(true);
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
    // 工作台的可见集不含已定案的两态（`matchStage(_, "all")` 只放行在制）。
    const visible = rows.filter((r) => matchStage(r.stage, "all"));
    const secs = buildSections(rows, visible);
    expect(secs.map((s) => s.batchId)).toEqual([31]);
    expect(secs[0]?.done).toBe(false);
  });

  it("但显式筛「未通过」时那一节要回来 —— 定案的条目不该变得无处可寻", () => {
    const rows = derive([
      clip({ id: 1, batchId: 31, stage: "rev" }),
      clip({ id: 2, batchId: 30, stage: "rej" }),
    ]);
    const visible = rows.filter((r) => matchStage(r.stage, "rej"));
    const secs = buildSections(rows, visible);
    // 30 回来了（它有命中的行）；31 仍在（它还有活），只是这一屏里没有它的行。
    // 「还有活的批次一直在」是刻意的：那条分段条正是「这一批做到哪了」的答案，
    // 不该因为换了个筛选就消失。
    expect(secs.map((s) => s.batchId)).toEqual([31, 30]);
    expect(secs.find((s) => s.batchId === 30)?.done).toBe(true);
    expect(secs.find((s) => s.batchId === 31)?.rows).toHaveLength(0);
  });

  it("无批次的历史条目垫底，不占最新那一节的位置", () => {
    const rows = derive([
      clip({ id: 1, batchId: null, stage: "rewrite" }),
      clip({ id: 2, batchId: 12, stage: "rev" }),
    ]);
    expect(buildSections(rows, rows).map((s) => s.batchId)).toEqual([12, null]);
  });

  it("分段条按七态固定顺序，且百分比合计 100", () => {
    const rows = derive([
      clip({ id: 1, batchId: 31, stage: "pass" }),
      clip({ id: 2, batchId: 31, stage: "rewrite" }),
      clip({ id: 3, batchId: 31, stage: "run", videoPath: null }),
      clip({ id: 4, batchId: 31, stage: "rewrite" }),
    ]);
    const seg = buildSections(rows, rows)[0]?.seg ?? [];
    // 固定顺序，不随「哪一条先变状态」重排 —— 否则同一批的色块会看着像换了一批。
    expect(seg.map((g) => g.stage)).toEqual(["rewrite", "run", "pass"]);
    expect(seg.reduce((a, g) => a + g.pct, 0)).toBeCloseTo(100);
  });

  it("分节标题取组名；混多组时只列前两个", () => {
    const rows = derive([
      clip({ id: 1, batchId: 7, groupName: "甲组" }),
      clip({ id: 2, batchId: 7, groupName: "乙组" }),
      clip({ id: 3, batchId: 7, groupName: "丙组" }),
    ]);
    expect(buildSections(rows, rows)[0]?.title).toBe("甲组 · 乙组 等 3 组");
  });

  it("筛选只影响列出来的行，分段条仍按全貌算 —— 否则「这一批做到哪了」当场失真", () => {
    const rows = derive([
      clip({ id: 1, batchId: 31, stage: "rev" }),
      clip({ id: 2, batchId: 31, stage: "pass" }),
    ]);
    const visible = rows.filter((r) => r.stage === "rev");
    const sec = buildSections(rows, visible)[0];
    expect(sec?.rows).toHaveLength(1);
    expect(sec?.all).toHaveLength(2);
    expect(sec?.seg).toHaveLength(2);
  });
});
