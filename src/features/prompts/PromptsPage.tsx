import { ConfirmModal, Modal } from "@/components/ui/Modal";
import { ImportPreviewModal } from "@/features/_shared/ImportPreviewModal";
import { PageScaffold } from "@/features/_shared/PageScaffold";
import {
  type GroupView,
  type ImportPreview,
  type PromptView,
  commands,
  subscribeFileDrop,
  unwrap,
} from "@/lib/ipc";
import { cn, promptLabel } from "@/lib/utils";
import { CheckSquare, FileUp, FolderInput, MoreHorizontal, Plus, Search, Star } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { toast } from "sonner";

/** 提取路径列表中首个 .txt（拖拽导入）。 */
function firstTxt(paths: string[]): string | undefined {
  return paths.find((p) => p.toLowerCase().endsWith(".txt"));
}

export function PromptsPage() {
  const [groups, setGroups] = useState<GroupView[]>([]);
  const [byGroup, setByGroup] = useState<Record<number, PromptView[]>>({});
  const [query, setQuery] = useState("");
  const [searchResults, setSearchResults] = useState<PromptView[] | null>(null);
  const [detailId, setDetailId] = useState<number | null>(null);
  const [importPreview, setImportPreview] = useState<ImportPreview | null>(null);
  const [tagFilter, setTagFilter] = useState<string | null>(null);

  // E36 多选态
  const [selectMode, setSelectMode] = useState(false);
  const [sel, setSel] = useState<Set<number>>(new Set());
  const lastClicked = useRef<number | null>(null);

  // E20 分组管理弹窗
  const [renaming, setRenaming] = useState<GroupView | null>(null);
  const [merging, setMerging] = useState<GroupView | null>(null);
  const [deleting, setDeleting] = useState<GroupView | null>(null);
  const [creating, setCreating] = useState(false);
  const [movePicker, setMovePicker] = useState<{ ids: number[] } | null>(null);

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

  // 全部标签（去重，来自各分组绑定）。
  const allTags = Array.from(new Set(groups.flatMap((g) => g.tags))).sort();
  // 应用标签筛选后的分组。
  const shownGroups = tagFilter == null ? groups : groups.filter((g) => g.tags.includes(tagFilter));

  // 多选：当前可选提示词的扁平序（用于 shift 范围）。搜索态下为搜索结果，否则为 shownGroups 顺序。
  const flat: PromptView[] = searchResults
    ? searchResults
    : shownGroups.flatMap((g) => byGroup[g.id] ?? []);

  const clearSel = () => {
    setSel(new Set());
    lastClicked.current = null;
  };
  const exitSelect = () => {
    setSelectMode(false);
    clearSel();
  };

  const onChipClick = (globalIdx: number, id: number, shift: boolean) => {
    if (!selectMode) {
      setDetailId(id);
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

  const parsePath = useCallback(async (path: string) => {
    const preview = await unwrap(commands.parsePromptTxt(path)).catch((e) => {
      toast.error(String(e));
      return null;
    });
    if (preview) setImportPreview(preview);
  }, []);

  const doImport = async () => {
    const path = await unwrap(commands.pickTxtFile()).catch(() => null);
    if (!path) return;
    await parsePath(path);
  };

  // E14：拖拽 .txt 进本页 → 走同一预览确认。
  useEffect(() => {
    let un = () => {};
    void subscribeFileDrop((paths) => {
      const txt = firstTxt(paths);
      if (txt) void parsePath(txt);
      else if (paths.length > 0) toast.error("提示词库仅支持拖入 .txt 文件");
    }).then((f) => {
      un = f;
    });
    return () => un();
  }, [parsePath]);

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

  // ── 批量操作（E36）────────────────────────────────────────────
  const batchFavorite = async () => {
    const ids = [...sel];
    await unwrap(commands.setPromptsFavorite(ids, true)).catch((e) => toast.error(String(e)));
    toast.success(`已收藏 ${ids.length} 条`);
    exitSelect();
    void load();
  };
  const batchDelete = async () => {
    const ids = [...sel];
    await unwrap(commands.trashPrompts(ids)).catch((e) => toast.error(String(e)));
    toast(`已移入废纸篓 ${ids.length} 条`);
    exitSelect();
    void load();
  };
  const batchMove = () => setMovePicker({ ids: [...sel] });
  const doMove = async (groupId: number) => {
    const ids = movePicker?.ids ?? [];
    await unwrap(commands.movePromptsToGroup(ids, groupId)).catch((e) => toast.error(String(e)));
    toast.success(`已移动 ${ids.length} 条`);
    setMovePicker(null);
    exitSelect();
    void load();
  };

  // ── 分组管理（E20）────────────────────────────────────────────
  const doRename = async (name: string) => {
    if (!renaming) return;
    await unwrap(commands.renamePromptGroup(renaming.id, name)).catch((e) =>
      toast.error(String(e)),
    );
    setRenaming(null);
    void load();
  };
  const doMerge = async (intoId: number) => {
    if (!merging) return;
    await unwrap(commands.mergePromptGroups(merging.id, intoId)).catch((e) =>
      toast.error(String(e)),
    );
    toast.success("已合并分组");
    setMerging(null);
    void load();
  };
  const doDelete = async () => {
    if (!deleting) return;
    await unwrap(commands.deletePromptGroup(deleting.id)).catch((e) => toast.error(String(e)));
    toast(`已删除分组「${deleting.name}」`);
    setDeleting(null);
    void load();
  };
  const doCreate = async (name: string) => {
    await unwrap(commands.createPromptGroup(name)).catch((e) => toast.error(String(e)));
    setCreating(false);
    void load();
  };

  // 扁平索引映射：为 shift 范围提供全局序号。
  let cursor = 0;

  return (
    <PageScaffold title="提示词库" caption={`${total} 条`}>
      <div className="phd" style={{ borderBottom: "none", minHeight: 0, paddingTop: 8 }}>
        <div className="srch">
          <Search className="ic12" />
          <input
            className="inp"
            placeholder="搜索编号或正文…"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
        </div>
        <div className="f1" />
        {selectMode ? (
          <>
            <span className="fs12 t2 nowrap">已选 {sel.size}</span>
            <button
              type="button"
              className="btn sm"
              disabled={sel.size === 0}
              onClick={batchFavorite}
            >
              <Star className="ic12" />
              收藏
            </button>
            <button type="button" className="btn sm" disabled={sel.size === 0} onClick={batchMove}>
              <FolderInput className="ic12" />
              移动到分组
            </button>
            <button
              type="button"
              className="btn sm gho dng"
              disabled={sel.size === 0}
              onClick={batchDelete}
            >
              删除
            </button>
            <button type="button" className="btn sm gho" onClick={exitSelect}>
              退出多选
            </button>
          </>
        ) : (
          <>
            <button
              type="button"
              className="btn sm gho"
              onClick={() => setSelectMode(true)}
              title="多选提示词做批量操作"
            >
              <CheckSquare className="ic12" />
              多选
            </button>
            <button type="button" className="btn sm gho" onClick={() => setCreating(true)}>
              <Plus className="ic12" />
              新建分组
            </button>
            <button type="button" className="btn sm" onClick={doImport}>
              <FileUp className="ic12" />
              导入 .txt
            </button>
          </>
        )}
      </div>

      {allTags.length > 0 && !searchResults && (
        <div className="tagbar">
          <button
            type="button"
            className={cn("tagchip", tagFilter == null && "on")}
            onClick={() => setTagFilter(null)}
          >
            全部
          </button>
          {allTags.map((t) => (
            <button
              key={t}
              type="button"
              className={cn("tagchip", tagFilter === t && "on")}
              onClick={() => setTagFilter((cur) => (cur === t ? null : t))}
            >
              {t}
            </button>
          ))}
        </div>
      )}

      <div className="pbody">
        {searchResults ? (
          <div className="gsec">
            <div className="ghead">
              <span className="fw6 fs13">搜索结果</span>
              <span className="fs11 t3">{searchResults.length}</span>
            </div>
            <div className="pgrid">
              {searchResults.map((p, i) => (
                <PChip
                  key={p.id}
                  p={p}
                  selected={sel.has(p.id)}
                  onClick={(shift) => onChipClick(i, p.id, shift)}
                />
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
        ) : shownGroups.length === 0 ? (
          <div className="bigempty">
            <div className="fs13 fw5 t2">没有匹配「{tagFilter}」的分组</div>
            <button type="button" className="btn mt10" onClick={() => setTagFilter(null)}>
              清除筛选
            </button>
          </div>
        ) : (
          shownGroups.map((g) => {
            const ps = byGroup[g.id] ?? [];
            const base = cursor;
            cursor += ps.length;
            return (
              <div className="gsec" key={g.id}>
                <div className="ghead">
                  <i className="gdot" style={{ background: "var(--acc)" }} />
                  <span className="fw6 fs13 nowrap">{g.name}</span>
                  <span className="chip">{g.prefix}</span>
                  {g.scene && <span className="bdg b-gray">{g.scene}</span>}
                  {g.isTemp && <span className="bdg b-amber">临时 · 待验收入库</span>}
                  {g.tags.map((t) => (
                    <span key={t} className="bdg b-gray">
                      #{t}
                    </span>
                  ))}
                  <div className="f1" />
                  <span className="fs11 t3 nowrap">{g.count}</span>
                  {!selectMode && (
                    <GroupMenu
                      onRename={() => setRenaming(g)}
                      onMerge={() => setMerging(g)}
                      onDelete={() => setDeleting(g)}
                      canMerge={groups.length > 1}
                    />
                  )}
                </div>
                <div className="pgrid">
                  {ps.map((p, i) => (
                    <PChip
                      key={p.id}
                      p={p}
                      selected={sel.has(p.id)}
                      onClick={(shift) => onChipClick(base + i, p.id, shift)}
                    />
                  ))}
                </div>
              </div>
            );
          })
        )}
      </div>

      {detailId != null && (
        <PromptDetail
          id={detailId}
          groups={groups}
          onClose={() => setDetailId(null)}
          onChanged={load}
        />
      )}

      {creating && (
        <NameModal
          title="新建分组"
          placeholder="分组名（自动生成编号前缀）"
          confirmLabel="创建"
          onConfirm={doCreate}
          onClose={() => setCreating(false)}
        />
      )}
      {renaming && (
        <NameModal
          title="重命名分组"
          initial={renaming.name}
          confirmLabel="保存"
          onConfirm={doRename}
          onClose={() => setRenaming(null)}
        />
      )}
      {merging && (
        <GroupPickerModal
          title={`合并「${merging.name}」到`}
          desc="源分组的提示词与参考图将并入所选分组，编号保持不变，源分组随后删除。"
          groups={groups.filter((g) => g.id !== merging.id)}
          onPick={doMerge}
          onClose={() => setMerging(null)}
        />
      )}
      {movePicker && (
        <GroupPickerModal
          title={`移动 ${movePicker.ids.length} 条提示词到`}
          groups={groups}
          onPick={doMove}
          onClose={() => setMovePicker(null)}
        />
      )}
      {deleting && (
        <ConfirmModal
          title={`删除分组「${deleting.name}」`}
          desc={`组内 ${deleting.count} 条提示词将移入废纸篓（清理时回收编号）。已生成的作品不受影响。`}
          confirmLabel="删除分组"
          danger
          onConfirm={doDelete}
          onClose={() => setDeleting(null)}
        />
      )}

      {importPreview && (
        <ImportPreviewModal
          preview={importPreview}
          confirmLabel={`导入 ${importPreview.total} 条`}
          onConfirm={confirmImport}
          onClose={() => setImportPreview(null)}
        />
      )}
    </PageScaffold>
  );
}

function PChip({
  p,
  selected,
  onClick,
}: { p: PromptView; selected: boolean; onClick: (shift: boolean) => void }) {
  return (
    <button
      type="button"
      className={cn("pchip", selected && "sel")}
      onClick={(e) => onClick(e.shiftKey)}
      title={p.text.slice(0, 88)}
    >
      {p.favorite && <i className="favdot" />}
      {promptLabel(p.code, p.title)}
    </button>
  );
}

/** 分组操作菜单（E20：重命名 / 合并 / 删除）。 */
function GroupMenu({
  onRename,
  onMerge,
  onDelete,
  canMerge,
}: { onRename: () => void; onMerge: () => void; onDelete: () => void; canMerge: boolean }) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    if (!open) return;
    const onDoc = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener("mousedown", onDoc);
    return () => window.removeEventListener("mousedown", onDoc);
  }, [open]);
  const pick = (fn: () => void) => () => {
    setOpen(false);
    fn();
  };
  return (
    <div className="gwrap" ref={ref}>
      <button
        type="button"
        className="icb"
        onClick={() => setOpen((v) => !v)}
        aria-label="分组操作"
      >
        <MoreHorizontal className="ic12" />
      </button>
      {open && (
        <div className="gmenu">
          <button type="button" className="gmi" onClick={pick(onRename)}>
            重命名
          </button>
          <button
            type="button"
            className="gmi"
            onClick={pick(onMerge)}
            disabled={!canMerge}
            title={canMerge ? undefined : "至少两个分组才能合并"}
          >
            合并到…
          </button>
          <button type="button" className="gmi dng" onClick={pick(onDelete)}>
            删除分组
          </button>
        </div>
      )}
    </div>
  );
}

/** 名称输入弹窗（新建 / 重命名分组）。 */
function NameModal({
  title,
  initial = "",
  placeholder = "",
  confirmLabel,
  onConfirm,
  onClose,
}: {
  title: string;
  initial?: string;
  placeholder?: string;
  confirmLabel: string;
  onConfirm: (name: string) => void;
  onClose: () => void;
}) {
  const [name, setName] = useState(initial);
  const submit = () => {
    const v = name.trim();
    if (v) onConfirm(v);
  };
  return (
    <Modal
      title={title}
      width="w360"
      onClose={onClose}
      footer={
        <>
          <div className="f1" />
          <button type="button" className="btn sm" onClick={onClose}>
            取消
          </button>
          <button type="button" className="btn pri sm" onClick={submit} disabled={!name.trim()}>
            {confirmLabel}
          </button>
        </>
      }
    >
      <input
        className="inp"
        style={{ width: "100%" }}
        // biome-ignore lint/a11y/noAutofocus: 弹窗打开即聚焦输入符合预期
        autoFocus
        placeholder={placeholder}
        value={name}
        onChange={(e) => setName(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") submit();
        }}
      />
    </Modal>
  );
}

/** 分组选择弹窗（合并目标 / 移动目标）。 */
function GroupPickerModal({
  title,
  desc,
  groups,
  onPick,
  onClose,
}: {
  title: string;
  desc?: string;
  groups: GroupView[];
  onPick: (groupId: number) => void;
  onClose: () => void;
}) {
  return (
    <Modal title={title} width="w420" onClose={onClose}>
      {desc && (
        <div className="fs12 t2" style={{ lineHeight: 1.7, marginBottom: 10 }}>
          {desc}
        </div>
      )}
      {groups.length === 0 ? (
        <div className="fs12 t3">没有可选分组。</div>
      ) : (
        <div style={{ border: "1px solid var(--line)", borderRadius: 9, overflow: "hidden" }}>
          {groups.map((g) => (
            <button
              key={g.id}
              type="button"
              className="fx ac gap9 gmi"
              style={{ width: "100%", padding: "10px 12px", borderRadius: 0 }}
              onClick={() => onPick(g.id)}
            >
              <i className="gdot" style={{ background: "var(--acc)" }} />
              <span className="fw5 fs12 nowrap">{g.name}</span>
              <span className="chip">{g.prefix}</span>
              {g.isTemp && <span className="bdg b-amber">临时</span>}
              <div className="f1" />
              <span className="fs11 t3">{g.count}</span>
            </button>
          ))}
        </div>
      )}
    </Modal>
  );
}

function PromptDetail({
  id,
  groups,
  onClose,
  onChanged,
}: {
  id: number;
  groups: GroupView[];
  onClose: () => void;
  onChanged: () => Promise<void>;
}) {
  const [p, setP] = useState<PromptView | null>(null);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");
  const [confirmDel, setConfirmDel] = useState(false);
  const [moving, setMoving] = useState(false);

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
  const move = async (groupId: number) => {
    await unwrap(commands.movePromptsToGroup([id], groupId)).catch((e) => toast.error(String(e)));
    setMoving(false);
    onClose();
    toast.success("已移动到分组");
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
          <button type="button" className="btn sm" onClick={() => setMoving(true)}>
            移动到分组
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
      {moving && (
        <GroupPickerModal
          title="移动到分组"
          groups={groups.filter((g) => g.id !== p.groupId)}
          onPick={move}
          onClose={() => setMoving(false)}
        />
      )}
    </Modal>
  );
}
