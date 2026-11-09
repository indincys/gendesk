import { ConfirmModal, Modal } from "@/components/ui/Modal";
import { PageScaffold } from "@/features/_shared/PageScaffold";
import { RefImportOverlay, type RefImportState } from "@/features/_shared/RefImport";
import { assetSrc } from "@/lib/img";
import {
  type LibraryCategoryView,
  type LibraryImagePatch,
  type LibraryImageView,
  type LibraryOverview,
  type LibraryStyleView,
  commands,
  subscribeRefImportProgress,
  unwrap,
} from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { Dices, ImagePlus, Pencil, Plus, Settings2 } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { toast } from "sonner";

const NO_PATCH: LibraryImagePatch = { categoryId: null, enabled: null, note: null, removed: null };

/** 「11-14」这种短日期。 */
function shortDate(unix: number): string {
  const d = new Date(unix * 1000);
  return `${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
}

/** 卡片上的状态徽章：停用 / 已移出 / 冷却到哪天 / 可用。 */
function statusBadge(img: LibraryImageView): { text: string; tone: string } {
  if (img.removed) return { text: "已移出", tone: "b-gray" };
  if (!img.enabled) return { text: "停用", tone: "b-gray" };
  if (img.coolingUntil) return { text: `冷却至 ${shortDate(img.coolingUntil)}`, tone: "b-amber" };
  return { text: "可用", tone: "b-green" };
}

function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/**
 * 参考图库：按款式、分类长期保存参考图。工单清单里写 `{"category":"正面图"}`，
 * 收录时由应用随机抽图，冷却期内用过的图优先跳过（冷却天数按分类设）。
 */
export function RefLibraryPage() {
  const [overview, setOverview] = useState<LibraryOverview | null>(null);
  const [styleId, setStyleId] = useState<number | null>(null);
  const [categoryId, setCategoryId] = useState<number | null>(null);
  const [showRemoved, setShowRemoved] = useState(false);
  const [images, setImages] = useState<LibraryImageView[]>([]);
  const [sel, setSel] = useState<Set<number>>(new Set());
  const [styleEdit, setStyleEdit] = useState<LibraryStyleView | "new" | null>(null);
  const [catsOpen, setCatsOpen] = useState(false);
  const [importAsk, setImportAsk] = useState<string[] | null>(null);
  const [confirmRemove, setConfirmRemove] = useState(false);
  const [pickCount, setPickCount] = useState(3);
  const [importing, setImporting] = useState<RefImportState | null>(null);
  const busy = useRef(false);

  const refreshOverview = useCallback(async () => {
    try {
      const o = await unwrap(commands.libraryOverview());
      setOverview(o);
      setStyleId((cur) =>
        cur !== null && o.styles.some((s) => s.id === cur) ? cur : (o.styles[0]?.id ?? null),
      );
    } catch (e) {
      toast.error(`读取参考图库失败：${message(e)}`);
    }
  }, []);

  const refreshImages = useCallback(async () => {
    if (styleId === null) {
      setImages([]);
      return;
    }
    try {
      setImages(await unwrap(commands.listLibraryImages(styleId, categoryId, showRemoved)));
    } catch (e) {
      toast.error(`读取图片失败：${message(e)}`);
    }
  }, [styleId, categoryId, showRemoved]);

  const refresh = useCallback(async () => {
    await Promise.all([refreshOverview(), refreshImages()]);
  }, [refreshOverview, refreshImages]);

  useEffect(() => {
    void refreshOverview();
  }, [refreshOverview]);
  useEffect(() => {
    setSel(new Set());
    void refreshImages();
  }, [refreshImages]);

  const style = overview?.styles.find((s) => s.id === styleId) ?? null;
  const categories = overview?.categories ?? [];
  const category = categories.find((c) => c.id === categoryId) ?? null;
  const countOf = (cid: number) => style?.byCategory.find((c) => c.categoryId === cid);
  const selected = useMemo(() => images.filter((i) => sel.has(i.id)), [images, sel]);

  const toggle = (id: number) =>
    setSel((s) => {
      const next = new Set(s);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  async function runImport(paths: string[], cid: number) {
    if (styleId === null || busy.current) return;
    busy.current = true;
    setImporting({ done: 0, total: paths.length, name: "", failed: 0 });
    const unsub = await subscribeRefImportProgress((p) =>
      setImporting({ done: p.done, total: p.total, name: p.name, failed: p.failed }),
    );
    try {
      const r = await unwrap(commands.importLibraryImages(styleId, cid, paths));
      const parts = [`导入 ${r.imported.length} 张`];
      if (r.restored.length) parts.push(`放回 ${r.restored.length} 张`);
      if (r.duplicates.length) parts.push(`${r.duplicates.length} 张已在库里`);
      if (r.failed.length) parts.push(`${r.failed.length} 张读不了`);
      (r.failed.length ? toast.warning : toast.success)(parts.join("，"));
      setCategoryId(cid);
    } catch (e) {
      toast.error(`导入失败：${message(e)}`);
    } finally {
      unsub();
      setImporting(null);
      busy.current = false;
      await refresh();
    }
  }

  async function startImport() {
    if (!style) {
      toast.info("先新建一个款式");
      return;
    }
    const paths = await unwrap(commands.pickImageFiles()).catch(() => []);
    if (paths.length === 0) return;
    if (category) await runImport(paths, category.id);
    else setImportAsk(paths);
  }

  async function patch(p: Partial<LibraryImagePatch>, done: string) {
    try {
      const n = await unwrap(commands.updateLibraryImages([...sel], { ...NO_PATCH, ...p }));
      toast.success(`${done} ${n} 张`);
      setSel(new Set());
      await refresh();
    } catch (e) {
      toast.error(message(e));
    }
  }

  async function tryPick() {
    if (!style || !category) return;
    try {
      const r = await unwrap(commands.pickLibraryImages(style.id, category.id, pickCount, false));
      setSel(new Set(r.picks.map((p) => p.image.id)));
      toast.info(`试抽 ${r.picks.length} 张（已选中，不记为使用）：${r.message}`);
    } catch (e) {
      toast.error(message(e));
    }
  }

  const right = (
    <>
      <button type="button" className="btn sm gho" onClick={() => setCatsOpen(true)}>
        <Settings2 className="ic12" />
        分类与冷却
      </button>
      <button type="button" className="btn sm" onClick={() => setStyleEdit("new")}>
        <Plus className="ic12" />
        新建款式
      </button>
      <button type="button" className="btn sm pri" disabled={!style} onClick={startImport}>
        <ImagePlus className="ic12" />
        导入图片
      </button>
    </>
  );

  return (
    <PageScaffold
      title="参考图库"
      caption="工单按款式和分类随机抽图，冷却期内用过的图优先跳过"
      right={right}
      bodyClassName="rlwb"
    >
      <div className="rlside">
        {overview && overview.styles.length === 0 && (
          <div className="fs12 t3 rlhint">还没有款式。点「新建款式」，再导入它的参考图。</div>
        )}
        {overview?.styles.map((s) => (
          <div
            key={s.id}
            className={cn("rlstyle", s.id === styleId && "on")}
            onClick={() => setStyleId(s.id)}
            onKeyDown={(e) => e.key === "Enter" && setStyleId(s.id)}
            role="button"
            tabIndex={0}
          >
            <div className="f1 ohide">
              <div className="fs12 fw5 nowrap ohide">{s.name}</div>
              {s.product && <div className="fs10 t3 nowrap ohide">{s.product}</div>}
            </div>
            <span className="fs11 t3 mono nowrap" title="可用 / 总数">
              {s.available}/{s.total}
            </span>
            <button
              type="button"
              className="icb rledit"
              title="改名"
              onClick={(e) => {
                e.stopPropagation();
                setStyleEdit(s);
              }}
            >
              <Pencil className="ic12" />
            </button>
          </div>
        ))}
      </div>

      <div className="rlmain">
        <div className="fbar">
          <div className="seg">
            <span
              className={cn("sgi", categoryId === null && "on")}
              onClick={() => setCategoryId(null)}
            >
              全部 <span className="t3 mono">{style?.total ?? 0}</span>
            </span>
            {categories.map((c) => {
              const n = countOf(c.id);
              return (
                <span
                  key={c.id}
                  className={cn("sgi", categoryId === c.id && "on")}
                  onClick={() => setCategoryId(c.id)}
                  title={`可用 ${n?.available ?? 0} 张 · 冷却 ${c.cooldownDays} 天`}
                >
                  {c.name}{" "}
                  <span className="t3 mono">
                    {n?.available ?? 0}/{n?.total ?? 0}
                  </span>
                </span>
              );
            })}
          </div>
          <div className="f1" />
          <label className="fx ac gap6 fs12 t2">
            <input
              type="checkbox"
              checked={showRemoved}
              onChange={(e) => setShowRemoved(e.target.checked)}
            />
            显示已移出
          </label>
          {category && (
            <span className="fx ac gap6">
              <input
                className="inp sm rlnum"
                type="number"
                min={1}
                max={64}
                value={pickCount}
                onChange={(e) =>
                  setPickCount(Math.max(1, Math.min(64, Number(e.target.value) || 1)))
                }
              />
              <button
                type="button"
                className="btn sm"
                onClick={tryPick}
                title="按工单规则随机抽，不记为使用"
              >
                <Dices className="ic12" />
                试抽
              </button>
            </span>
          )}
        </div>

        {sel.size > 0 && (
          <div className="rlbar">
            <span className="fs12 fw5">已选 {sel.size} 张</span>
            <select
              className="inp sm"
              value=""
              onChange={(e) => {
                const cid = Number(e.target.value);
                if (cid) void patch({ categoryId: cid }, "已改分类");
              }}
            >
              <option value="">移到分类…</option>
              {categories.map((c) => (
                <option key={c.id} value={c.id}>
                  {c.name}
                </option>
              ))}
            </select>
            <button
              type="button"
              className="btn sm"
              onClick={() => patch({ enabled: false }, "已停用")}
            >
              停用
            </button>
            <button
              type="button"
              className="btn sm"
              onClick={() => patch({ enabled: true }, "已启用")}
            >
              启用
            </button>
            {selected.some((i) => i.removed) && (
              <button
                type="button"
                className="btn sm"
                onClick={() => patch({ removed: false }, "已放回")}
              >
                放回图库
              </button>
            )}
            {selected.some((i) => !i.removed) && (
              <button
                type="button"
                className="btn sm gho dng"
                onClick={() => setConfirmRemove(true)}
              >
                移出图库
              </button>
            )}
            <div className="f1" />
            <button type="button" className="btn sm gho" onClick={() => setSel(new Set())}>
              取消选择
            </button>
          </div>
        )}

        <div className="pbody">
          {!style ? (
            <div className="bigempty">
              <div className="fs13 fw5 t2">选择或新建一个款式</div>
            </div>
          ) : images.length === 0 ? (
            <div className="bigempty">
              <div className="fs13 fw5 t2">
                {style.name}
                {category ? ` · ${category.name}` : ""} 还没有图片
              </div>
              <div className="fs12 t3">点右上角「导入图片」，从本地选图放进图库。</div>
            </div>
          ) : (
            <div className="rlgrid">
              {images.map((img) => {
                const b = statusBadge(img);
                const src = assetSrc(img.thumbPath);
                return (
                  <div
                    key={img.id}
                    className={cn(
                      "rlcard",
                      sel.has(img.id) && "sel",
                      (!img.enabled || img.removed) && "off",
                    )}
                    onClick={() => toggle(img.id)}
                    onKeyDown={(e) => e.key === " " && toggle(img.id)}
                    role="button"
                    tabIndex={0}
                    title={img.filePath}
                  >
                    <div className="rlimg ph">
                      {src ? (
                        <img className="thumbimg" src={src} alt="" loading="lazy" />
                      ) : (
                        <div className="thumbfill" />
                      )}
                    </div>
                    <div className="rlmeta">
                      <span className={cn("bdg", b.tone)}>{b.text}</span>
                      <span className="f1" />
                      <span className="fs10 t3 mono">#{img.id}</span>
                    </div>
                    <div className="fs10 t3 nowrap ohide">
                      {categoryId === null ? `${img.category} · ` : ""}
                      {img.useCount > 0
                        ? `用过 ${img.useCount} 次 · 上次 ${shortDate(img.lastUsedAt ?? 0)}`
                        : "还没用过"}
                    </div>
                  </div>
                );
              })}
            </div>
          )}
        </div>
      </div>

      {styleEdit && (
        <StyleModal
          style={styleEdit === "new" ? null : styleEdit}
          onClose={() => setStyleEdit(null)}
          onSaved={async (id) => {
            setStyleEdit(null);
            setStyleId(id);
            await refreshOverview();
          }}
          onDeleted={async () => {
            setStyleEdit(null);
            setStyleId(null);
            await refreshOverview();
          }}
        />
      )}
      {catsOpen && (
        <CategoriesModal
          categories={categories}
          onClose={() => setCatsOpen(false)}
          onChanged={refresh}
        />
      )}
      {importAsk && (
        <Modal title="导入到哪个分类" onClose={() => setImportAsk(null)}>
          <div className="fs12 t2">
            {importAsk.length} 张图导入「{style?.name}」，选一个分类：
          </div>
          <div className="fx ac gap8 mt10 wrap">
            {categories.map((c) => (
              <button
                key={c.id}
                type="button"
                className="btn sm"
                onClick={() => {
                  const paths = importAsk;
                  setImportAsk(null);
                  void runImport(paths, c.id);
                }}
              >
                {c.name}
              </button>
            ))}
          </div>
        </Modal>
      )}
      {confirmRemove && (
        <ConfirmModal
          title="移出图库"
          desc={`移出 ${selected.filter((i) => !i.removed).length} 张图：之后不再被工单抽到，文件保留，可在「显示已移出」里放回。`}
          confirmLabel="移出"
          danger
          onClose={() => setConfirmRemove(false)}
          onConfirm={() => {
            setConfirmRemove(false);
            void patch({ removed: true }, "已移出");
          }}
        />
      )}
      {importing && <RefImportOverlay state={importing} title="导入参考图库" />}
    </PageScaffold>
  );
}

function StyleModal({
  style,
  onClose,
  onSaved,
  onDeleted,
}: {
  style: LibraryStyleView | null;
  onClose: () => void;
  onSaved: (id: number) => void;
  onDeleted: () => void;
}) {
  const [name, setName] = useState(style?.name ?? "");
  const [product, setProduct] = useState(style?.product ?? "");
  const [note, setNote] = useState(style?.note ?? "");

  async function save() {
    try {
      onSaved(await unwrap(commands.saveLibraryStyle(style?.id ?? null, name, product, note)));
    } catch (e) {
      toast.error(message(e));
    }
  }

  async function remove() {
    if (!style) return;
    try {
      await unwrap(commands.deleteLibraryStyle(style.id));
      onDeleted();
    } catch (e) {
      toast.error(message(e));
    }
  }

  return (
    <Modal
      title={style ? "修改款式" : "新建款式"}
      onClose={onClose}
      footer={
        <>
          {style && style.total === 0 && (
            <button type="button" className="btn sm gho dng" onClick={remove}>
              删除款式
            </button>
          )}
          <div className="f1" />
          <button type="button" className="btn sm gho" onClick={onClose}>
            取消
          </button>
          <button type="button" className="btn sm pri" disabled={!name.trim()} onClick={save}>
            保存
          </button>
        </>
      }
    >
      <div className="rlform">
        <span className="fs12 t2">款式名</span>
        <input
          className="inp"
          value={name}
          placeholder="与工单 organization.style 一致，如 王一博"
          onChange={(e) => setName(e.target.value)}
        />
        <span className="fs12 t2">产品</span>
        <input
          className="inp"
          value={product}
          placeholder="可空；同名款式属于不同产品时用来区分"
          onChange={(e) => setProduct(e.target.value)}
        />
        <span className="fs12 t2">备注</span>
        <input className="inp" value={note} onChange={(e) => setNote(e.target.value)} />
      </div>
    </Modal>
  );
}

/** 一行可编辑的分类。 */
interface CatDraft {
  id: number | null;
  name: string;
  cooldownDays: number;
  imageCount: number;
}

function CategoriesModal({
  categories,
  onClose,
  onChanged,
}: {
  categories: LibraryCategoryView[];
  onClose: () => void;
  onChanged: () => Promise<void>;
}) {
  const [rows, setRows] = useState<CatDraft[]>(() =>
    categories.map((c) => ({
      id: c.id,
      name: c.name,
      cooldownDays: c.cooldownDays,
      imageCount: c.imageCount,
    })),
  );

  const edit = (i: number, p: Partial<CatDraft>) =>
    setRows((r) => r.map((row, k) => (k === i ? { ...row, ...p } : row)));

  async function save() {
    try {
      for (const r of rows) {
        const old = categories.find((c) => c.id === r.id);
        if (old && old.name === r.name.trim() && old.cooldownDays === r.cooldownDays) continue;
        if (!r.id && !r.name.trim()) continue;
        await unwrap(commands.saveLibraryCategory(r.id, r.name, r.cooldownDays, null));
      }
      await onChanged();
      onClose();
    } catch (e) {
      toast.error(message(e));
    }
  }

  async function remove(row: CatDraft) {
    if (!row.id) {
      setRows((r) => r.filter((x) => x !== row));
      return;
    }
    try {
      await unwrap(commands.deleteLibraryCategory(row.id));
      setRows((r) => r.filter((x) => x !== row));
      await onChanged();
    } catch (e) {
      toast.error(message(e));
    }
  }

  return (
    <Modal
      title="分类与冷却"
      width="w640"
      onClose={onClose}
      footer={
        <>
          <button
            type="button"
            className="btn sm gho"
            onClick={() =>
              setRows((r) => [...r, { id: null, name: "", cooldownDays: 7, imageCount: 0 }])
            }
          >
            <Plus className="ic12" />
            新分类
          </button>
          <div className="f1" />
          <button type="button" className="btn sm gho" onClick={onClose}>
            取消
          </button>
          <button type="button" className="btn sm pri" onClick={save}>
            保存
          </button>
        </>
      }
    >
      <div className="fs12 t3">
        冷却天数：一张图被工单用过后，这么多天内抽图时优先跳过它；可用图不够时仍会退而用最久没用的那张。填
        0 表示这一类不冷却，可以随时重复用。
      </div>
      <div className="rlcats mt10">
        <span className="fs11 fw6 t3">分类</span>
        <span className="fs11 fw6 t3">冷却天数</span>
        <span className="fs11 fw6 t3">图片</span>
        <span />
        {rows.map((r, i) => (
          <CatRow
            key={r.id ?? `new-${i}`}
            row={r}
            onEdit={(p) => edit(i, p)}
            onRemove={() => remove(r)}
          />
        ))}
      </div>
    </Modal>
  );
}

function CatRow({
  row,
  onEdit,
  onRemove,
}: {
  row: CatDraft;
  onEdit: (p: Partial<CatDraft>) => void;
  onRemove: () => void;
}) {
  return (
    <>
      <input
        className="inp sm"
        value={row.name}
        placeholder="如 礼盒图"
        onChange={(e) => onEdit({ name: e.target.value })}
      />
      <input
        className="inp sm rlnum"
        type="number"
        min={0}
        max={365}
        value={row.cooldownDays}
        onChange={(e) =>
          onEdit({
            cooldownDays: Math.max(0, Math.min(365, Math.round(Number(e.target.value) || 0))),
          })
        }
      />
      <span className="fs12 t2 mono">{row.imageCount}</span>
      <button
        type="button"
        className="btn xs gho dng"
        disabled={row.imageCount > 0}
        title={row.imageCount > 0 ? "分类下还有图片，不能删除" : "删除分类"}
        onClick={onRemove}
      >
        删除
      </button>
    </>
  );
}
