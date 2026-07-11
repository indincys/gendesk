import { ConfirmModal, Modal } from "@/components/ui/Modal";
import { PageScaffold } from "@/features/_shared/PageScaffold";
import { assetSrc } from "@/lib/img";
import {
  type GroupView,
  type RefImageDetail,
  type RefImageView,
  commands,
  subscribeFileDrop,
  unwrap,
} from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { Upload } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { toast } from "sonner";

export function RefsPage() {
  const [refs, setRefs] = useState<RefImageView[]>([]);
  const [groups, setGroups] = useState<GroupView[]>([]);
  const [detail, setDetail] = useState<RefImageDetail | null>(null);
  const [confirmDel, setConfirmDel] = useState<RefImageDetail | null>(null);
  // E30a：导入时先选分组。pendingPaths 非空即展示选组弹窗。
  const [pendingPaths, setPendingPaths] = useState<string[] | null>(null);
  const [newGroupName, setNewGroupName] = useState("");
  const [importing, setImporting] = useState(false);

  const load = useCallback(async () => {
    try {
      setRefs(await unwrap(commands.listRefImages()));
      setGroups(await unwrap(commands.listPromptGroups()));
    } catch (e) {
      if (e instanceof Error) toast.error(e.message);
    }
  }, []);
  useEffect(() => {
    void load();
  }, [load]);

  // 第一步：选文件；有文件则弹出选组弹窗（E30a）。
  const importRefs = async () => {
    const paths = await unwrap(commands.pickImageFiles()).catch(() => [] as string[]);
    if (paths.length === 0) return;
    setNewGroupName("");
    setPendingPaths(paths);
  };

  // E14：拖拽图片进参考图库 → 走同一选组弹窗。
  useEffect(() => {
    let un = () => {};
    void subscribeFileDrop((paths) => {
      const images = paths.filter((p) => /\.(png|jpe?g|webp|bmp)$/i.test(p));
      if (images.length > 0) {
        setNewGroupName("");
        setPendingPaths(images);
      } else if (paths.length > 0) {
        toast.error("参考图库仅支持拖入图片文件");
      }
    }).then((f) => {
      un = f;
    });
    return () => un();
  }, []);

  // 第二步：带选定分组导入。gid=null 为未分组。
  const doImport = async (gid: number | null) => {
    const paths = pendingPaths;
    if (!paths || importing) return;
    setImporting(true);
    try {
      await unwrap(commands.importRefImages(paths, gid));
      toast(`已导入 ${paths.length} 张参考图`);
      setPendingPaths(null);
      void load();
    } catch (e) {
      if (e instanceof Error) toast.error(e.message);
    } finally {
      setImporting(false);
    }
  };

  // 新建分组后立即用它导入。
  const importIntoNewGroup = async () => {
    const name = newGroupName.trim();
    if (!name || importing) return;
    try {
      const g = await unwrap(commands.createPromptGroup(name));
      setGroups((gs) => [...gs, g]);
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

  const del = async (d: RefImageDetail) => {
    await unwrap(commands.trashRefImage(d.id)).catch(() => {});
    setDetail(null);
    void load();
    toast("已移入废纸篓");
  };

  const byGroup = (gid: number | null) => refs.filter((r) => (r.groupId ?? null) === gid);
  const sections: { name: string; gid: number | null; items: RefImageView[] }[] = [
    ...groups.map((g) => ({ name: g.name, gid: g.id as number | null, items: byGroup(g.id) })),
    { name: "未分组", gid: null, items: byGroup(null) },
  ].filter((s) => s.items.length > 0);

  return (
    <PageScaffold title="参考图库" caption="与提示词库共用同一套分组体系">
      <div className="phd" style={{ borderBottom: "none", minHeight: 0, paddingTop: 8 }}>
        <span className="cnt">{refs.length} 张</span>
        <div className="f1" />
        <button type="button" className="btn sm" onClick={importRefs}>
          <Upload className="ic12" />
          导入参考图
        </button>
      </div>

      {refs.length === 0 ? (
        <div className="bigempty">
          <div className="fs13 fw5 t2">参考图库为空</div>
          <div className="fs12 t3">导入参考图作为可复用素材，后续在生成页挂靠提示词组</div>
          <button type="button" className="btn mt10" onClick={importRefs}>
            导入参考图
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
                {s.items.map((r) => (
                  <div key={r.id} className="rcard" onClick={() => openDetail(r.id)}>
                    <div className="ph rcimg" style={bg(r.thumbPath)} />
                    <div className="rmeta">
                      <span className="mono fs11 fw5 nowrap ohide f1">{r.name}</span>
                    </div>
                  </div>
                ))}
              </div>
            </div>
          ))}
        </div>
      )}

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
                  </div>
                ))}
              </div>
              <div className="fs11 t3 mt10" style={{ lineHeight: 1.7 }}>
                参考图与提示词库共用分组体系；调整分组不影响历史任务与作品的关联。
              </div>
            </div>
          </div>
        </Modal>
      )}

      {pendingPaths && (
        <Modal
          title={`导入 ${pendingPaths.length} 张参考图 · 选择分组`}
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
                <span className="fs11 t3">{g.prefix}</span>
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
              disabled={!newGroupName.trim() || importing}
              onClick={() => void importIntoNewGroup()}
            >
              新建并导入
            </button>
          </div>
          <div className="fs11 t3 mt10" style={{ lineHeight: 1.7 }}>
            参考图与提示词库共用分组体系；点分组即导入，也可先新建分组。
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
    </PageScaffold>
  );
}

function bg(path?: string | null): React.CSSProperties {
  const src = assetSrc(path);
  return src
    ? { backgroundImage: `url(${src})`, backgroundSize: "cover", backgroundPosition: "center" }
    : {};
}
