import { Modal } from "@/components/ui/Modal";
import { PageScaffold } from "@/features/_shared/PageScaffold";
import { useListSelection } from "@/features/_shared/useListSelection";
import { type GroupView, type ProductSkuView, type ProductView, commands, unwrap } from "@/lib/ipc";
import { FileInput, Plus, RefreshCw, Trash2 } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { toast } from "sonner";

const PLATFORM_LABELS: Record<string, string> = {
  douyin: "抖音",
  xhs: "小红书",
  kuaishou: "快手",
  shipinhao: "视频号",
};

function message(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

type ProductDraft = {
  code: string;
  name: string;
  platforms: string[];
  cartEnabled: boolean;
  douyinProductUrl: string;
  douyinShortTitle: string;
  status: string;
  note: string;
};

const EMPTY_PRODUCT: ProductDraft = {
  code: "",
  name: "",
  platforms: ["douyin", "xhs", "kuaishou", "shipinhao"],
  cartEnabled: false,
  douyinProductUrl: "",
  douyinShortTitle: "",
  status: "active",
  note: "",
};

export function ProductsPage() {
  const [products, setProducts] = useState<ProductView[]>([]);
  const [allSkus, setAllSkus] = useState<ProductSkuView[]>([]);
  const [groups, setGroups] = useState<GroupView[]>([]);
  const [activeId, setActiveId] = useState<number | null>(null);
  const [draft, setDraft] = useState<ProductDraft | null>(null);
  const [editingId, setEditingId] = useState<number | null>(null);
  const [skuModal, setSkuModal] = useState(false);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    try {
      const [nextProducts, nextSkus, nextGroups] = await Promise.all([
        unwrap(commands.listProducts()),
        unwrap(commands.listProductSkus()),
        unwrap(commands.listPromptGroups()),
      ]);
      setProducts(nextProducts);
      setAllSkus(nextSkus);
      setGroups(nextGroups);
      setActiveId((id) =>
        id !== null && nextProducts.some((p) => p.id === id) ? id : (nextProducts[0]?.id ?? null),
      );
    } catch (error) {
      toast.error(`商品资料加载失败：${message(error)}`);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const active = products.find((p) => p.id === activeId) ?? null;
  const productSelection = useListSelection(
    products.map((p) => p.id),
    { onOpen: setActiveId },
  );
  const skuSelection = useListSelection(active?.skus.map((s) => s.id) ?? []);

  const editProduct = (product?: ProductView) => {
    setEditingId(product?.id ?? null);
    setDraft(
      product
        ? {
            code: product.code,
            name: product.name,
            platforms: product.platforms,
            cartEnabled: product.cartEnabled,
            douyinProductUrl: product.douyinProductUrl,
            douyinShortTitle: product.douyinShortTitle,
            status: product.status,
            note: product.note,
          }
        : { ...EMPTY_PRODUCT },
    );
  };

  const saveProduct = async () => {
    if (!draft || !draft.code.trim() || !draft.name.trim()) return;
    setBusy(true);
    try {
      if (editingId === null) {
        await unwrap(
          commands.createProduct({
            code: draft.code.trim(),
            name: draft.name.trim(),
            platforms: draft.platforms,
            cartEnabled: draft.cartEnabled,
            douyinProductUrl: draft.douyinProductUrl.trim(),
            douyinShortTitle: draft.douyinShortTitle.trim(),
            note: draft.note.trim(),
          }),
        );
      } else {
        await unwrap(
          commands.updateProduct(editingId, {
            name: draft.name.trim(),
            platforms: draft.platforms,
            cartEnabled: draft.cartEnabled,
            douyinProductUrl: draft.douyinProductUrl.trim(),
            douyinShortTitle: draft.douyinShortTitle.trim(),
            status: draft.status,
            note: draft.note.trim(),
          }),
        );
      }
      setDraft(null);
      await load();
      toast.success("商品资料已保存");
    } catch (error) {
      toast.error(`保存失败：${message(error)}`);
    } finally {
      setBusy(false);
    }
  };

  const deleteActive = async () => {
    if (!active || !window.confirm(`删除商品「${active.name}」？已有任务单的商品无法删除。`))
      return;
    try {
      await unwrap(commands.deleteProduct(active.id));
      await load();
      toast.success("商品已删除");
    } catch (error) {
      toast.error(`删除失败：${message(error)}`);
    }
  };

  const importCatalog = async () => {
    try {
      const path = await unwrap(commands.pickProductCatalogFile());
      if (!path) return;
      const report = await unwrap(commands.importProductCatalog(path));
      await load();
      toast.success(`建档完成：${report.rows} 行`, {
        description: `新建商品 ${report.productsCreated} · 新建 SKU ${report.skusCreated} · 更新 SKU ${report.skusUpdated}`,
      });
    } catch (error) {
      toast.error(`批量建档失败：${message(error)}`);
    }
  };

  return (
    <PageScaffold
      title="商品资料"
      caption="商品定义文案与平台，SKU 定义图片与款式"
      right={
        <div className="row gap6">
          <button type="button" className="btn sm" onClick={() => void load()}>
            <RefreshCw className="ic12" /> 刷新
          </button>
          <button
            type="button"
            className="btn sm"
            title="UTF-8 CSV/TSV 列：所属商品编码、所属商品名称、SKU编码、SKU名称、层级、文件夹别名、音乐关键词"
            onClick={() => void importCatalog()}
          >
            <FileInput className="ic12" /> 批量建档
          </button>
          <button type="button" className="btn sm pri" onClick={() => editProduct()}>
            <Plus className="ic12" /> 新建商品
          </button>
        </div>
      }
    >
      <div className="pub-split">
        <aside className="pub-rail" {...productSelection.containerProps} aria-label="商品列表">
          <div className="pub-rail-head">{products.length} 个商品</div>
          {products.map((product) => (
            <button
              key={product.id}
              type="button"
              className={`pub-list-row ${activeId === product.id ? "on" : ""}`}
              onClick={(event) => {
                productSelection.select(product.id, event);
                setActiveId(product.id);
              }}
              onDoubleClick={() => editProduct(product)}
            >
              <span className="pub-code">{product.code}</span>
              <span className="f1 nowrap ohide">{product.name}</span>
              <span className={product.status === "active" ? "pub-dot ok" : "pub-dot"} />
            </button>
          ))}
          {products.length === 0 && <div className="pub-empty-sm">先创建一个商品</div>}
        </aside>

        <section className="pub-work">
          {active ? (
            <>
              <header className="pub-work-head">
                <div>
                  <div className="row gap8">
                    <span className="pub-code lg">{active.code}</span>
                    <h2>{active.name}</h2>
                  </div>
                  <div className="pub-meta">
                    {active.platforms.map((p) => PLATFORM_LABELS[p] ?? p).join(" · ") ||
                      "未启用平台"}
                    {active.cartEnabled ? " · 抖音挂车" : " · 不挂车"}
                  </div>
                </div>
                <div className="row gap6">
                  <button type="button" className="btn sm" onClick={() => editProduct(active)}>
                    编辑
                  </button>
                  <button type="button" className="btn sm dng" onClick={() => void deleteActive()}>
                    <Trash2 className="ic12" /> 删除
                  </button>
                </div>
              </header>

              <div className="pub-stat-grid">
                <Stat label="可用标题" value={active.titleFree} />
                <Stat label="可用正文" value={active.bodyFree} />
                <Stat label="可用图片" value={active.imageFree} />
                <Stat label="SKU" value={active.skus.length} />
              </div>

              <div className="pub-section-head">
                <div>
                  <b>SKU 与素材入口</b>
                  <span>图片归 SKU，发布文案归商品</span>
                </div>
                <button type="button" className="btn xs" onClick={() => setSkuModal(true)}>
                  管理 SKU
                </button>
              </div>
              <div className="pub-table" {...skuSelection.containerProps}>
                <div className="pub-tr pub-th">
                  <span>SKU</span>
                  <span>款式</span>
                  <span>层级</span>
                  <span>可用图片</span>
                  <span>音乐关键词</span>
                </div>
                {active.skus.map((sku) => (
                  <button
                    type="button"
                    key={sku.id}
                    className={`pub-tr ${skuSelection.isSelected(sku.id) ? "sel" : ""}`}
                    onClick={(e) => skuSelection.select(sku.id, e)}
                  >
                    <span className="pub-code">{sku.code}</span>
                    <span>{sku.name}</span>
                    <span>{tierName(sku.tier)}</span>
                    <span>{sku.freeImages}</span>
                    <span className="nowrap ohide t3">{sku.musicKeyword || "未设置"}</span>
                  </button>
                ))}
              </div>

              <PromptSkuBindings groups={groups} skus={active.skus} onSaved={load} />
            </>
          ) : (
            <div className="bigempty">
              <div className="fw6">从左侧选择商品</div>
              <div className="t3 mt6">商品是图文发布的根实体。</div>
            </div>
          )}
        </section>
      </div>

      {draft && (
        <ProductModal
          draft={draft}
          setDraft={setDraft}
          editing={editingId !== null}
          busy={busy}
          onSave={() => void saveProduct()}
          onClose={() => setDraft(null)}
        />
      )}
      {skuModal && active && (
        <SkuModal
          product={active}
          allSkus={allSkus}
          onClose={() => setSkuModal(false)}
          onSaved={async () => {
            await load();
          }}
        />
      )}
    </PageScaffold>
  );
}

function Stat({ label, value }: { label: string; value: number }) {
  return (
    <div className="pub-stat">
      <span>{label}</span>
      <b>{value}</b>
    </div>
  );
}

function tierName(tier: string) {
  return ({ hot: "热款", warm: "温款", cold: "冷款" } as Record<string, string>)[tier] ?? tier;
}

function ProductModal({
  draft,
  setDraft,
  editing,
  busy,
  onSave,
  onClose,
}: {
  draft: ProductDraft;
  setDraft: (draft: ProductDraft) => void;
  editing: boolean;
  busy: boolean;
  onSave: () => void;
  onClose: () => void;
}) {
  const shortInvalid = draft.douyinShortTitle.length > 10;
  return (
    <Modal
      title={editing ? "编辑商品" : "新建商品"}
      width="w640"
      onClose={onClose}
      footer={
        <>
          <div className="f1" />
          <button type="button" className="btn sm" onClick={onClose}>
            取消
          </button>
          <button
            type="button"
            className="btn sm pri"
            disabled={busy || shortInvalid || !draft.code.trim() || !draft.name.trim()}
            onClick={onSave}
          >
            {busy ? "保存中…" : "保存"}
          </button>
        </>
      }
    >
      <div className="pub-form-grid">
        <label>
          <span>商品编码</span>
          <input
            className="inp"
            value={draft.code}
            disabled={editing}
            onChange={(e) => setDraft({ ...draft, code: e.target.value })}
            placeholder="product-001"
          />
        </label>
        <label>
          <span>商品名称</span>
          <input
            className="inp"
            value={draft.name}
            onChange={(e) => setDraft({ ...draft, name: e.target.value })}
          />
        </label>
        <fieldset className="pub-fieldset">
          <legend>发布平台</legend>
          {Object.entries(PLATFORM_LABELS).map(([code, label]) => (
            <label key={code} className="pub-check">
              <input
                type="checkbox"
                checked={draft.platforms.includes(code)}
                onChange={(e) =>
                  setDraft({
                    ...draft,
                    platforms: e.target.checked
                      ? [...draft.platforms, code]
                      : draft.platforms.filter((p) => p !== code),
                  })
                }
              />
              {label}
            </label>
          ))}
        </fieldset>
        <label className="pub-check align-end">
          <input
            type="checkbox"
            checked={draft.cartEnabled}
            onChange={(e) => setDraft({ ...draft, cartEnabled: e.target.checked })}
          />
          抖音挂车
        </label>
        <label>
          <span>抖音商品链接</span>
          <input
            className="inp"
            disabled={!draft.cartEnabled}
            value={draft.douyinProductUrl}
            onChange={(e) => setDraft({ ...draft, douyinProductUrl: e.target.value })}
            placeholder="未挂车时保留但禁用"
          />
        </label>
        <label>
          <span>
            抖音短标题{" "}
            <i className={shortInvalid ? "er" : "t3"}>{draft.douyinShortTitle.length}/10</i>
          </span>
          <input
            className={`inp ${shortInvalid ? "invalid" : ""}`}
            disabled={!draft.cartEnabled}
            value={draft.douyinShortTitle}
            onChange={(e) => setDraft({ ...draft, douyinShortTitle: e.target.value })}
          />
        </label>
        {editing && (
          <label>
            <span>状态</span>
            <select
              className="inp"
              value={draft.status}
              onChange={(e) => setDraft({ ...draft, status: e.target.value })}
            >
              <option value="active">启用</option>
              <option value="paused">暂停</option>
            </select>
          </label>
        )}
        <label className="span2">
          <span>备注</span>
          <textarea
            className="inp pub-textarea"
            value={draft.note}
            onChange={(e) => setDraft({ ...draft, note: e.target.value })}
          />
        </label>
      </div>
    </Modal>
  );
}

function SkuModal({
  product,
  allSkus,
  onClose,
  onSaved,
}: {
  product: ProductView;
  allSkus: ProductSkuView[];
  onClose: () => void;
  onSaved: () => Promise<void>;
}) {
  const unassigned = useMemo(() => allSkus.filter((sku) => sku.productId === null), [allSkus]);
  const [checked, setChecked] = useState<number[]>([]);
  const [form, setForm] = useState({
    code: "",
    name: "",
    tier: "warm",
    folderAlias: "",
    musicKeyword: "",
    note: "",
  });
  const save = async () => {
    try {
      if (checked.length) await unwrap(commands.assignSkusToProduct(product.id, checked));
      if (form.code.trim() && form.name.trim())
        await unwrap(
          commands.createProductSku({
            productId: product.id,
            code: form.code.trim(),
            name: form.name.trim(),
            tier: form.tier,
            folderAlias: form.folderAlias.trim(),
            musicKeyword: form.musicKeyword.trim(),
            note: form.note.trim(),
          }),
        );
      await onSaved();
      onClose();
      toast.success("SKU 已更新");
    } catch (error) {
      toast.error(`SKU 保存失败：${message(error)}`);
    }
  };
  return (
    <Modal
      title={`管理 SKU · ${product.name}`}
      width="w700"
      onClose={onClose}
      footer={
        <>
          <div className="f1" />
          <button type="button" className="btn sm" onClick={onClose}>
            取消
          </button>
          <button type="button" className="btn sm pri" onClick={() => void save()}>
            保存
          </button>
        </>
      }
    >
      <div className="pub-modal-section">
        <b>挂靠已有 SKU</b>
        <div className="pub-check-grid">
          {unassigned.map((sku) => (
            <label key={sku.id} className="pub-check">
              <input
                type="checkbox"
                checked={checked.includes(sku.id)}
                onChange={(e) =>
                  setChecked(
                    e.target.checked ? [...checked, sku.id] : checked.filter((id) => id !== sku.id),
                  )
                }
              />
              <span className="pub-code">{sku.code}</span>
              {sku.name}
            </label>
          ))}
          {unassigned.length === 0 && <span className="t3">没有未挂靠 SKU</span>}
        </div>
      </div>
      <div className="pub-modal-section">
        <b>或新建 SKU</b>
        <div className="pub-form-grid compact">
          <label>
            <span>SKU 编码</span>
            <input
              className="inp"
              value={form.code}
              onChange={(e) => setForm({ ...form, code: e.target.value })}
            />
          </label>
          <label>
            <span>款式名</span>
            <input
              className="inp"
              value={form.name}
              onChange={(e) => setForm({ ...form, name: e.target.value })}
            />
          </label>
          <label>
            <span>层级</span>
            <select
              className="inp"
              value={form.tier}
              onChange={(e) => setForm({ ...form, tier: e.target.value })}
            >
              <option value="hot">热款</option>
              <option value="warm">温款</option>
              <option value="cold">冷款</option>
            </select>
          </label>
          <label>
            <span>文件夹别名</span>
            <input
              className="inp"
              value={form.folderAlias}
              onChange={(e) => setForm({ ...form, folderAlias: e.target.value })}
            />
          </label>
          <label className="span2">
            <span>音乐关键词</span>
            <input
              className="inp"
              value={form.musicKeyword}
              onChange={(e) => setForm({ ...form, musicKeyword: e.target.value })}
            />
          </label>
        </div>
      </div>
    </Modal>
  );
}

function PromptSkuBindings({
  groups,
  skus,
  onSaved,
}: { groups: GroupView[]; skus: ProductSkuView[]; onSaved: () => Promise<void> }) {
  const relevant = groups.filter(
    (group) => group.skuId === null || skus.some((sku) => sku.id === group.skuId),
  );
  if (relevant.length === 0) return null;
  return (
    <>
      <div className="pub-section-head">
        <div>
          <b>生图分组绑定</b>
          <span>验收通过后自动进入绑定 SKU 的图片库</span>
        </div>
      </div>
      <div className="pub-bindings">
        {relevant.map((group) => (
          <label key={group.id}>
            <span>
              <b>{group.name}</b>
              <small>
                {group.prefix} · {group.count} 条
              </small>
            </span>
            <select
              className="inp"
              value={group.skuId ?? ""}
              onChange={async (e) => {
                try {
                  await unwrap(
                    commands.setPromptGroupSku(
                      group.id,
                      e.target.value ? Number(e.target.value) : null,
                    ),
                  );
                  await onSaved();
                  toast.success("分组绑定已更新");
                } catch (error) {
                  toast.error(message(error));
                }
              }}
            >
              <option value="">不进入发布素材库</option>
              {skus.map((sku) => (
                <option key={sku.id} value={sku.id}>
                  {sku.code} · {sku.name}
                </option>
              ))}
            </select>
          </label>
        ))}
      </div>
    </>
  );
}
