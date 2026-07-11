import { Modal } from "@/components/ui/Modal";
import { Stepper } from "@/components/ui/Stepper";
import { ImportPreviewModal } from "@/features/_shared/ImportPreviewModal";
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
import { cn, promptLabel } from "@/lib/utils";
import { useEngineStore } from "@/stores/engine";
import { useGenerateStore } from "@/stores/generate";
import { useUiStore } from "@/stores/ui";
import { ChevronDown, ChevronRight, FileUp, Play, Plus, Upload, X } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { toast } from "sonner";

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
  const size = useGenerateStore((s) => s.size);
  const setSize = useGenerateStore((s) => s.setSize);
  const quality = useGenerateStore((s) => s.quality);
  const setQuality = useGenerateStore((s) => s.setQuality);
  // E17 / D2：抽卡次数 k（每组合独立生成 k 次），默认 1，上限 5。
  const draws = useGenerateStore((s) => s.draws);
  const setDraws = useGenerateStore((s) => s.setDraws);
  // E31：启用 Key 快照（确认摘要展示 + ETA 估算并发）。
  const [keys, setKeys] = useState<ApiKeyView[]>([]);
  // E31：开始生成确认卡（null = 未打开）。
  const [confirm, setConfirm] = useState<null | { avgSec: number | null }>(null);
  const [modal, setModal] = useState<null | "groups" | "refs" | { assign: number }>(null);
  const [starting, setStarting] = useState(false);
  // E14：生成页 txt 导入改走预览确认（取消不落库、不产生临时分组）。
  const [importPreview, setImportPreview] = useState<ImportPreview | null>(null);
  // E25：今日生产总览。
  const [overview, setOverview] = useState<ProductionOverview | null>(null);
  // 已展开查看提示词原文的分组 + 其提示词缓存（按需加载）。
  const [expanded, setExpanded] = useState<Set<number>>(new Set());
  const [promptsByGroup, setPromptsByGroup] = useState<Record<number, PromptView[]>>({});

  const toggleExpand = useCallback(
    async (gid: number) => {
      setExpanded((cur) => {
        const next = new Set(cur);
        if (next.has(gid)) next.delete(gid);
        else next.add(gid);
        return next;
      });
      if (!promptsByGroup[gid]) {
        try {
          const ps = await unwrap(commands.listPrompts(gid));
          setPromptsByGroup((m) => ({ ...m, [gid]: ps }));
        } catch (e) {
          if (e instanceof Error) toast.error(e.message);
        }
      }
    },
    [promptsByGroup],
  );

  const load = useCallback(async () => {
    try {
      setGroups(await unwrap(commands.listPromptGroups()));
      setRefs(await unwrap(commands.listRefImages()));
      setKeys(await unwrap(commands.listApiKeys()));
      setOverview(await unwrap(commands.productionOverview()).catch(() => null));
    } catch (e) {
      if (e instanceof Error) toast.error(e.message);
    }
  }, []);
  useEffect(() => {
    void load();
  }, [load]);

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

  const confirmImport = async () => {
    if (!importPreview) return;
    const res = await unwrap(commands.commitPromptImport(importPreview, "generate")).catch((e) => {
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

  const importImages = useCallback(
    async (paths: string[]) => {
      if (paths.length === 0) return;
      const added = await unwrap(commands.importRefImages(paths, null)).catch((e) => {
        toast.error(String(e));
        return [];
      });
      if (added.length > 0) {
        toast(`已上传 ${added.length} 张参考图`);
        await load();
        setSelRefIds((cur) => [...new Set([...cur, ...added.map((r) => r.id)])]);
      }
    },
    [load, setSelRefIds],
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
  const buildParamsJson = () => {
    const p: Record<string, string> = {};
    if (size) p.size = size;
    if (quality) p.quality = quality;
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
      toast(`已创建批次 #${batch.id} · ${batch.taskCount} 个任务`);
      await loadBatchTasks(batch.id, null);
      go("tasks");
    } catch (e) {
      if (e instanceof Error) toast.error(e.message);
    } finally {
      setStarting(false);
    }
  };

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
        <span className="pcap">参考图 × 提示词组 → 任务队列 → 验收 → 输出</span>
      </div>
      <div className="pbody">
        <div className="cwrap">
          {/* 提示词卡 */}
          <div className="card">
            <div className="chead">
              <span className="fw6 fs13">提示词</span>
              {selGroups.length > 0 && <span className="cnt">{selGroups.length} 组</span>}
              <div className="f1" />
              <button type="button" className="btn sm gho" onClick={importTxt}>
                <FileUp className="ic12" />
                导入 .txt
              </button>
              <button type="button" className="btn sm" onClick={() => setModal("groups")}>
                <Plus className="ic12" />
                选择提示词组
              </button>
            </div>
            {selGroups.length === 0 ? (
              <div className="empt">
                <div className="fs13 fw5 t2">尚未选择提示词</div>
                <div className="fs12 t3 mt4">
                  从提示词库选择分组，或导入 .txt 作为本批次的临时提示词
                </div>
              </div>
            ) : (
              selGroups.map((g) => (
                <div className="ggrp" key={g.id}>
                  <div className="fx ac gap9">
                    <button
                      type="button"
                      className="icb"
                      title={expanded.has(g.id) ? "收起提示词" : "展开查看提示词"}
                      onClick={() => toggleExpand(g.id)}
                    >
                      {expanded.has(g.id) ? (
                        <ChevronDown className="ic12" />
                      ) : (
                        <ChevronRight className="ic12" />
                      )}
                    </button>
                    <i className="gdot" style={{ background: "var(--acc)" }} />
                    <span className="fw5 nowrap">{g.name}</span>
                    <span className="chip">{g.prefix}</span>
                    {g.scene && <span className="bdg b-gray">{g.scene}</span>}
                    {g.isTemp && <span className="bdg b-amber">临时 · 验收通过后自动入库</span>}
                    <div className="f1" />
                    <span className="t3 fs12 nowrap">{g.count} 条</span>
                    <button
                      type="button"
                      className="icb"
                      onClick={() => setSelGroupIds((c) => c.filter((x) => x !== g.id))}
                    >
                      <X className="ic12" />
                    </button>
                  </div>
                  {expanded.has(g.id) && <GroupPromptList prompts={promptsByGroup[g.id]} />}
                </div>
              ))
            )}
          </div>

          {/* 参考图卡 */}
          <div className="card mt14">
            <div className="chead">
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
            {selRefs.length === 0 ? (
              <div className="empt">
                <div className="fs13 fw5 t2">尚未选择参考图</div>
                <div className="fs12 t3 mt4">
                  从参考图库选择已有素材，或上传新图；每张参考图将挂靠一个提示词组
                </div>
              </div>
            ) : (
              <div className="refgrid">
                {selRefs.map((r) => {
                  const mg = groups.find((g) => g.id === mapping[r.id]);
                  return (
                    <div className="reftile" key={r.id}>
                      <div className="ph rtph" style={bg(r.thumbPath)}>
                        <button
                          type="button"
                          className="rtx"
                          onClick={() => setSelRefIds((c) => c.filter((x) => x !== r.id))}
                        >
                          <X className="ic12" />
                        </button>
                      </div>
                      <div className="fs11 fw5 mt6 nowrap ohide tc mono">{r.name}</div>
                      <button
                        type="button"
                        className={cn("mapchip", mg ? "mapped" : "needmap")}
                        onClick={() => setModal({ assign: r.id })}
                      >
                        <span className="nowrap ohide">{mg ? mg.name : "指定提示词组"}</span>
                        <ChevronDown className="ic12" />
                      </button>
                    </div>
                  );
                })}
              </div>
            )}
          </div>

          {/* 生成参数卡（E16 / D1：默认跟随提示词，显式设置才透传） */}
          <div className="card mt14">
            <div className="chead">
              <span className="fw6 fs13">生成参数</span>
              <span className="fs11 t3">未设置 = 不传该参数，以提示词与模型默认为准</span>
            </div>
            <ParamRow label="尺寸 / 比例" value={size} onChange={setSize} options={SIZE_OPTS} />
            <div className="mt10">
              <ParamRow label="质量" value={quality} onChange={setQuality} options={QUALITY_OPTS} />
            </div>
            <div className="fx ac gap10 mt10">
              <span className="fs12 t2" style={{ width: 76 }}>
                抽卡次数
              </span>
              <Stepper value={draws} min={1} max={5} onChange={setDraws} />
              <span className="fs11 t3">
                每个「参考图 × 提示词」组合独立生成 k 次，各占一个任务
              </span>
            </div>
          </div>
        </div>
      </div>

      {/* genbar */}
      <div className="genbar">
        <div className="fx ac gap6 wrap f1">
          {selRefs.map((r) => {
            const g = groups.find((x) => x.id === mapping[r.id]);
            return (
              <span key={r.id} className={cn("fchip", !g && "warn")}>
                {r.name} × {g ? `${g.prefix}(${g.count})` : "?"}
              </span>
            );
          })}
          {combos > 0 && (
            <>
              <span className="t3 fs12">=</span>
              <span className="fw6 fs13" style={{ color: "var(--acc2)" }}>
                {combos} 组合{draws > 1 ? ` × ${draws}` : ""} = {taskTotal} 个任务
              </span>
            </>
          )}
        </div>
        {!allMapped && selRefs.length > 0 && (
          <span className="fs12 nowrap" style={{ color: "var(--wr)" }}>
            每张参考图需指定一个提示词组
          </span>
        )}
        {/* E12：无启用 Key（或并发合计为 0）时禁用并引导去设置 */}
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
          className="btn pri"
          disabled={!allMapped || starting || !hasUsableKey}
          onClick={openConfirm}
        >
          <Play className="ic12" />
          开始生成
        </button>
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
            <SummaryLine label="预计请求数" value={`${taskTotal} 次（不含失败重试）`} />
            <SummaryLine
              label="预计耗时"
              value={etaLabel(confirm.avgSec, taskTotal, enabledKeys)}
            />
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

      {modal === "groups" && (
        <PickGroups
          groups={groups}
          selected={selGroupIds}
          onClose={() => setModal(null)}
          onConfirm={(ids) => {
            setSelGroupIds(ids);
            setModal(null);
          }}
        />
      )}
      {modal === "refs" && (
        <PickRefs
          refs={refs}
          selected={selRefIds}
          onClose={() => setModal(null)}
          onConfirm={(ids) => {
            setSelRefIds(ids);
            setModal(null);
          }}
        />
      )}
      {modal && typeof modal === "object" && "assign" in modal && (
        <AssignGroup
          refName={refs.find((r) => r.id === modal.assign)?.name ?? ""}
          groups={selGroups}
          onClose={() => setModal(null)}
          onPick={(gid) => {
            setMapping((m) => ({ ...m, [(modal as { assign: number }).assign]: gid }));
            setModal(null);
          }}
        />
      )}
      {importPreview && (
        <ImportPreviewModal
          preview={importPreview}
          note="将作为本批次临时分组"
          confirmLabel={`导入 ${importPreview.total} 条`}
          onConfirm={confirmImport}
          onClose={() => setImportPreview(null)}
        />
      )}
    </div>
  );
}

function PickGroups({
  groups,
  selected,
  onClose,
  onConfirm,
}: {
  groups: GroupView[];
  selected: number[];
  onClose: () => void;
  onConfirm: (ids: number[]) => void;
}) {
  const [sel, setSel] = useState<number[]>(selected);
  const toggle = (id: number) =>
    setSel((c) => (c.includes(id) ? c.filter((x) => x !== id) : [...c, id]));
  return (
    <Modal
      title="选择提示词组"
      width="w640"
      onClose={onClose}
      footer={
        <>
          <span className="fs11 t3">选中分组的全部提示词参与本批生成</span>
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
      {groups.map((g) => (
        <div
          key={g.id}
          className={cn("gpick", sel.includes(g.id) && "sel")}
          onClick={() => toggle(g.id)}
        >
          <div className="fx ac gap9">
            <span className={cn("ckb", sel.includes(g.id) && "on")} />
            <i className="gdot" style={{ background: "var(--acc)" }} />
            <span className="fw5 nowrap">{g.name}</span>
            <span className="chip">{g.prefix}</span>
            {g.scene && <span className="bdg b-gray">{g.scene}</span>}
            <div className="f1" />
            <span className="t3 fs12 nowrap">{g.count} 条</span>
          </div>
        </div>
      ))}
      {groups.length === 0 && <div className="fs12 t3">暂无提示词分组，请先在提示词库导入</div>}
    </Modal>
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
  const toggle = (id: number) =>
    setSel((c) => (c.includes(id) ? c.filter((x) => x !== id) : [...c, id]));
  return (
    <Modal
      title="从参考图库选择"
      width="w640"
      onClose={onClose}
      footer={
        <>
          <span className="fs11 t3">选中的参考图会进入生成页的已选区</span>
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
        {refs.map((r) => (
          <div
            key={r.id}
            className={cn("rcard", sel.includes(r.id) && "sel")}
            onClick={() => toggle(r.id)}
          >
            <div className="ph rcimg" style={bg(r.thumbPath)} />
            <span className={cn("rck", sel.includes(r.id) && "on")} />
            <div className="fs10 mono t2 mt4 nowrap ohide tc">{r.name}</div>
          </div>
        ))}
      </div>
      {refs.length === 0 && <div className="fs12 t3">参考图库为空，请先上传参考图</div>}
    </Modal>
  );
}

function AssignGroup({
  refName,
  groups,
  onClose,
  onPick,
}: {
  refName: string;
  groups: GroupView[];
  onClose: () => void;
  onPick: (gid: number) => void;
}) {
  return (
    <Modal title={`为 ${refName} 指定提示词组`} width="w640" onClose={onClose}>
      {groups.map((g) => (
        <div key={g.id} className="gpick" onClick={() => onPick(g.id)}>
          <div className="fx ac gap9">
            <i className="gdot" style={{ background: "var(--acc)" }} />
            <span className="fw5 nowrap">{g.name}</span>
            <span className="chip">{g.prefix}</span>
            <div className="f1" />
            <span className="t3 fs12 nowrap">{g.count} 条</span>
          </div>
        </div>
      ))}
      {groups.length === 0 && <div className="fs12 t3">请先在上方「选择提示词组」中选择分组</div>}
    </Modal>
  );
}

/** 展开区：分组内提示词列表，点击某条切换显示其原文。 */
function GroupPromptList({ prompts }: { prompts: PromptView[] | undefined }) {
  const [openId, setOpenId] = useState<number | null>(null);
  if (!prompts) {
    return (
      <div className="fs12 t3 mt6" style={{ paddingLeft: 26 }}>
        加载中…
      </div>
    );
  }
  if (prompts.length === 0) {
    return (
      <div className="fs12 t3 mt6" style={{ paddingLeft: 26 }}>
        该分组暂无提示词
      </div>
    );
  }
  return (
    <div className="col gap6 mt6" style={{ paddingLeft: 26 }}>
      {prompts.map((p) => {
        const open = openId === p.id;
        return (
          <div key={p.id} className="col gap4">
            <button
              type="button"
              className="pchip"
              style={{ alignSelf: "flex-start" }}
              title={open ? "收起原文" : "查看原文"}
              onClick={() => setOpenId(open ? null : p.id)}
            >
              {p.favorite && <i className="favdot" />}
              {promptLabel(p.code, p.title)}
            </button>
            {open && <div className="ptext">{p.text}</div>}
          </div>
        );
      })}
    </div>
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

type ParamOpt = { v: string | null; label: string };
const SIZE_OPTS: ParamOpt[] = [
  { v: null, label: "跟随提示词" },
  { v: "1024x1024", label: "1:1" },
  { v: "1536x1024", label: "3:2 横" },
  { v: "1024x1536", label: "2:3 竖" },
  { v: "auto", label: "自动" },
];
const QUALITY_OPTS: ParamOpt[] = [
  { v: null, label: "跟随提示词" },
  { v: "low", label: "低" },
  { v: "medium", label: "中" },
  { v: "high", label: "高" },
  { v: "auto", label: "自动" },
];

/** 生成参数单行：分段控件，第一项「跟随提示词」为未设置态（虚线占位强调，D1）。 */
function ParamRow({
  label,
  value,
  onChange,
  options,
}: {
  label: string;
  value: string | null;
  onChange: (v: string | null) => void;
  options: ParamOpt[];
}) {
  return (
    <div className="fx ac gap10">
      <span className="fs12 t2" style={{ width: 76 }}>
        {label}
      </span>
      <div className="seg">
        {options.map((o) => {
          const active = value === o.v;
          const isUnset = o.v === null;
          return (
            <span
              key={o.label}
              className={cn("sgi", active && "on")}
              // 未设置态用虚线边框区分「跟随提示词」与已设置项（D1）。
              style={isUnset && active ? { borderStyle: "dashed" } : undefined}
              onClick={() => onChange(o.v)}
            >
              {o.label}
            </span>
          );
        })}
      </div>
    </div>
  );
}

function bg(thumbPath?: string | null): React.CSSProperties {
  const src = assetSrc(thumbPath);
  return src
    ? { backgroundImage: `url(${src})`, backgroundSize: "cover", backgroundPosition: "center" }
    : {};
}
