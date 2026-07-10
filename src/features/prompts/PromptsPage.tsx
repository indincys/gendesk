import { ConfirmModal, Modal } from "@/components/ui/Modal";
import { PageScaffold } from "@/features/_shared/PageScaffold";
import { type GroupView, type ImportPreview, type PromptView, commands, unwrap } from "@/lib/ipc";
import { cn, promptLabel } from "@/lib/utils";
import { FileUp, Search, Star } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { toast } from "sonner";

export function PromptsPage() {
  const [groups, setGroups] = useState<GroupView[]>([]);
  const [byGroup, setByGroup] = useState<Record<number, PromptView[]>>({});
  const [query, setQuery] = useState("");
  const [searchResults, setSearchResults] = useState<PromptView[] | null>(null);
  const [detailId, setDetailId] = useState<number | null>(null);
  const [importPreview, setImportPreview] = useState<ImportPreview | null>(null);

  const load = useCallback(async () => {
    try {
      const gs = await unwrap(commands.listPromptGroups());
      setGroups(gs);
      const map: Record<number, PromptView[]> = {};
      for (const g of gs) map[g.id] = await unwrap(commands.listPrompts(g.id));
      setByGroup(map);
    } catch (e) {
      if (e instanceof Error) toast.error(e.message);
    }
  }, []);
  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    const q = query.trim();
    if (!q) {
      setSearchResults(null);
      return;
    }
    let live = true;
    void unwrap(commands.searchPrompts(q))
      .then((r) => live && setSearchResults(r))
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [query]);

  const total = Object.values(byGroup).reduce((n, ps) => n + ps.length, 0);

  const doImport = async () => {
    const path = await unwrap(commands.pickTxtFile()).catch(() => null);
    if (!path) return;
    const preview = await unwrap(commands.parsePromptTxt(path)).catch((e) => {
      toast.error(String(e));
      return null;
    });
    if (preview) setImportPreview(preview);
  };

  const confirmImport = async () => {
    if (!importPreview) return;
    const res = await unwrap(commands.commitPromptImport(importPreview, "library")).catch((e) => {
      toast.error(String(e));
      return null;
    });
    if (res) {
      toast.success(`已导入 ${res.inserted} 条`);
      setImportPreview(null);
      void load();
    }
  };

  return (
    <PageScaffold title="提示词库" caption={`${total} 条`}>
      <div className="phd" style={{ borderBottom: "none", minHeight: 0, paddingTop: 8 }}>
        <div className="f1" />
        <div className="srch">
          <Search className="ic12" />
          <input
            className="inp"
            placeholder="搜索编号或正文…"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
        </div>
        <button type="button" className="btn sm" onClick={doImport}>
          <FileUp className="ic12" />
          导入 .txt
        </button>
      </div>

      <div className="pbody">
        {searchResults ? (
          <div className="gsec">
            <div className="ghead">
              <span className="fw6 fs13">搜索结果</span>
              <span className="fs11 t3">{searchResults.length}</span>
            </div>
            <div className="pgrid">
              {searchResults.map((p) => (
                <PChip key={p.id} p={p} onOpen={() => setDetailId(p.id)} />
              ))}
            </div>
          </div>
        ) : groups.length === 0 ? (
          <div className="bigempty">
            <div className="fs13 fw5 t2">提示词库为空</div>
            <div className="fs12 t3">导入 .txt 自动解析「分组 / 标签 / 正文」并按前缀编号</div>
            <button type="button" className="btn mt10" onClick={doImport}>
              导入 .txt
            </button>
          </div>
        ) : (
          groups.map((g) => (
            <div className="gsec" key={g.id}>
              <div className="ghead">
                <i className="gdot" style={{ background: "var(--acc)" }} />
                <span className="fw6 fs13 nowrap">{g.name}</span>
                <span className="chip">{g.prefix}</span>
                {g.scene && <span className="bdg b-gray">{g.scene}</span>}
                {g.isTemp && <span className="bdg b-amber">临时 · 待验收入库</span>}
                <div className="f1" />
                <span className="fs11 t3 nowrap">{g.count}</span>
              </div>
              <div className="pgrid">
                {(byGroup[g.id] ?? []).map((p) => (
                  <PChip key={p.id} p={p} onOpen={() => setDetailId(p.id)} />
                ))}
              </div>
            </div>
          ))
        )}
      </div>

      {detailId != null && (
        <PromptDetail id={detailId} onClose={() => setDetailId(null)} onChanged={load} />
      )}
      {importPreview && (
        <Modal
          title="导入提示词 .txt"
          onClose={() => setImportPreview(null)}
          footer={
            <>
              <div className="f1" />
              <button type="button" className="btn" onClick={() => setImportPreview(null)}>
                取消
              </button>
              <button type="button" className="btn pri" onClick={confirmImport}>
                导入 {importPreview.total} 条
              </button>
            </>
          }
        >
          <div className="fx ac gap8">
            <span className="chip">{importPreview.encoding}</span>
            <span className="fs11 t3">解析成功</span>
          </div>
          <div
            className="mt10"
            style={{ border: "1px solid var(--line)", borderRadius: 9, overflow: "hidden" }}
          >
            {importPreview.groups.map((o) => (
              <div key={o.prefix} className="fx ac gap9" style={{ padding: "9px 11px" }}>
                <i className="gdot" style={{ background: "var(--wr)" }} />
                <span className="fw5 fs12 nowrap">{o.name}</span>
                <span className="chip">{o.prefix}</span>
                <span className="t3 fs11 f1 nowrap ohide">{o.tags.join(" · ")}</span>
                <span className="chip">{o.codeRange}</span>
                <span className="t3 fs11 nowrap">{o.count} 条</span>
              </div>
            ))}
          </div>
        </Modal>
      )}
    </PageScaffold>
  );
}

function PChip({ p, onOpen }: { p: PromptView; onOpen: () => void }) {
  return (
    <button type="button" className="pchip" onClick={onOpen} title={p.text.slice(0, 88)}>
      {p.favorite && <i className="favdot" />}
      {promptLabel(p.code, p.title)}
    </button>
  );
}

function PromptDetail({
  id,
  onClose,
  onChanged,
}: { id: number; onClose: () => void; onChanged: () => Promise<void> }) {
  const [p, setP] = useState<PromptView | null>(null);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");
  const [confirmDel, setConfirmDel] = useState(false);

  useEffect(() => {
    void unwrap(commands.getPrompt(id))
      .then((r) => {
        setP(r);
        setDraft(r.text);
      })
      .catch(() => {});
  }, [id]);

  if (!p) return null;

  const save = async () => {
    await unwrap(commands.updatePromptText(id, draft)).catch(() => {});
    setP({ ...p, text: draft, edited: true });
    setEditing(false);
    toast("已保存修改");
    void onChanged();
  };
  const toggleFav = async () => {
    await unwrap(commands.togglePromptFavorite(id)).catch(() => {});
    setP({ ...p, favorite: !p.favorite });
    void onChanged();
  };
  const del = async () => {
    await unwrap(commands.trashPrompt(id)).catch(() => {});
    onClose();
    toast("已移入废纸篓");
    void onChanged();
  };

  return (
    <Modal
      title={<span className="pid">{promptLabel(p.code, p.title)}</span>}
      width="w700"
      onClose={onClose}
      headerExtra={
        <>
          {p.edited && <span className="bdg b-blue">已微调</span>}
          <button
            type="button"
            className={cn("star", p.favorite && "on")}
            onClick={toggleFav}
            title="收藏"
          >
            <Star className="ic" fill={p.favorite ? "currentColor" : "none"} />
          </button>
        </>
      }
      footer={
        <>
          <div className="f1" />
          <button type="button" className="btn gho dng sm" onClick={() => setConfirmDel(true)}>
            删除
          </button>
          {editing ? (
            <>
              <button
                type="button"
                className="btn sm"
                onClick={() => {
                  setEditing(false);
                  setDraft(p.text);
                }}
              >
                取消
              </button>
              <button type="button" className="btn pri sm" onClick={save}>
                保存修改
              </button>
            </>
          ) : (
            <button type="button" className="btn sm" onClick={() => setEditing(true)}>
              编辑
            </button>
          )}
        </>
      }
    >
      {editing ? (
        <textarea className="ta" value={draft} onChange={(e) => setDraft(e.target.value)} />
      ) : (
        <div className="ptext">{p.text}</div>
      )}
      {confirmDel && (
        <ConfirmModal
          title="删除提示词"
          desc="删除后进入废纸篓，清理时编号将被回收。"
          confirmLabel="删除"
          danger
          onConfirm={del}
          onClose={() => setConfirmDel(false)}
        />
      )}
    </Modal>
  );
}
