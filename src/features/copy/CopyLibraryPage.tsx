import { Modal } from "@/components/ui/Modal";
import { PageScaffold } from "@/features/_shared/PageScaffold";
import { useListSelection } from "@/features/_shared/useListSelection";
import {
  type CopyItemView,
  type ProductView,
  type TopicGroupView,
  commands,
  unwrap,
} from "@/lib/ipc";
import { FileInput, Plus, Trash2 } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { toast } from "sonner";

type Tab = "title" | "body" | "topics";

function message(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

export function CopyLibraryPage() {
  const [products, setProducts] = useState<ProductView[]>([]);
  const [productId, setProductId] = useState<number | null>(null);
  const [tab, setTab] = useState<Tab>("title");
  const [items, setItems] = useState<CopyItemView[]>([]);
  const [topics, setTopics] = useState<TopicGroupView[]>([]);
  const [edit, setEdit] = useState<{ id: number | null; text: string } | null>(null);
  const [topicEdit, setTopicEdit] = useState<TopicGroupView | null | "new">(null);

  const load = useCallback(async () => {
    try {
      const nextProducts = await unwrap(commands.listProducts());
      setProducts(nextProducts);
      setProductId((id) => id ?? nextProducts[0]?.id ?? null);
      if (tab === "topics") setTopics(await unwrap(commands.listTopicGroups(productId)));
      else setItems(await unwrap(commands.listCopyItems(productId, tab)));
    } catch (error) {
      toast.error(`文案库加载失败：${message(error)}`);
    }
  }, [productId, tab]);

  useEffect(() => {
    void load();
  }, [load]);

  const selection = useListSelection(
    (tab === "topics" ? topics : items).map((item) => item.id),
    {
      onOpen: (id) => {
        if (tab === "topics") setTopicEdit(topics.find((item) => item.id === id) ?? null);
        else setEdit({ id, text: items.find((item) => item.id === id)?.text ?? "" });
      },
      onDelete: (ids) => void remove(ids),
    },
  );

  async function remove(ids = [...selection.selected]) {
    if (
      !ids.length ||
      !window.confirm(
        `删除选中的 ${ids.length} 条${tab === "topics" ? "话题组" : "文案"}？已进入任务单的内容会保留快照。`,
      )
    )
      return;
    try {
      const n =
        tab === "topics"
          ? await unwrap(commands.deleteTopicGroups(ids))
          : await unwrap(commands.deleteCopyItems(ids));
      selection.clear();
      await load();
      toast.success(`已删除 ${n} 条`);
    } catch (error) {
      toast.error(`删除失败：${message(error)}`);
    }
  }

  async function setEnabled(enabled: boolean) {
    try {
      if (tab === "topics")
        await unwrap(commands.setTopicGroupsEnabled([...selection.selected], enabled));
      else await unwrap(commands.setCopyItemsEnabled([...selection.selected], enabled));
      await load();
      selection.clear();
    } catch (error) {
      toast.error(message(error));
    }
  }

  async function saveCopy() {
    if (!edit || productId === null || !edit.text.trim()) return;
    try {
      if (edit.id === null) await unwrap(commands.addCopyItem(productId, tab, edit.text.trim()));
      else await unwrap(commands.updateCopyItem(edit.id, edit.text.trim()));
      setEdit(null);
      await load();
      toast.success("文案已保存");
    } catch (error) {
      toast.error(`保存失败：${message(error)}`);
    }
  }

  async function importFile() {
    if (productId === null) return;
    try {
      const path = await unwrap(commands.pickCopyFile());
      if (!path) return;
      const preview = await unwrap(commands.previewCopyFile(path));
      if (
        !window.confirm(
          `识别到标题 ${preview.titles} 条、正文 ${preview.bodies} 条、话题 ${preview.topics.length} 个。确认导入到当前商品？`,
        )
      )
        return;
      const count = await unwrap(commands.importCopyFile(productId, path));
      await load();
      toast.success(`已导入 ${count} 条内容`);
    } catch (error) {
      toast.error(`导入失败：${message(error)}`);
    }
  }

  const currentProduct = products.find((p) => p.id === productId);
  return (
    <PageScaffold
      title="文案库"
      caption="标题与正文按商品共享；话题组可限定商品或 SKU"
      right={
        <div className="row gap6">
          <button
            type="button"
            className="btn sm"
            disabled={productId === null}
            onClick={() => void importFile()}
          >
            <FileInput className="ic12" />
            导入 TXT
          </button>
          <button
            type="button"
            className="btn sm pri"
            disabled={productId === null}
            onClick={() =>
              tab === "topics" ? setTopicEdit("new") : setEdit({ id: null, text: "" })
            }
          >
            <Plus className="ic12" />
            新增{tab === "topics" ? "话题组" : "文案"}
          </button>
        </div>
      }
    >
      <div className="pub-toolbar">
        <select
          className="inp"
          value={productId ?? ""}
          onChange={(e) => {
            setProductId(e.target.value ? Number(e.target.value) : null);
            selection.clear();
          }}
        >
          <option value="">选择商品</option>
          {products.map((p) => (
            <option key={p.id} value={p.id}>
              {p.code} · {p.name}
            </option>
          ))}
        </select>
        <div className="pub-tabs">
          {(["title", "body", "topics"] as Tab[]).map((key) => (
            <button
              type="button"
              key={key}
              className={tab === key ? "on" : ""}
              onClick={() => {
                setTab(key);
                selection.clear();
              }}
            >
              {key === "title" ? "标题" : key === "body" ? "正文" : "话题组"}
            </button>
          ))}
        </div>
        <span className="cnt">{tab === "topics" ? topics.length : items.length} 条</span>
        <div className="f1" />
        {selection.count > 0 && (
          <>
            <span className="pub-selected">已选 {selection.count}</span>
            <button type="button" className="btn sm" onClick={() => void setEnabled(true)}>
              启用
            </button>
            <button type="button" className="btn sm" onClick={() => void setEnabled(false)}>
              停用
            </button>
            <button type="button" className="btn sm dng" onClick={() => void remove()}>
              <Trash2 className="ic12" />
              删除
            </button>
          </>
        )}
      </div>
      <div className="pub-copy-wrap" {...selection.containerProps}>
        {tab !== "topics"
          ? items.map((item) => (
              <button
                type="button"
                key={item.id}
                className={`pub-copy-row ${selection.isSelected(item.id) ? "sel" : ""} ${!item.enabled ? "muted" : ""}`}
                onClick={(e) => selection.select(item.id, e)}
                onDoubleClick={() => setEdit({ id: item.id, text: item.text })}
              >
                <span className="pub-code">{item.id}</span>
                <span className="pub-copy-text">{item.text}</span>
                <span className={`bdg ${item.state === "free" ? "b-green" : "b-gray"}`}>
                  {item.state === "free" ? "空闲" : item.state === "held" ? "占用" : "已使用"}
                </span>
                <span className="t3">{item.source === "manual" ? "手工" : "导入"}</span>
              </button>
            ))
          : topics.map((group) => (
              <button
                type="button"
                key={group.id}
                className={`pub-copy-row ${selection.isSelected(group.id) ? "sel" : ""} ${!group.enabled ? "muted" : ""}`}
                onClick={(e) => selection.select(group.id, e)}
                onDoubleClick={() => setTopicEdit(group)}
              >
                <span className="bdg b-blue">
                  {group.scope === "general" ? "通用" : group.scope === "product" ? "商品" : "组合"}
                </span>
                <span className="pub-copy-text pub-topic-tags">
                  {group.tags.map((tag) => (
                    <i key={tag}>{tag.startsWith("#") ? tag : `#${tag}`}</i>
                  ))}
                </span>
                <span className="t3">
                  {group.productName ?? "所有商品"}
                  {group.skuIds.length ? ` · ${group.skuIds.length} 个 SKU` : ""}
                </span>
              </button>
            ))}
        {(tab === "topics" ? topics : items).length === 0 && (
          <div className="bigempty">
            <div className="fw6">
              {currentProduct
                ? `${currentProduct.name} 还没有${tab === "title" ? "标题" : tab === "body" ? "正文" : "话题组"}`
                : "先选择商品"}
            </div>
            <div className="t3 mt6">可逐条新增，也可按收件箱格式导入 TXT。</div>
          </div>
        )}
      </div>
      {edit && (
        <Modal
          title={edit.id === null ? `新增${tab === "title" ? "标题" : "正文"}` : "编辑文案"}
          width="w640"
          onClose={() => setEdit(null)}
          footer={
            <>
              <div className="f1" />
              <button type="button" className="btn sm" onClick={() => setEdit(null)}>
                取消
              </button>
              <button
                type="button"
                className="btn sm pri"
                disabled={!edit.text.trim()}
                onClick={() => void saveCopy()}
              >
                保存
              </button>
            </>
          }
        >
          <textarea
            className="ta"
            value={edit.text}
            onChange={(e) => setEdit({ ...edit, text: e.target.value })}
          />
          <div className="pub-counter">{edit.text.length} 字</div>
        </Modal>
      )}
      {topicEdit && productId !== null && (
        <TopicModal
          value={topicEdit === "new" ? null : topicEdit}
          product={currentProduct ?? null}
          onClose={() => setTopicEdit(null)}
          onSaved={load}
        />
      )}
    </PageScaffold>
  );
}

function TopicModal({
  value,
  product,
  onClose,
  onSaved,
}: {
  value: TopicGroupView | null;
  product: ProductView | null;
  onClose: () => void;
  onSaved: () => Promise<void>;
}) {
  const [scope, setScope] = useState(value?.scope ?? "product");
  const [skuIds, setSkuIds] = useState<number[]>(value?.skuIds ?? []);
  const [raw, setRaw] = useState(value?.tags.join(" ") ?? "");
  const save = async () => {
    if (!product) return;
    const tags = raw
      .split(/[\s,，#]+/)
      .map((tag) => tag.trim())
      .filter(Boolean);
    try {
      await unwrap(
        commands.saveTopicGroup(value?.id ?? null, {
          productId: scope === "general" ? null : product.id,
          scope,
          skuIds: scope === "combo" ? skuIds : [],
          tags,
        }),
      );
      await onSaved();
      onClose();
      toast.success("话题组已保存");
    } catch (error) {
      toast.error(`保存失败：${message(error)}`);
    }
  };
  return (
    <Modal
      title={value ? "编辑话题组" : "新增话题组"}
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
            disabled={!raw.trim() || (scope === "combo" && skuIds.length < 2)}
            onClick={() => void save()}
          >
            保存
          </button>
        </>
      }
    >
      <div className="pub-form-grid">
        <label>
          <span>作用范围</span>
          <select className="inp" value={scope} onChange={(e) => setScope(e.target.value)}>
            <option value="general">全局通用</option>
            <option value="product">当前商品</option>
            <option value="combo">SKU 组合</option>
          </select>
        </label>
        <label className="span2">
          <span>话题，用空格或逗号分隔</span>
          <textarea
            className="ta"
            value={raw}
            onChange={(e) => setRaw(e.target.value)}
            placeholder="#生活好物 #开箱"
          />
        </label>
        {scope === "combo" && (
          <fieldset className="pub-fieldset span2">
            <legend>选择 SKU</legend>
            {product?.skus.map((sku) => (
              <label key={sku.id} className="pub-check">
                <input
                  type="checkbox"
                  checked={skuIds.includes(sku.id)}
                  onChange={(e) =>
                    setSkuIds(
                      e.target.checked ? [...skuIds, sku.id] : skuIds.filter((id) => id !== sku.id),
                    )
                  }
                />
                {sku.code} · {sku.name}
              </label>
            ))}
          </fieldset>
        )}
      </div>
    </Modal>
  );
}
