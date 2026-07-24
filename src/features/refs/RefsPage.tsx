import { ConfirmModal, Modal } from "@/components/ui/Modal";
import { NatThumb } from "@/features/_shared/NatThumb";
import { PageScaffold } from "@/features/_shared/PageScaffold";
import { RefImportOverlay, useRefImport } from "@/features/_shared/RefImport";
import { assetSrc } from "@/lib/img";
import {
  type RefGroupView,
  type RefImageDetail,
  type RefImageView,
  type RefScanItem,
  commands,
  subscribeFileDrop,
  unwrap,
} from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { CheckSquare, FolderInput, FolderPlus, Pencil, Trash2, Upload } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { toast } from "sonner";

export function RefsPage() {
  const [refs, setRefs] = useState<RefImageView[]>([]);
  // 0019：图库自己的分组（ref_groups），不再借用提示词组。
  const [groups, setGroups] = useState<RefGroupView[]>([]);
  const [detail, setDetail] = useState<RefImageDetail | null>(null);
  const [confirmDel, setConfirmDel] = useState<RefImageDetail | null>(null);
  // E30a：导入时先选分组。pendingPaths 非空即展示选组弹窗。
  const [pendingPaths, setPendingPaths] = useState<string[] | null>(null);
  const [newGroupName, setNewGroupName] = useState("");
  // E30b：导入去重扫描结果（含重复项时弹窗），与多选批量操作态。
  const [scan, setScan] = useState<RefScanItem[] | null>(null);
  const [selectMode, setSelectMode] = useState(false);
  const [sel, setSel] = useState<Set<number>>(new Set());
  const [batchPick, setBatchPick] = useState(false);
  const [confirmBatchDel, setConfirmBatchDel] = useState(false);
  // 分组管理弹窗（新建/改名/删除）。
  const [manageGroups, setManageGroups] = useState(false);
  const [confirmDelGroup, setConfirmDelGroup] = useState<RefGroupView | null>(null);
  const lastClicked = useRef<number | null>(null);
  const { state: importing, busy, run: runImport } = useRefImport();

  const load = useCallback(async () => {
    try {
      // 临时上传（生成页随手传的图）不属于长期图库，这里一律不列。
      const all = await unwrap(commands.listRefImages());
      setRefs(all.filter((r) => !r.ephemeral));
      setGroups(await unwrap(commands.listRefGroups()));
    } catch (e) {
      if (e instanceof Error) toast.error(e.message);
    }
  }, []);
  useEffect(() => {
    void load();
  }, [load]);

  // E30b：导入前先按内容 hash 扫描重复；有重复弹窗（默认跳过），否则直接进选组。
  const beginImport = useCallback(
    async (paths: string[]) => {
      if (paths.length === 0 || busy.current) return;
      const items = await unwrap(commands.scanRefImports(paths)).catch((e) => {
        toast.error(String(e));
        return null;
      });
      if (!items) return;
      if (items.some((i) => i.duplicate)) {
        setScan(items);
      } else {
        setNewGroupName("");
        setPendingPaths(paths);
      }
    },
    [busy],
  );

  // 第一步：选文件 → 去重扫描（E30a + E30b）。多选一次可挑整批。
  const importRefs = async () => {
    const paths = await unwrap(commands.pickImageFiles()).catch(() => [] as string[]);
    await beginImport(paths);
  };

  // E14：拖拽图片进参考图库 → 同一去重 + 选组流程。
  useEffect(() => {
    let un = () => {};
    void subscribeFileDrop((paths) => {
      const images = paths.filter((p) => /\.(png|jpe?g|webp|bmp)$/i.test(p));
      if (images.length > 0) void beginImport(images);
      else if (paths.length > 0) toast.error("参考图库仅支持拖入图片文件");
    }).then((f) => {
      un = f;
    });
    return () => un();
  }, [beginImport]);

  // 去重弹窗：跳过重复继续（默认），或全部仍导入。
  const proceedSkipDup = () => {
    const keep = (scan ?? []).filter((i) => !i.duplicate).map((i) => i.path);
    setScan(null);
    if (keep.length === 0) {
      toast("全部为重复项，已跳过");
      return;
    }
    setNewGroupName("");
    setPendingPaths(keep);
  };
  const proceedImportAll = () => {
    const all = (scan ?? []).map((i) => i.path);
    setScan(null);
    setNewGroupName("");
    setPendingPaths(all);
  };

  // 第二步：带选定分组导入。gid=null 为未分组。ephemeral=false —— 图库的图是长期资产。
  const doImport = async (gid: number | null) => {
    const paths = pendingPaths;
    if (!paths) return;
    setPendingPaths(null);
    try {
      const added = await runImport(paths, gid, false);
      const skipped = paths.length - added.length;
      toast(
        skipped > 0
          ? `已导入 ${added.length} 张，${skipped} 张失败已跳过`
          : `已导入 ${added.length} 张参考图`,
      );
      void load();
    } catch (e) {
      if (e instanceof Error) toast.error(e.message);
    }
  };

  // 新建分组后立即用它导入。
  const importIntoNewGroup = async () => {
    const name = newGroupName.trim();
    if (!name || busy.current) return;
    try {
      const g = await unwrap(commands.createRefGroup(name));
      setGroups((gs) => (gs.some((x) => x.id === g.id) ? gs : [...gs, g]));
      await doImport(g.id);
    } catch (e) {
      if (e instanceof Error) toast.error(e.message);
    }
  };

  const openDetail = async (id: number) => {
    setDetail(await unwrap(commands.getRefImage(id)).catch(() => null));
  };

  const setGroup = async (id: number, gid: number | null) => {
    await unwrap(commands.setRefImageGroup(id, gid)).catch(() => {});
    setDetail((d) => (d ? { ...d, groupId: gid } : d));
    void load();
  };

  const replace = async (id: number) => {
    const path = await unwrap(commands.pickImageFiles()).catch(() => [] as string[]);
    if (path.length === 0) return;
    await unwrap(commands.replaceRefImageFile(id, path[0] as string)).catch((e) =>
      toast.error(String(e)),
    );
    toast("已更换图片");
    void load();
    void openDetail(id);
  };

  // 0016：归档只管「生成页选择器是否列出它」，图与分组归属分毫不动。
  const toggleArchive = async (id: number, archived: boolean) => {
    try {
      await unwrap(commands.setRefImageArchived(id, !archived));
      setRefs((cur) => cur.map((r) => (r.id === id ? { ...r, archived: !archived } : r)));
      toast(archived ? "已取消归档" : "已归档");
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
      void load();
    }
  };

  const del = async (d: RefImageDetail) => {
    await unwrap(commands.trashRefImage(d.id)).catch(() => {});
    setDetail(null);
    void load();
    toast("已移入废纸篓");
  };

  // ---- 分组管理 ----
  const createGroup = async (name: string) => {
    const n = name.trim();
    if (!n) return;
    try {
      await unwrap(commands.createRefGroup(n));
      void load();
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    }
  };
  const renameGroup = async (id: number, name: string) => {
    try {
      await unwrap(commands.renameRefGroup(id, name.trim()));
      void load();
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    }
  };
  const deleteGroup = async (g: RefGroupView) => {
    try {
      await unwrap(commands.deleteRefGroup(g.id));
      setConfirmDelGroup(null);
      toast(g.count > 0 ? `已删除分组，${g.count} 张图回到未分组` : "已删除分组");
      void load();
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    }
  };

  const byGroup = (gid: number | null) => refs.filter((r) => (r.groupId ?? null) === gid);
  const sections: { name: string; gid: number | null; items: RefImageView[] }[] = [
    ...groups.map((g) => ({ name: g.name, gid: g.id as number | null, items: byGroup(g.id) })),
    { name: "未分组", gid: null, items: byGroup(null) },
  ].filter((s) => s.items.length > 0);

  // E30b：多选批量操作。扁平序（分组内顺序）供 shift 范围选择。
  const flat = sections.flatMap((s) => s.items);
  const exitSelect = () => {
    setSelectMode(false);
    setSel(new Set());
    lastClicked.current = null;
  };
  const onCardClick = (globalIdx: number, id: number, shift: boolean) => {
    if (!selectMode) {
      void openDetail(id);
      return;
    }
    if (shift && lastClicked.current !== null) {
      const a = Math.min(lastClicked.current, globalIdx);
      const b = Math.max(lastClicked.current, globalIdx);
      setSel((s) => {
        const n = new Set(s);
        for (let i = a; i <= b; i++) {
          const it = flat[i];
          if (it) n.add(it.id);
        }
        return n;
      });
    } else {
      setSel((s) => {
        const n = new Set(s);
        if (n.has(id)) n.delete(id);
        else n.add(id);
        return n;
      });
      lastClicked.current = globalIdx;
    }
  };
  const batchSetGroup = async (gid: number | null) => {
    const ids = [...sel];
    await unwrap(commands.setRefImagesGroup(ids, gid)).catch((e) => toast.error(String(e)));
    toast.success(`已移动 ${ids.length} 张`);
    setBatchPick(false);
    exitSelect();
    void load();
  };
  const batchDelete = async () => {
    const ids = [...sel];
    await unwrap(commands.trashRefImages(ids)).catch((e) => toast.error(String(e)));
    toast(`已移入废纸篓 ${ids.length} 张`);
    setConfirmBatchDel(false);
    exitSelect();
    void load();
  };

  return (
    <PageScaffold title="参考图库" caption="长期素材库 · 自定义分组">
      <div className="phd" style={{ borderBottom: "none", minHeight: 0, paddingTop: 8 }}>
        <span className="cnt">{refs.length} 张</span>
        <span className="fs11 t3 nowrap">{groups.length} 个分组</span>
        <div className="f1" />
        {selectMode ? (
          <>
            <span className="fs12 t2 nowrap">已选 {sel.size}</span>
            <button
              type="button"
              className="btn sm"
              disabled={sel.size === 0}
              onClick={() => setBatchPick(true)}
            >
              <FolderInput className="ic12" />
              改分组
            </button>
            <button
              type="button"
              className="btn sm gho dng"
              disabled={sel.size === 0}
              onClick={() => setConfirmBatchDel(true)}
            >
              删除
            </button>
            <button type="button" className="btn sm gho" onClick={exitSelect}>
              退出多选
            </button>
          </>
        ) : (
          <>
            {refs.length > 0 && (
              <button
                type="button"
                className="btn sm gho"
                onClick={() => setSelectMode(true)}
                title="多选参考图做批量操作"
              >
                <CheckSquare className="ic12" />
                多选
              </button>
            )}
            <button
              type="button"
              className="btn sm gho"
              onClick={() => setManageGroups(true)}
              title="新建 / 重命名 / 删除图库分组"
            >
              <FolderPlus className="ic12" />
              管理分组
            </button>
            <button
              type="button"
              className="btn sm"
              disabled={importing !== null}
              onClick={importRefs}
            >
              <Upload className="ic12" />
              批量上传
            </button>
          </>
        )}
      </div>

      {refs.length === 0 ? (
        <div className="bigempty">
          <div className="fs13 fw5 t2">参考图库为空</div>
          <div className="fs12 t3">
            批量上传长期复用的素材（可一次多选，也可整批拖入窗口），按自定义分组归置
          </div>
          <button
            type="button"
            className="btn mt10"
            disabled={importing !== null}
            onClick={importRefs}
          >
            批量上传
          </button>
        </div>
      ) : (
        <div className="pbody">
          {sections.map((s) => (
            <div className="gsec" key={s.name}>
              <div className="ghead">
                <span className="fw6 fs13 nowrap">{s.name}</span>
                <span className="fs11 t3">{s.items.length}</span>
              </div>
              <div className="fgrid">
                {s.items.map((r) => {
                  const idx = flat.findIndex((f) => f.id === r.id);
                  return (
                    <div
                      key={r.id}
                      className={cn(
                        "rcard",
                        selectMode && sel.has(r.id) && "sel",
                        r.archived && "arch",
                      )}
                      onClick={(e) => onCardClick(idx, r.id, e.shiftKey)}
                    >
                      <NatThumb path={r.thumbPath} className="rcimg rcnat" />
                      <div className="rmeta">
                        <span className="mono fs11 fw5 nowrap ohide f1">{r.name}</span>
                        {r.archived && (
                          <span className="bdg b-gray" title="已跑过批次，生成页选择器默认不再列出">
                            已归档
                          </span>
                        )}
                      </div>
                    </div>
                  );
                })}
              </div>
            </div>
          ))}
        </div>
      )}

      {importing && <RefImportOverlay state={importing} title="正在上传参考图" />}

      {detail && (
        <Modal
          title={<span className="mono">{detail.name}</span>}
          width="w640"
          onClose={() => setDetail(null)}
        >
          <div className="fx gap14">
            <div style={{ width: 280, flex: "none" }}>
              <div
                className="ph"
                style={{
                  ...bg(detail.thumbPath),
                  aspectRatio: "1",
                  borderRadius: 10,
                  border: "1px solid var(--line)",
                }}
              />
              <div className="fx ac gap6 mt10 wrap">
                <span className="chip">使用 {detail.usedCount} 次</span>
                <span className="chip">产出 {detail.worksCount} 张</span>
              </div>
              <div className="fx gap8 mt10">
                <button
                  type="button"
                  className="btn sm f1"
                  style={{ justifyContent: "center" }}
                  onClick={() => replace(detail.id)}
                >
                  更换图片
                </button>
                {(() => {
                  const archived = refs.find((r) => r.id === detail.id)?.archived ?? false;
                  return (
                    <button
                      type="button"
                      className="btn sm gho"
                      title="归档后生成页的选择器默认不再列出这张图；图本身留在库里"
                      onClick={() => void toggleArchive(detail.id, archived)}
                    >
                      {archived ? "取消归档" : "归档"}
                    </button>
                  );
                })()}
                <button
                  type="button"
                  className="btn sm gho dng"
                  onClick={() => setConfirmDel(detail)}
                >
                  删除
                </button>
              </div>
            </div>
            <div className="f1" style={{ minWidth: 0 }}>
              <div className="fs11 fw6 t3" style={{ letterSpacing: ".05em" }}>
                所属分组
              </div>
              <div className="mt6">
                <div className="pickrow" onClick={() => setGroup(detail.id, null)}>
                  <span className={cn("ckb", detail.groupId == null && "on")} />
                  <span className="fw5 f1 nowrap ohide">未分组</span>
                </div>
                {groups.map((g) => (
                  <div key={g.id} className="pickrow" onClick={() => setGroup(detail.id, g.id)}>
                    <span className={cn("ckb", detail.groupId === g.id && "on")} />
                    <i className="gdot" style={{ background: "var(--acc)" }} />
                    <span className="fw5 f1 nowrap ohide">{g.name}</span>
                    <span className="fs11 t3">{g.count}</span>
                  </div>
                ))}
              </div>
              <div className="fs11 t3 mt10" style={{ lineHeight: 1.7 }}>
                分组是图库自己的目录，与提示词组无关；调整分组不影响历史任务与作品的关联。
              </div>
            </div>
          </div>
        </Modal>
      )}

      {manageGroups && (
        <ManageGroupsModal
          groups={groups}
          unassigned={byGroup(null).length}
          onCreate={createGroup}
          onRename={renameGroup}
          onDelete={(g) => setConfirmDelGroup(g)}
          onClose={() => setManageGroups(false)}
        />
      )}

      {confirmDelGroup && (
        <ConfirmModal
          title={`删除分组「${confirmDelGroup.name}」`}
          desc={
            confirmDelGroup.count > 0
              ? `组内 ${confirmDelGroup.count} 张图不会被删除，会回到「未分组」。`
              : "该分组为空，删除不影响任何图片。"
          }
          confirmLabel="删除分组"
          danger
          onConfirm={() => void deleteGroup(confirmDelGroup)}
          onClose={() => setConfirmDelGroup(null)}
        />
      )}

      {pendingPaths && (
        <Modal
          title={`上传 ${pendingPaths.length} 张参考图 · 选择分组`}
          width="w420"
          onClose={() => setPendingPaths(null)}
        >
          <div className="mlist" style={{ maxHeight: 300, overflow: "auto" }}>
            <div className="pickrow" onClick={() => void doImport(null)}>
              <span className="ckb" />
              <span className="fw5 f1 nowrap ohide">未分组</span>
            </div>
            {groups.map((g) => (
              <div key={g.id} className="pickrow" onClick={() => void doImport(g.id)}>
                <span className="ckb" />
                <i className="gdot" style={{ background: "var(--acc)" }} />
                <span className="fw5 f1 nowrap ohide">{g.name}</span>
                <span className="fs11 t3">{g.count}</span>
              </div>
            ))}
          </div>
          <div className="fx ac gap8 mt10">
            <input
              className="inp f1"
              placeholder="新建分组名…"
              value={newGroupName}
              onChange={(e) => setNewGroupName(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") void importIntoNewGroup();
              }}
            />
            <button
              type="button"
              className="btn sm"
              disabled={!newGroupName.trim()}
              onClick={() => void importIntoNewGroup()}
            >
              新建并上传
            </button>
          </div>
          <div className="fs11 t3 mt10" style={{ lineHeight: 1.7 }}>
            点分组即开始上传；上传期间会显示逐张进度，请勿重复点击。
          </div>
        </Modal>
      )}

      {confirmDel && (
        <ConfirmModal
          title="删除参考图"
          desc="删除后进入废纸篓，清理后不可恢复。历史任务与作品的关联仍保留快照。"
          confirmLabel="删除"
          danger
          onConfirm={() => del(confirmDel)}
          onClose={() => setConfirmDel(null)}
        />
      )}

      {scan && (
        <Modal
          title="发现重复图片"
          width="w420"
          onClose={() => setScan(null)}
          footer={
            <>
              <div className="f1" />
              <button type="button" className="btn sm gho" onClick={proceedImportAll}>
                全部仍导入
              </button>
              <button type="button" className="btn pri sm" onClick={proceedSkipDup}>
                跳过重复并继续
              </button>
            </>
          }
        >
          <div className="fs12 t2" style={{ lineHeight: 1.7 }}>
            {scan.filter((i) => i.duplicate).length} / {scan.length} 张与库内已有图片内容相同。
            默认跳过重复项，仅导入其余 {scan.filter((i) => !i.duplicate).length} 张。
          </div>
          <div
            className="mt10 mlist"
            style={{
              maxHeight: 260,
              overflow: "auto",
              border: "1px solid var(--line)",
              borderRadius: 9,
            }}
          >
            {scan.map((i) => (
              <div key={i.path} className="fx ac gap9" style={{ padding: "8px 11px" }}>
                <span className="mono fs11 f1 nowrap ohide">{i.name}</span>
                {i.duplicate ? (
                  <span className="bdg b-amber">重复{i.dupOf ? ` · 同「${i.dupOf}」` : ""}</span>
                ) : (
                  <span className="bdg b-gray">新图</span>
                )}
              </div>
            ))}
          </div>
        </Modal>
      )}

      {batchPick && (
        <Modal
          title={`将 ${sel.size} 张移动到分组`}
          width="w420"
          onClose={() => setBatchPick(false)}
        >
          <div className="mlist" style={{ maxHeight: 320, overflow: "auto" }}>
            <div className="pickrow" onClick={() => void batchSetGroup(null)}>
              <span className="ckb" />
              <span className="fw5 f1 nowrap ohide">未分组</span>
            </div>
            {groups.map((g) => (
              <div key={g.id} className="pickrow" onClick={() => void batchSetGroup(g.id)}>
                <span className="ckb" />
                <i className="gdot" style={{ background: "var(--acc)" }} />
                <span className="fw5 f1 nowrap ohide">{g.name}</span>
                <span className="fs11 t3">{g.count}</span>
              </div>
            ))}
          </div>
        </Modal>
      )}

      {confirmBatchDel && (
        <ConfirmModal
          title={`删除 ${sel.size} 张参考图`}
          desc="删除后进入废纸篓，清理后不可恢复。历史任务与作品的关联仍保留快照。"
          confirmLabel="删除"
          danger
          onConfirm={batchDelete}
          onClose={() => setConfirmBatchDel(false)}
        />
      )}
    </PageScaffold>
  );
}

/** 分组管理：新建 / 就地改名 / 删除（组内图片回未分组）。 */
function ManageGroupsModal({
  groups,
  unassigned,
  onCreate,
  onRename,
  onDelete,
  onClose,
}: {
  groups: RefGroupView[];
  unassigned: number;
  onCreate: (name: string) => void;
  onRename: (id: number, name: string) => void;
  onDelete: (g: RefGroupView) => void;
  onClose: () => void;
}) {
  const [name, setName] = useState("");
  const [editing, setEditing] = useState<number | null>(null);
  const [draft, setDraft] = useState("");

  const submitRename = (id: number) => {
    const n = draft.trim();
    setEditing(null);
    if (n && n !== groups.find((g) => g.id === id)?.name) onRename(id, n);
  };

  return (
    <Modal title="管理图库分组" width="w420" onClose={onClose}>
      <div className="fx ac gap8">
        <input
          className="inp f1"
          placeholder="新建分组名…"
          value={name}
          onChange={(e) => setName(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && name.trim()) {
              onCreate(name);
              setName("");
            }
          }}
        />
        <button
          type="button"
          className="btn sm"
          disabled={!name.trim()}
          onClick={() => {
            onCreate(name);
            setName("");
          }}
        >
          <FolderPlus className="ic12" />
          新建
        </button>
      </div>

      <div
        className="mt10 mlist"
        style={{
          maxHeight: 320,
          overflow: "auto",
          border: "1px solid var(--line)",
          borderRadius: 9,
        }}
      >
        {groups.map((g) => (
          <div key={g.id} className="fx ac gap9" style={{ padding: "7px 11px" }}>
            <i className="gdot" style={{ background: "var(--acc)" }} />
            {editing === g.id ? (
              <input
                className="inp f1"
                value={draft}
                // biome-ignore lint/a11y/noAutofocus: 点了「重命名」才出现的就地输入框，聚焦符合预期
                autoFocus
                onChange={(e) => setDraft(e.target.value)}
                onBlur={() => submitRename(g.id)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") submitRename(g.id);
                  if (e.key === "Escape") setEditing(null);
                }}
              />
            ) : (
              <span className="fw5 f1 nowrap ohide">{g.name}</span>
            )}
            <span className="fs11 t3 nowrap">{g.count} 张</span>
            <button
              type="button"
              className="icb"
              title="重命名"
              onClick={() => {
                setEditing(g.id);
                setDraft(g.name);
              }}
            >
              <Pencil className="ic12" />
            </button>
            <button type="button" className="icb" title="删除分组" onClick={() => onDelete(g)}>
              <Trash2 className="ic12" />
            </button>
          </div>
        ))}
        <div className="fx ac gap9" style={{ padding: "7px 11px" }}>
          <span className="fw5 f1 nowrap ohide t3">未分组</span>
          <span className="fs11 t3 nowrap">{unassigned} 张</span>
        </div>
      </div>

      <div className="fs11 t3 mt10" style={{ lineHeight: 1.7 }}>
        分组是参考图库自己的目录，与提示词组彼此独立。删除分组不删图，组内图片回到「未分组」。
      </div>
    </Modal>
  );
}

function bg(path?: string | null): React.CSSProperties {
  const src = assetSrc(path);
  return src
    ? { backgroundImage: `url(${src})`, backgroundSize: "cover", backgroundPosition: "center" }
    : {};
}
