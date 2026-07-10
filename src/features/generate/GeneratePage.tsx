import { Modal } from "@/components/ui/Modal";
import { assetSrc } from "@/lib/img";
import { type GroupView, type PromptView, type RefImageView, commands, unwrap } from "@/lib/ipc";
import { cn, promptLabel } from "@/lib/utils";
import { useEngineStore } from "@/stores/engine";
import { useUiStore } from "@/stores/ui";
import { ChevronDown, ChevronRight, FileUp, Play, Plus, Upload, X } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { toast } from "sonner";

export function GeneratePage() {
  const go = useUiStore((s) => s.go);
  const loadBatchTasks = useEngineStore((s) => s.loadBatchTasks);

  const [groups, setGroups] = useState<GroupView[]>([]);
  const [refs, setRefs] = useState<RefImageView[]>([]);
  const [selGroupIds, setSelGroupIds] = useState<number[]>([]);
  const [selRefIds, setSelRefIds] = useState<number[]>([]);
  const [mapping, setMapping] = useState<Record<number, number>>({});
  const [modal, setModal] = useState<null | "groups" | "refs" | { assign: number }>(null);
  const [starting, setStarting] = useState(false);
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
    } catch (e) {
      if (e instanceof Error) toast.error(e.message);
    }
  }, []);
  useEffect(() => {
    void load();
  }, [load]);

  const selGroups = groups.filter((g) => selGroupIds.includes(g.id));
  const selRefs = refs.filter((r) => selRefIds.includes(r.id));
  const allMapped = selRefs.length > 0 && selRefs.every((r) => mapping[r.id] != null);
  const total = selRefs.reduce((sum, r) => {
    const g = groups.find((x) => x.id === mapping[r.id]);
    return sum + (g?.count ?? 0);
  }, 0);

  const importTxt = async () => {
    try {
      const path = await unwrap(commands.pickTxtFile());
      if (!path) return;
      const preview = await unwrap(commands.parsePromptTxt(path));
      const res = await unwrap(commands.commitPromptImport(preview, "generate"));
      toast(`已导入 ${res.inserted} 条到临时分组`);
      await load();
      setSelGroupIds((cur) => [...new Set([...cur, ...res.groupIds])]);
    } catch (e) {
      if (e instanceof Error) toast.error(e.message);
    }
  };

  const uploadRefs = async () => {
    try {
      const paths = await unwrap(commands.pickImageFiles());
      if (paths.length === 0) return;
      const added = await unwrap(commands.importRefImages(paths, null));
      toast(`已上传 ${added.length} 张参考图`);
      await load();
      setSelRefIds((cur) => [...new Set([...cur, ...added.map((r) => r.id)])]);
    } catch (e) {
      if (e instanceof Error) toast.error(e.message);
    }
  };

  const start = async () => {
    setStarting(true);
    try {
      const batch = await unwrap(
        commands.createBatch({
          refs: selRefs.map((r) => ({ refImageId: r.id, promptGroupId: mapping[r.id] as number })),
          paramsJson: "{}",
        }),
      );
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
          {total > 0 && (
            <>
              <span className="t3 fs12">=</span>
              <span className="fw6 fs13" style={{ color: "var(--acc2)" }}>
                {total} 个任务
              </span>
            </>
          )}
        </div>
        {!allMapped && selRefs.length > 0 && (
          <span className="fs12 nowrap" style={{ color: "var(--wr)" }}>
            每张参考图需指定一个提示词组
          </span>
        )}
        <button type="button" className="btn pri" disabled={!allMapped || starting} onClick={start}>
          <Play className="ic12" />
          开始生成
        </button>
      </div>

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

function bg(thumbPath?: string | null): React.CSSProperties {
  const src = assetSrc(thumbPath);
  return src
    ? { backgroundImage: `url(${src})`, backgroundSize: "cover", backgroundPosition: "center" }
    : {};
}
