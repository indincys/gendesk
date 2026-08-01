import { Modal } from "@/components/ui/Modal";
import { Stepper } from "@/components/ui/Stepper";
import { ImportPreviewModal } from "@/features/_shared/ImportPreviewModal";
import { RefImportOverlay, useRefImport } from "@/features/_shared/RefImport";
import { assetSrc } from "@/lib/img";
import {
  type ApiKeyView,
  type GroupView,
  type ImportPreview,
  type ProductionOverview,
  type PromptView,
  type RefImageView,
  commands,
  subscribeFileDrop,
  unwrap,
} from "@/lib/ipc";
import { cn, promptLabel, sortGroupsByPinyin } from "@/lib/utils";
import { useEngineStore } from "@/stores/engine";
import { useGenerateStore } from "@/stores/generate";
import { useUiStore } from "@/stores/ui";
import { Check, ChevronDown, FileUp, Play, Plus, Upload, X } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { toast } from "sonner";

/** 分组配色板（gc0–gc4，循环）；按分组在已选序列中的位置分配，稳定可预期。 */
const GC = ["gc0", "gc1", "gc2", "gc3", "gc4"] as const;

/** 按扩展名分拣拖入路径（E14）。 */
function sortDrops(paths: string[]): { txt: string | undefined; images: string[] } {
  const images = paths.filter((p) => /\.(png|jpe?g|webp|bmp)$/i.test(p));
  const txt = paths.find((p) => p.toLowerCase().endsWith(".txt"));
  return { txt, images };
}

export function GeneratePage() {
  const go = useUiStore((s) => s.go);
  const loadBatchTasks = useEngineStore((s) => s.loadBatchTasks);

  const [groups, setGroups] = useState<GroupView[]>([]);
  const [refs, setRefs] = useState<RefImageView[]>([]);
  // E07：选择态持久化（切页/重启沿用）。
  const selGroupIds = useGenerateStore((s) => s.selGroupIds);
  const setSelGroupIds = useGenerateStore((s) => s.setSelGroupIds);
  const selRefIds = useGenerateStore((s) => s.selRefIds);
  const setSelRefIds = useGenerateStore((s) => s.setSelRefIds);
  const mapping = useGenerateStore((s) => s.mapping);
  const setMapping = useGenerateStore((s) => s.setMapping);
  // E16 / D1：生成参数，null = 未设置（不传该参数，跟随提示词）。
  // 只保留真的会用到的三项：比例（画幅首选，size 只有部分模型认）+ 精确尺寸 + 输出格式。
  const aspectRatio = useGenerateStore((s) => s.aspectRatio);
  const setAspectRatio = useGenerateStore((s) => s.setAspectRatio);
  const size = useGenerateStore((s) => s.size);
  const setSize = useGenerateStore((s) => s.setSize);
  const outputFormat = useGenerateStore((s) => s.outputFormat);
  const setOutputFormat = useGenerateStore((s) => s.setOutputFormat);
  // E17 / D2：抽卡次数 k（每组合独立生成 k 次），默认 1，上限 5。
  const draws = useGenerateStore((s) => s.draws);
  const setDraws = useGenerateStore((s) => s.setDraws);
  // 任务1 输出处理：去水印档位 + 清除 AI 元数据 + 去除 C2PA。
  const watermark = useGenerateStore((s) => s.watermark);
  const setWatermark = useGenerateStore((s) => s.setWatermark);
  const clearAiMetadata = useGenerateStore((s) => s.clearAiMetadata);
  const setClearAiMetadata = useGenerateStore((s) => s.setClearAiMetadata);
  const removeC2pa = useGenerateStore((s) => s.removeC2pa);
  const setRemoveC2pa = useGenerateStore((s) => s.setRemoveC2pa);
  // E31：启用 Key 快照（确认摘要展示 + ETA 估算并发）。
  const [keys, setKeys] = useState<ApiKeyView[]>([]);
  // E31：开始生成确认卡（null = 未打开）。
  const [confirm, setConfirm] = useState<null | { avgSec: number | null }>(null);
  const [modal, setModal] = useState<null | "refs">(null);
  const [starting, setStarting] = useState(false);
  // E14：生成页 txt 导入改走预览确认（取消不落库、不产生临时分组）。
  const [importPreview, setImportPreview] = useState<ImportPreview | null>(null);
  // E25：今日生产总览。
  const [overview, setOverview] = useState<ProductionOverview | null>(null);
  // 分组内提示词缓存（渲染词条 chip + 原文弹窗，按需加载）。
  const [promptsByGroup, setPromptsByGroup] = useState<Record<number, PromptView[]>>({});

  // 就地交互态（不落库）：展开的词组、悬停联动、拖拽、挂靠弹层、参数弹层、原文弹窗。
  const [expG, setExpG] = useState<number | null>(null);
  const [hovG, setHovG] = useState<number | null>(null);
  const [hovR, setHovR] = useState<number | null>(null);
  const [drag, setDrag] = useState<number | null>(null);
  const [apop, setApop] = useState<number | null>(null);
  const [paramPop, setParamPop] = useState(false);
  const [viewer, setViewer] = useState<null | { gid: number; index: number }>(null);
  // 上传进度（生成页上传的图为「临时上传」，不进参考图库）。
  const { state: uploading, run: runImport } = useRefImport();

  const load = useCallback(async () => {
    try {
      // 提示词组默认按拼音首字母排序：左栏词组卡、右侧参考图挂靠弹层、
      // 「选择提示词组」弹窗均沿用此序（配色仍按选中先后，稳定不乱）。
      setGroups(sortGroupsByPinyin(await unwrap(commands.listPromptGroups())));
      // 生成页是唯一要看见临时上传的地方：人刚拖进来的那几张就在里面。
      setRefs(await unwrap(commands.listRefImages(true)));
      setKeys(await unwrap(commands.listApiKeys()));
      setOverview(await unwrap(commands.productionOverview()).catch(() => null));
    } catch (e) {
      if (e instanceof Error) toast.error(e.message);
    }
  }, []);
  useEffect(() => {
    void load();
  }, [load]);

  // 词条 chip 需要各已选分组的提示词：按需拉取尚未缓存的分组。
  useEffect(() => {
    for (const gid of selGroupIds) {
      if (promptsByGroup[gid]) continue;
      unwrap(commands.listPrompts(gid))
        .then((ps) => setPromptsByGroup((m) => (m[gid] ? m : { ...m, [gid]: ps })))
        .catch((e) => {
          if (e instanceof Error) toast.error(e.message);
        });
    }
  }, [selGroupIds, promptsByGroup]);

  // E32 挂靠记忆：选中的参考图若尚未挂靠、且其上次挂靠组在本次已选分组中，自动预填（可手动改）。
  useEffect(() => {
    if (refs.length === 0) return;
    setMapping((m) => {
      let changed = false;
      const next = { ...m };
      for (const rid of selRefIds) {
        if (next[rid] != null) continue;
        const last = refs.find((r) => r.id === rid)?.lastGroupId;
        if (last != null && selGroupIds.includes(last)) {
          next[rid] = last;
          changed = true;
        }
      }
      return changed ? next : m;
    });
  }, [selRefIds, selGroupIds, refs, setMapping]);

  const selGroups = groups.filter((g) => selGroupIds.includes(g.id));
  const selRefs = refs.filter((r) => selRefIds.includes(r.id));
  const allMapped = selRefs.length > 0 && selRefs.every((r) => mapping[r.id] != null);
  // 组合数 = Σ(参考图 × 挂靠组提示词数)；任务总数 = 组合数 × 抽卡次数（D2）。
  const combos = selRefs.reduce((sum, r) => {
    const g = groups.find((x) => x.id === mapping[r.id]);
    return sum + (g?.count ?? 0);
  }, 0);
  const taskTotal = combos * draws;
  const enabledKeys = keys.filter((k) => k.enabled);
  // E12：有可用 Key = 至少一个启用且并发合计 > 0。
  const hasUsableKey = enabledKeys.reduce((s, k) => s + k.concurrencyLimit, 0) > 0;

  /** 分组配色类：按其在已选序列中的下标循环取色（未选中回退主强调色）。 */
  const ccOf = (gid: number): string => {
    const i = selGroupIds.indexOf(gid);
    return i >= 0 ? (GC[i % GC.length] ?? "gc0") : "gc0";
  };

  // E14：解析 txt → 预览确认（不落库）；确认后才 commit 为临时分组。
  const parseTxt = useCallback(async (path: string) => {
    const preview = await unwrap(commands.parsePromptTxt(path)).catch((e) => {
      toast.error(String(e));
      return null;
    });
    if (preview) setImportPreview(preview);
  }, []);

  const importTxt = async () => {
    const path = await unwrap(commands.pickTxtFile()).catch(() => null);
    if (!path) return;
    await parseTxt(path);
  };

  // edited = 用户在预览弹窗里改过分组/条目后的最终态。
  const confirmImport = async (edited: ImportPreview) => {
    const res = await unwrap(commands.commitPromptImport(edited, "generate")).catch((e) => {
      toast.error(String(e));
      return null;
    });
    if (res) {
      toast(`已导入 ${res.inserted} 条到临时分组`);
      setImportPreview(null);
      await load();
      setSelGroupIds((cur) => [...new Set([...cur, ...res.groupIds])]);
    }
  };

  // 生成页上传 = **临时上传**（ephemeral，0019）：只作本批附件，不进长期参考图库。
  // 参考图库是长期资产库，有它自己的批量上传入口；随手拖一张来跑一次的图不该长住那里。
  const importImages = useCallback(
    async (paths: string[]) => {
      if (paths.length === 0) return;
      try {
        const added = await runImport(paths, null, true);
        if (added.length === 0) return;
        const skipped = paths.length - added.length;
        toast(
          skipped > 0
            ? `已上传 ${added.length} 张（${skipped} 张读取失败已跳过）· 本批临时使用`
            : `已上传 ${added.length} 张 · 本批临时使用，不进参考图库`,
        );
        await load();
        setSelRefIds((cur) => [...new Set([...cur, ...added.map((r) => r.id)])]);
      } catch (e) {
        toast.error(e instanceof Error ? e.message : String(e));
      }
    },
    [load, setSelRefIds, runImport],
  );

  const uploadRefs = async () => {
    const paths = await unwrap(commands.pickImageFiles()).catch(() => []);
    await importImages(paths);
  };

  // E14：拖拽——txt 走预览确认，图片直接导入为参考图。
  useEffect(() => {
    let un = () => {};
    void subscribeFileDrop((paths) => {
      const { txt, images } = sortDrops(paths);
      if (txt) void parseTxt(txt);
      if (images.length > 0) void importImages(images);
      if (!txt && images.length === 0 && paths.length > 0)
        toast.error("仅支持拖入 .txt 或图片文件");
    }).then((f) => {
      un = f;
    });
    return () => un();
  }, [parseTxt, importImages]);

  // 仅收集显式设置的参数（D1：未设置的键不出现在 JSON 中 → provider 不透传）。
  //
  // 这些是**会发到远端**的键（Rust GenParams 认它们，键名对齐端点文档的参数表）。
  // 确认卡与参数弹层里的「实际发往接口的字段」直接由它渲染——展示与执行同一来源，
  // 「我设了却没生效」这类怀疑才无处可生。
  //
  // 每项 = [multipart 字段名, 快照键名, 值]。快照键名是 camelCase（Rust serde 配置）。
  // 快照里另有一个 `watermark` 是**本地**去水印档位，不是远端参数，故不在此表。
  const wireEntries: [string, string, unknown][] = (
    [
      ["aspect_ratio", "aspectRatio", aspectRatio],
      ["size", "size", size],
      ["output_format", "outputFormat", outputFormat],
    ] as [string, string, unknown][]
  ).filter(([, , v]) => v != null && v !== "");
  const wire = Object.fromEntries(wireEntries.map(([w, , v]) => [w, v]));

  const buildParamsJson = () => {
    const p: Record<string, unknown> = {};
    for (const [, snapKey, v] of wireEntries) p[snapKey] = v;
    // 任务1：输出处理开关随批次记忆（后端缺省视为开启，故显式写入以便「再来一批」还原）。
    p.watermark = watermark;
    p.clearAiMetadata = clearAiMetadata;
    p.removeC2pa = removeC2pa;
    // 抽卡次数是 createBatch 的独立入参、不进 GenParams；但不写进快照，
    // 「再来一批」就会把 ×3 悄悄还原成 ×1，任务数对不上而没人知道为什么。
    p.draws = draws;
    return JSON.stringify(p);
  };

  // E31：点「开始生成」先拉 ETA 均值并弹确认卡（不直接建批次）。
  const openConfirm = async () => {
    const avgSec = await unwrap(commands.estimateTaskSeconds()).catch(() => null);
    setConfirm({ avgSec });
  };

  const start = async () => {
    setStarting(true);
    try {
      const batch = await unwrap(
        commands.createBatch({
          refs: selRefs.map((r) => ({ refImageId: r.id, promptGroupId: mapping[r.id] as number })),
          paramsJson: buildParamsJson(),
          draws,
        }),
      );
      setConfirm(null);
      // 批次已开始：清空提示词组区与图片挂靠区，回到空白起点（本批内容不再滞留）。
      setSelGroupIds([]);
      setSelRefIds([]);
      setMapping({});
      setExpG(null);
      setHovG(null);
      setHovR(null);
      // 后端已随批次同事务归档本批的组与图（0016）；重新拉一遍库，让选择器立刻反映归档态。
      void load();
      toast(`已创建批次 #${batch.id} · ${batch.taskCount} 个任务`);
      await loadBatchTasks(batch.id, null);
      go("tasks");
    } catch (e) {
      if (e instanceof Error) toast.error(e.message);
    } finally {
      setStarting(false);
    }
  };

  /** 移除某分组：同时清掉其挂靠。 */
  const removeGroup = (gid: number) => {
    setSelGroupIds((c) => c.filter((x) => x !== gid));
    setMapping((m) => {
      const next = { ...m };
      for (const r of Object.keys(next)) if (next[Number(r)] === gid) delete next[Number(r)];
      return next;
    });
    if (expG === gid) setExpG(null);
    if (hovG === gid) setHovG(null);
  };

  /** 挂靠某参考图到某分组（点选或拖放）。 */
  const assign = (rid: number, gid: number) => {
    setMapping((m) => ({ ...m, [rid]: gid }));
    setApop(null);
  };

  // 参数自检：把端点会拒的取值挡在花钱之前（后端 create_batch 有同规则的兜底）。
  const paramErr = sizeIssue(size);

  const fmtLabel = outputFormat === "png" ? "PNG" : outputFormat === "jpeg" ? "JPG" : "跟随";
  /**
   * 选比例即带上配套精确尺寸（见 `RATIO_SIZE` 的注释：单发 aspect_ratio 实测回正方形）。
   *
   * 只在尺寸「是自动填的」时才覆盖——空着，或正好等于上一个比例的配套值。用户手打过
   * 别的尺寸就不动它：那是他明确的意思，被下拉框静默改掉正是「我明明设了却不生效」的成因。
   */
  const pickRatio = (v: string | null) => {
    const wasAuto = size === null || (aspectRatio !== null && size === RATIO_SIZE[aspectRatio]);
    setAspectRatio(v);
    if (wasAuto) setSize(v === null ? null : (RATIO_SIZE[v] ?? null));
  };

  const paramShort = `${aspectRatio ?? size ?? "跟随"} · ${fmtLabel} · ×${draws}`;
  const paramSum = `比例 ${aspectRatio ?? "跟随提示词"}${size ? ` · 尺寸 ${size}` : ""} · 输出 ${fmtLabel} · 抽卡 ×${draws}`;

  return (
    <div className="col f1 ohide">
      <div className="phd">
        <span className="ptt">图片生成</span>
        {overview && (
          <div className="ovbar">
            <span>
              今日生成 <b>{overview.generatedToday}</b> 张
            </span>
            <span className="ovsep" />
            <span>
              通过率{" "}
              <b>
                {overview.generatedToday > 0
                  ? `${Math.round((overview.acceptedToday / overview.generatedToday) * 100)}%`
                  : "—"}
              </b>
            </span>
            <span className="ovsep" />
            <span>
              今日请求 <b>{overview.requestsToday}</b> 次
            </span>
          </div>
        )}
        <div className="f1" />
        <span className="pcap">点击词组展开 · 点图片或标签就地选择挂靠 · 拖拽同样有效</span>
      </div>

      {/* 挂靠弹层的点击外部关闭遮罩 */}
      {apop != null && <div className="pcl" onClick={() => setApop(null)} />}

      <div className="abody">
        {/* 左栏：提示词 */}
        <div className="acol">
          <div className="colhd">
            <span className="fw6 fs13">提示词</span>
            {selGroups.length > 0 && <span className="cnt">{selGroups.length} 组</span>}
            <div className="f1" />
            <button type="button" className="btn sm" onClick={importTxt}>
              <FileUp className="ic12" />
              导入 .txt
            </button>
          </div>
          <div className="colsc">
            {selGroups.length === 0 ? (
              <div className="empt">
                <div className="fs13 fw5 t2">尚未导入提示词</div>
                <div className="fs12 t3 mt4" style={{ lineHeight: 1.7 }}>
                  导入 .txt（或直接拖进来）作为本批提示词
                  <br />
                  提示词是消耗品：跑完并验收干净后随批次一起清掉，没有可回头挑选的历史库
                </div>
              </div>
            ) : (
              selGroups.map((g) => {
                const cc = ccOf(g.id);
                const nref = selRefs.filter((r) => mapping[r.id] === g.id).length;
                const hl = (hovR != null && mapping[hovR] === g.id) || hovG === g.id;
                const dim =
                  !hl &&
                  ((hovG != null && hovG !== g.id) ||
                    (hovR != null && mapping[hovR] != null && mapping[hovR] !== g.id));
                const exp = expG === g.id;
                const chips = promptsByGroup[g.id];
                return (
                  <div
                    key={g.id}
                    className={cn(
                      "gcard2",
                      cc,
                      hl && "hl",
                      dim && "dim",
                      drag === g.id && "drg",
                      exp && "exp",
                    )}
                    draggable
                    title="点击展开/收起 · 拖拽到参考图挂靠"
                    onClick={() => setExpG((c) => (c === g.id ? null : g.id))}
                    onMouseEnter={() => setHovG(g.id)}
                    onMouseLeave={() => setHovG(null)}
                    onDragStart={(e) => {
                      try {
                        e.dataTransfer.setData("text/plain", String(g.id));
                        e.dataTransfer.effectAllowed = "link";
                      } catch {}
                      setDrag(g.id);
                    }}
                    onDragEnd={() => setDrag(null)}
                  >
                    <div className="gchd">
                      <i className="gdot" style={{ background: "var(--gc)" }} />
                      <span className="fw6 fs13 nowrap ohide">{g.name}</span>
                      <span className="chip">{g.prefix}</span>
                      {g.skuCode && <span className="bdg b-green">SKU {g.skuCode}</span>}
                      {g.isTemp && <span className="bdg b-amber">临时</span>}
                      <div className="f1" />
                      <span className="grcnt">{nref > 0 ? `× ${nref} 图` : "未挂靠"}</span>
                      <span className="t3 fs11 nowrap">{g.count} 条</span>
                      <span className={cn("car", exp && "open")}>▾</span>
                      <button
                        type="button"
                        className="icb"
                        onClick={(e) => {
                          e.stopPropagation();
                          removeGroup(g.id);
                        }}
                      >
                        <X className="ic12" />
                      </button>
                    </div>
                    <div className={exp ? "cfopen" : "cfclip"}>
                      <div className="chipflow">
                        {chips ? (
                          chips.map((p, i) => (
                            <button
                              type="button"
                              key={p.id}
                              className="pch"
                              title={promptLabel(p.code, p.title)}
                              onClick={(e) => {
                                e.stopPropagation();
                                setViewer({ gid: g.id, index: i });
                              }}
                            >
                              {p.favorite && <i className="favdot" />}
                              {p.code}
                            </button>
                          ))
                        ) : (
                          <span className="ghint">加载词条…</span>
                        )}
                      </div>
                    </div>
                    <div className="ghint">
                      {exp
                        ? "点击词条弹窗查看原文 · 再次点击卡片收起"
                        : `点击卡片展开全部 ${g.count} 条 · 拖拽本卡到参考图挂靠`}
                    </div>
                  </div>
                );
              })
            )}
          </div>
        </div>

        {/* 右栏：参考图 */}
        <div className="acol">
          <div className="colhd">
            <span className="fw6 fs13">参考图</span>
            {selRefs.length > 0 && <span className="cnt">{selRefs.length} 张</span>}
            <div className="f1" />
            <button type="button" className="btn sm gho" onClick={uploadRefs}>
              <Upload className="ic12" />
              上传
            </button>
            <button type="button" className="btn sm" onClick={() => setModal("refs")}>
              <Plus className="ic12" />
              从参考图库选择
            </button>
          </div>
          <div className="colsc">
            {selRefs.length === 0 ? (
              <div className="empt">
                <div className="fs13 fw5 t2">尚未选择参考图</div>
                <div className="fs12 t3 mt4">
                  从参考图库选择，或上传新图；每张参考图将挂靠一个提示词组
                </div>
              </div>
            ) : (
              <div className="rgrid4">
                {selRefs.map((r) => {
                  const gid = mapping[r.id];
                  const g = gid != null ? groups.find((x) => x.id === gid) : undefined;
                  const cc = g ? ccOf(g.id) : "gc0";
                  const glow = hovG != null && gid === hovG;
                  const dimm = hovG != null && gid !== hovG;
                  const src = assetSrc(r.thumbPath);
                  return (
                    <div
                      key={r.id}
                      className={cn("rt", cc, apop === r.id && "pz")}
                      onMouseEnter={() => setHovR(r.id)}
                      onMouseLeave={() => setHovR(null)}
                      onDragOver={(e) => e.preventDefault()}
                      onDrop={(e) => {
                        e.preventDefault();
                        if (drag != null) {
                          assign(r.id, drag);
                          setDrag(null);
                        }
                      }}
                    >
                      <div
                        className={cn(
                          "rtimg",
                          !src && "ph",
                          g ? "mapped" : "need",
                          glow && "glow",
                          dimm && "dimm",
                          drag != null && "drop",
                        )}
                        style={
                          src
                            ? {
                                backgroundImage: `url(${src})`,
                                backgroundSize: "cover",
                                backgroundPosition: "center",
                              }
                            : undefined
                        }
                        onClick={(e) => {
                          e.stopPropagation();
                          setApop((c) => (c === r.id ? null : r.id));
                        }}
                      >
                        {!src && <span className="phl">参考图</span>}
                        {g && <span className="rtb">{g.prefix}</span>}
                        <button
                          type="button"
                          className="rtx"
                          onClick={(e) => {
                            e.stopPropagation();
                            setSelRefIds((c) => c.filter((x) => x !== r.id));
                            setMapping((m) => {
                              const next = { ...m };
                              delete next[r.id];
                              return next;
                            });
                          }}
                        >
                          <X className="ic12" />
                        </button>
                      </div>
                      <div className="rtname" title={r.name}>
                        {r.name}
                      </div>
                      <button
                        type="button"
                        className={cn("rtag", cc, !g && "tneed")}
                        onClick={(e) => {
                          e.stopPropagation();
                          setApop((c) => (c === r.id ? null : r.id));
                        }}
                      >
                        <span className="nowrap ohide">
                          {g ? `${g.prefix} · ${g.name}` : "指定提示词组"}
                        </span>
                        <ChevronDown className="ic12" />
                      </button>
                      {apop === r.id && (
                        <div className="apop">
                          {selGroups.length === 0 ? (
                            <div className="fs11 t3" style={{ padding: "6px 8px" }}>
                              先在左侧选择提示词组
                            </div>
                          ) : (
                            selGroups.map((g2) => (
                              <div
                                key={g2.id}
                                className="apopi"
                                onClick={(e) => {
                                  e.stopPropagation();
                                  assign(r.id, g2.id);
                                }}
                              >
                                <i
                                  className={cn("gdot", ccOf(g2.id))}
                                  style={{ background: "var(--gc)" }}
                                />
                                <span className="fw5 nowrap ohide f1">{g2.name}</span>
                                <span className="fs11 t3 nowrap">{g2.count} 条</span>
                                {mapping[r.id] === g2.id && (
                                  <Check className="ic12" style={{ color: "var(--acc2)" }} />
                                )}
                              </div>
                            ))
                          )}
                        </div>
                      )}
                    </div>
                  );
                })}
              </div>
            )}
          </div>
        </div>
      </div>

      {/* 底部生成条 */}
      <div className="gb2">
        <div className="fchips">
          {selRefs.slice(0, 5).map((r) => {
            const g = groups.find((x) => x.id === mapping[r.id]);
            return (
              <span key={r.id} className={cn("fchip", !g && "warn")}>
                {r.name} × {g ? `${g.prefix}(${g.count})` : "?"}
              </span>
            );
          })}
          {selRefs.length > 5 && <span className="fchip">+{selRefs.length - 5}</span>}
          {combos > 0 && (
            <>
              <span className="t3 fs12">=</span>
              <span className="fw6 fs13 nowrap" style={{ color: "var(--acc2)" }}>
                {draws > 1
                  ? `${combos} 组合 × ${draws} 抽卡 = ${taskTotal} 个任务`
                  : `${combos} 组合 = ${taskTotal} 个任务`}
              </span>
            </>
          )}
        </div>
        {selRefs.length > 0 && !allMapped && (
          <span className="fs12 nowrap" style={{ color: "var(--wr)" }}>
            尚有参考图未挂靠提示词组
          </span>
        )}
        {!hasUsableKey && (
          <button
            type="button"
            className="fs12 nowrap"
            style={{ color: "var(--wr)", textDecoration: "underline" }}
            onClick={() => go("settings")}
          >
            无可用 API Key · 去设置
          </button>
        )}
        <button
          type="button"
          className="btn sm gho nowrap"
          title={paramErr ?? paramSum}
          onClick={() => setParamPop((v) => !v)}
        >
          参数 {paramShort} {paramErr && <span style={{ color: "var(--wr)" }}>· 有误</span>} ▾
        </button>
        <button
          type="button"
          className="btn pri"
          disabled={!allMapped || combos === 0 || starting || !hasUsableKey || paramErr != null}
          title={paramErr ?? undefined}
          onClick={openConfirm}
        >
          <Play className="ic12" />
          开始生成
        </button>

        {paramPop && (
          <div className="ppop">
            <div className="fx ac gap8">
              <span className="fw6 fs12">生成参数</span>
              <span className="fs11 t3">未设置 = 跟随提示词与模型默认</span>
              <div className="f1" />
              <button type="button" className="icb" onClick={() => setParamPop(false)}>
                <X className="ic12" />
              </button>
            </div>
            <div className="prow2 mt12" style={{ alignItems: "flex-start" }}>
              <span className="plabel" style={{ marginTop: 3 }}>
                比例
              </span>
              <Seg value={aspectRatio} options={RATIO_OPTS} onChange={pickRatio} wrap />
            </div>
            <div className="fs11 t3 mt6" style={{ lineHeight: 1.7, paddingLeft: 80 }}>
              选比例会自动带上配套的精确尺寸，<b>两个字段一起发</b>——实测只发{" "}
              <span className="mono">aspect_ratio</span> 时整批回的是 1024×1024 正方形。
              <b>提示词里写「9:16」对模型不构成约束</b>，那是描述不是参数。
            </div>
            <div className="prow2 mt8">
              <span className="plabel">精确尺寸</span>
              <input
                className="inp"
                style={{ width: 148 }}
                placeholder="留空，如 1152x2048"
                value={size ?? ""}
                onChange={(e) => setSize(e.target.value.trim() || null)}
              />
              <span className="fs11 t3">
                选比例已自动填；改了就以你填的为准，边长须为 16 的倍数
              </span>
            </div>
            {size !== null && aspectRatio !== null && size !== RATIO_SIZE[aspectRatio] && (
              <div className="fs11 t3 mt6" style={{ paddingLeft: 80 }}>
                你填的尺寸与「{aspectRatio}」的配套值（{RATIO_SIZE[aspectRatio]}）不同，
                两个字段都会照发；上游按哪个出图取决于模型。
              </div>
            )}
            {paramErr && (
              <div className="fs11 mt6" style={{ color: "var(--wr)", paddingLeft: 80 }}>
                {paramErr}
              </div>
            )}
            <div className="prow2 mt8">
              <span className="plabel">输出格式</span>
              <Seg value={outputFormat} options={OUT_FMT_OPTS} onChange={setOutputFormat} />
              <span className="fs11 t3">也决定本地存盘的格式</span>
            </div>
            <div className="prow2 mt8">
              <span className="plabel">抽卡次数</span>
              <Stepper value={draws} min={1} max={5} onChange={setDraws} />
              <span className="fs11 t3">每个组合独立请求 k 次，各占一个任务</span>
            </div>
            <div className="psep" />
            <div className="prow2">
              <span className="plabel">去水印</span>
              <div className="seg">
                <span
                  className={cn("sgi", watermark === "none" && "on")}
                  onClick={() => setWatermark("none")}
                >
                  无需去水印
                </span>
                <span className="sgi dis" title="可见水印去除即将支持">
                  去除可见水印
                </span>
              </div>
            </div>
            <div className="prow2 mt8">
              <span className="plabel">AI 元数据</span>
              <div className="seg">
                <span
                  className={cn("sgi", clearAiMetadata && "on")}
                  onClick={() => setClearAiMetadata(true)}
                >
                  清除
                </span>
                <span
                  className={cn("sgi", !clearAiMetadata && "on")}
                  onClick={() => setClearAiMetadata(false)}
                >
                  保留
                </span>
              </div>
            </div>
            <div className="prow2 mt8">
              <span className="plabel">C2PA</span>
              <div className="seg">
                <span className={cn("sgi", removeC2pa && "on")} onClick={() => setRemoveC2pa(true)}>
                  去除
                </span>
                <span
                  className={cn("sgi", !removeC2pa && "on")}
                  onClick={() => setRemoveC2pa(false)}
                >
                  保留
                </span>
              </div>
            </div>
            <div className="fs11 t3 mt8" style={{ lineHeight: 1.7 }}>
              {deliveryNote(outputFormat, clearAiMetadata, removeC2pa)}
            </div>
            <div className="psep" />
            <div className="fs11 t3" style={{ lineHeight: 1.7 }}>
              <span className="fw6">实际发往接口的字段：</span>
              <span className="mono"> {wireParamsLabel(wire)}</span>
              <br />
              抽卡 ×{draws} = 每个组合请求 {draws} 次（每次 <span className="mono">n=1</span>
              ，各自独立自动恢复与验收）；去水印/元数据处理在本机执行，不进请求。
            </div>
          </div>
        )}
      </div>

      {confirm && (
        <Modal
          title="确认开始生成"
          width="w420"
          onClose={() => setConfirm(null)}
          footer={
            <>
              <div className="f1" />
              <button type="button" className="btn sm" onClick={() => setConfirm(null)}>
                取消
              </button>
              <button
                type="button"
                className="btn pri sm"
                disabled={starting}
                onClick={() => void start()}
              >
                <Play className="ic12" />
                确认生成
              </button>
            </>
          }
        >
          <div className="col gap10">
            <SummaryLine
              label="任务总数"
              value={
                draws > 1
                  ? `${combos} 组合 × ${draws} 抽卡 = ${taskTotal}`
                  : `${combos} 组合 = ${taskTotal}`
              }
            />
            <SummaryLine label="预计请求数" value={`${taskTotal} 次（不含自动恢复）`} />
            <SummaryLine
              label="预计耗时"
              value={etaLabel(confirm.avgSec, taskTotal, enabledKeys)}
            />
            {/* 把「远端到底会收到什么」摆在确认前：设置了却没生效，只能在这里被看见。 */}
            <SummaryLine label="接口参数" value={wireParamsLabel(wire)} />
            <div>
              <div className="fs11 fw6 t3" style={{ letterSpacing: ".05em", marginBottom: 6 }}>
                参与的启用 Key（{enabledKeys.length}）
              </div>
              {enabledKeys.length === 0 ? (
                <div className="fs12" style={{ color: "var(--wr)" }}>
                  当前无启用的 API Key，任务将无法生成——请先到设置启用。
                </div>
              ) : (
                <div className="fx ac gap6 wrap">
                  {enabledKeys.map((k) => (
                    <span key={k.id} className="chip">
                      {k.name || "未命名"} · 并发 {k.concurrencyLimit}
                    </span>
                  ))}
                </div>
              )}
            </div>
          </div>
        </Modal>
      )}

      {modal === "refs" && (
        <PickRefs
          // 临时上传只属于本批，不该出现在「从参考图库选择」里（0019）。
          refs={refs.filter((r) => !r.ephemeral)}
          selected={selRefIds}
          onClose={() => setModal(null)}
          onConfirm={(ids) => {
            setSelRefIds(ids);
            setModal(null);
          }}
        />
      )}
      {viewer && (
        <PromptViewer
          prompts={promptsByGroup[viewer.gid]}
          group={groups.find((g) => g.id === viewer.gid)}
          cc={ccOf(viewer.gid)}
          index={viewer.index}
          onIndex={(i) => setViewer((v) => (v ? { ...v, index: i } : v))}
          onClose={() => setViewer(null)}
        />
      )}
      {uploading && <RefImportOverlay state={uploading} title="正在上传参考图" />}
      {importPreview && (
        <ImportPreviewModal
          preview={importPreview}
          note="将作为本批次临时分组"
          confirmLabel="导入"
          onConfirm={confirmImport}
          onClose={() => setImportPreview(null)}
        />
      )}
    </div>
  );
}

type ParamOpt = { v: string | null; label: string };
/**
 * `aspect_ratio` 受控取值（端点文档：gpt-image-2 系列经此参数控制输出比例，
 * 仅保证比例，像素由上游决定）。须与 Rust `provider::ASPECT_RATIOS` 一致——
 * 那边是真相，命令边界会据此校验，这里只是它的镜子。
 */
const RATIO_OPTS: ParamOpt[] = [
  { v: null, label: "跟随" },
  { v: "1:1", label: "1:1" },
  { v: "9:16", label: "9:16 竖" },
  { v: "16:9", label: "16:9 横" },
  { v: "3:4", label: "3:4 竖" },
  { v: "4:3", label: "4:3 横" },
  { v: "2:3", label: "2:3 竖" },
  { v: "3:2", label: "3:2 横" },
  { v: "21:9", label: "21:9" },
];

/**
 * 每个比例的**配套精确尺寸**：选比例即带上它。
 *
 * 为什么必须带：线上实测（key `aixoras` · 模型 `gpt-image-2-1k`，批次 24–27 共 56 张）
 * 单发 `aspect_ratio: "9:16"` 回来的整批是 **1024×1024 正方形** —— 那个参数在这条链路上
 * 没起作用；补上 `size` 之后才是竖幅（941×1672）。v0.15.2 写的「画幅走 aspect_ratio
 * 而非 size」是照端点文档来的，被实测推翻。两个都发且让二者自洽是代价最低的做法：
 * 多一个字段是零成本，少一个是一整批废图。
 *
 * 取值同时满足两条：**正好是该比例** + **两边都是 16 的倍数**（端点硬性要求）。
 * 所以取不到 1080×1920 那类「手机分辨率」——1080÷16=67.5，端点会整批拒；精确 9:16 的
 * 合法值是 …/1008×1792/1152×2048/1296×2304…，1152×2048 是跨过 1080×1920 的那一档。
 * 上游只保证比例、实际像素自己定，这里给的是**比例的载体**而不是交付分辨率。
 */
const RATIO_SIZE: Record<string, string> = {
  "1:1": "1024x1024",
  "9:16": "1152x2048",
  "16:9": "2048x1152",
  "3:4": "1536x2048",
  "4:3": "2048x1536",
  "2:3": "1024x1536",
  "3:2": "1536x1024",
  "21:9": "2016x864",
};
/** 输出格式：须与 Rust `provider::OUTPUT_FORMATS` 一致（那边是真相）。 */
const OUT_FMT_OPTS: ParamOpt[] = [
  { v: null, label: "跟随" },
  { v: "png", label: "PNG" },
  { v: "jpeg", label: "JPG" },
];

/**
 * 「本地最终存成什么」的说明。输出格式一旦显式选中就说了算——默认那条
 * 「全清 → 统一重编码 JPEG」的规则会把选中的 PNG 悄悄变成 JPG。
 */
function deliveryNote(fmt: string | null, clearMeta: boolean, rmC2pa: boolean): string {
  if (fmt === "png")
    return clearMeta || rmC2pa
      ? "输出存为 PNG：按上面的开关剥离 PNG 文本块与 C2PA，不重编码成 JPEG。"
      : "输出存为 PNG，原样保留其元数据与内容凭据。";
  if (fmt === "jpeg" || (fmt === null && clearMeta && rmC2pa))
    return "输出存为干净 JPG：不含 EXIF/XMP、PNG 文本、IPTC 与 C2PA 内容凭据。";
  return "输出保留远端返回的格式，仅按所选清除对应信息。";
}

/** 该端点对显式 `size` 的硬性要求（与 Rust `provider::validate_size` 同规则）。 */
const SIZE_EDGE_MULTIPLE = 16;

/**
 * 精确尺寸自检：返回错误文案，null = 没问题。
 *
 * 用户实际踩的坑：`1080x1920` 看着是标准 9:16，1080 却不是 16 的倍数，端点回
 * 「edges must be multiples of 16」——而那时钱已经花了。
 */
function sizeIssue(size: string | null): string | null {
  if (!size || size.toLowerCase() === "auto") return null;
  const m = /^(\d+)\s*[x×*]\s*(\d+)$/i.exec(size.trim());
  if (!m) return `尺寸「${size}」格式不对，应形如 1024x1024 或 auto`;
  const w = Number(m[1]);
  const h = Number(m[2]);
  if (w % SIZE_EDGE_MULTIPLE !== 0 || h % SIZE_EDGE_MULTIPLE !== 0) {
    const r = (v: number) => Math.max(1, Math.round(v / SIZE_EDGE_MULTIPLE)) * SIZE_EDGE_MULTIPLE;
    return `尺寸「${size}」边长须为 ${SIZE_EDGE_MULTIPLE} 的倍数（端点限制），可改为 ${r(w)}x${r(h)}，或清空改用上面的「比例」`;
  }
  return null;
}

/**
 * 「实际发往接口的字段」文案。
 *
 * 入参就是构建请求用的那份 wire 记录（键名 = multipart 字段名），故这条文案与
 * `provider/openai.rs` 真正发出去的表单是同一来源——它写什么，远端收什么。
 * 抽卡次数与输出处理全在本机执行，不进请求。
 */
function wireParamsLabel(wire: Record<string, unknown>): string {
  const parts = Object.entries(wire).map(
    ([k, v]) => `${k}=${typeof v === "object" ? JSON.stringify(v) : String(v)}`,
  );
  return parts.length > 0 ? parts.join(" · ") : "无（全部跟随提示词与模型默认）";
}

/** 分段控件：第一项「跟随」为未设置态（虚线强调，D1）。`wrap` 供取值多的行换行。 */
function Seg({
  value,
  options,
  onChange,
  wrap,
}: {
  value: string | null;
  options: ParamOpt[];
  onChange: (v: string | null) => void;
  wrap?: boolean;
}) {
  return (
    <div className={cn("seg", wrap && "segw")}>
      {options.map((o) => {
        const active = value === o.v;
        const isUnset = o.v === null;
        return (
          <span
            key={o.label}
            className={cn("sgi", active && "on", isUnset && "unset")}
            style={isUnset && active ? { borderStyle: "dashed" } : undefined}
            onClick={() => onChange(o.v)}
          >
            {o.label}
          </span>
        );
      })}
    </div>
  );
}

function PickRefs({
  refs,
  selected,
  onClose,
  onConfirm,
}: {
  refs: RefImageView[];
  selected: number[];
  onClose: () => void;
  onConfirm: (ids: number[]) => void;
}) {
  const [sel, setSel] = useState<number[]>(selected);
  // 归档（0016）：跑过批次的图默认折起，已选中的永远可见。
  const [showArchived, setShowArchived] = useState(false);
  const toggle = (id: number) =>
    setSel((c) => (c.includes(id) ? c.filter((x) => x !== id) : [...c, id]));
  // 同 PickGroups：取 initial selected，取消勾选不应让卡片当场消失。
  const [pinned] = useState(() => new Set(selected));
  const hidden = refs.filter((r) => r.archived && !pinned.has(r.id));
  const hiddenIds = new Set(hidden.map((r) => r.id));
  const visible = showArchived ? refs : refs.filter((r) => !hiddenIds.has(r.id));
  return (
    <Modal
      title="从参考图库选择"
      width="w640"
      onClose={onClose}
      headerExtra={
        hidden.length > 0 ? (
          <button
            type="button"
            className={cn("btn sm gho", showArchived && "on")}
            onClick={() => setShowArchived((v) => !v)}
          >
            {showArchived ? "隐藏已归档" : `显示已归档 · ${hidden.length}`}
          </button>
        ) : undefined
      }
      footer={
        <>
          <span className="fs11 t3">选中的参考图会进入已选区</span>
          <div className="f1" />
          <button type="button" className="btn" onClick={onClose}>
            取消
          </button>
          <button type="button" className="btn pri" onClick={() => onConfirm(sel)}>
            添加所选
          </button>
        </>
      }
    >
      <div className="grid" style={{ gridTemplateColumns: "repeat(5,1fr)", gap: 10 }}>
        {visible.map((r) => {
          const src = assetSrc(r.thumbPath);
          return (
            <div
              key={r.id}
              className={cn("rcard", sel.includes(r.id) && "sel", r.archived && "arch")}
              onClick={() => toggle(r.id)}
            >
              <div
                className={cn("rcimg", !src && "ph")}
                style={
                  src
                    ? {
                        backgroundImage: `url(${src})`,
                        backgroundSize: "cover",
                        backgroundPosition: "center",
                      }
                    : undefined
                }
              />
              <span className={cn("rck", sel.includes(r.id) && "on")}>
                <Check className="ic12" />
              </span>
              <div className="fs10 mono t2 mt4 nowrap ohide tc">{r.name}</div>
            </div>
          );
        })}
      </div>
      {visible.length === 0 && (
        <div className="fs12 t3">
          {refs.length === 0
            ? "参考图库为空，请先上传参考图"
            : "全部参考图都已归档 —— 上传新图，或点右上「显示已归档」取回"}
        </div>
      )}
    </Modal>
  );
}

/** 提示词原文弹窗：编号徽标 + 正文 + 组内上一条/下一条导航。 */
function PromptViewer({
  prompts,
  group,
  cc,
  index,
  onIndex,
  onClose,
}: {
  prompts: PromptView[] | undefined;
  group: GroupView | undefined;
  cc: string;
  index: number;
  onIndex: (i: number) => void;
  onClose: () => void;
}) {
  const p = prompts?.[index];
  const total = prompts?.length ?? 0;
  return (
    <Modal
      title={
        <span className="fx ac gap8">
          <span className={cn("pcode", cc)}>{p?.code ?? "—"}</span>
          <span className="fw6 fs13 nowrap ohide">{p?.title ?? group?.name ?? ""}</span>
          {p?.favorite && <i className="favdot" />}
        </span>
      }
      width="w420"
      onClose={onClose}
      headerExtra={<span className="fs11 t3 nowrap">{group?.name}</span>}
      footer={
        <>
          <button
            type="button"
            className="btn sm"
            disabled={index === 0}
            onClick={() => onIndex(index - 1)}
          >
            ‹ 上一条
          </button>
          <span className="fs11 t3 mono">{total > 0 ? `${index + 1} / ${total}` : "—"}</span>
          <button
            type="button"
            className="btn sm"
            disabled={index >= total - 1}
            onClick={() => onIndex(index + 1)}
          >
            下一条 ›
          </button>
          <div className="f1" />
          <button type="button" className="btn pri sm" onClick={onClose}>
            完成
          </button>
        </>
      }
    >
      {p ? <div className="pmtxt">{p.text}</div> : <div className="fs12 t3">加载中…</div>}
    </Modal>
  );
}

function SummaryLine({ label, value }: { label: string; value: string }) {
  return (
    <div className="fx ac gap10">
      <span className="fs12 t2" style={{ width: 76 }}>
        {label}
      </span>
      <span className="fs12 fw5 f1">{value}</span>
    </div>
  );
}

/** ETA 文案（E31）：历史均值 × 任务数 ÷ 有效并发；无历史或无 Key 则不给估算。 */
function etaLabel(avgSec: number | null, taskTotal: number, enabledKeys: ApiKeyView[]): string {
  const concurrency = enabledKeys.reduce((s, k) => s + k.concurrencyLimit, 0);
  if (avgSec == null || concurrency === 0 || taskTotal === 0) return "—（无历史数据）";
  const seconds = Math.round((avgSec * taskTotal) / concurrency);
  if (seconds < 60) return `约 ${seconds} 秒`;
  const m = Math.floor(seconds / 60);
  const s = seconds % 60;
  return s > 0 ? `约 ${m} 分 ${s} 秒` : `约 ${m} 分`;
}
