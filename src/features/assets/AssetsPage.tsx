import { NatThumb } from "@/features/_shared/NatThumb";
import { PageScaffold } from "@/features/_shared/PageScaffold";
import { useListSelection } from "@/features/_shared/useListSelection";
import {
  type ImageAssetView,
  type ProductSkuView,
  type ProductView,
  commands,
  subscribeFileDrop,
  unwrap,
} from "@/lib/ipc";
import { FolderInput, RefreshCw, Trash2 } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { toast } from "sonner";

function message(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

export function AssetsPage() {
  const [products, setProducts] = useState<ProductView[]>([]);
  const [skus, setSkus] = useState<ProductSkuView[]>([]);
  const [assets, setAssets] = useState<ImageAssetView[]>([]);
  const [productId, setProductId] = useState<number | null>(null);
  const [skuId, setSkuId] = useState<number | null>(null);
  const [assetState, setAssetState] = useState("free");
  const [busy, setBusy] = useState(false);
  const [unmatchedImport, setUnmatchedImport] = useState<{
    folder: string;
    names: string[];
  } | null>(null);
  const [fallbackSku, setFallbackSku] = useState<number | null>(null);
  const dropImport = useRef<(folder: string) => void>(() => undefined);

  const loadMeta = useCallback(async () => {
    const [p, s] = await Promise.all([
      unwrap(commands.listProducts()),
      unwrap(commands.listProductSkus()),
    ]);
    setProducts(p);
    setSkus(s);
  }, []);

  const loadAssets = useCallback(async () => {
    try {
      setAssets(await unwrap(commands.listImageAssets(productId, skuId, assetState || null)));
    } catch (error) {
      setAssets([]);
      toast.error(`图片库加载失败：${message(error)}`);
    }
  }, [assetState, productId, skuId]);

  useEffect(() => {
    void loadMeta().catch((error) => toast.error(message(error)));
  }, [loadMeta]);
  useEffect(() => {
    void loadAssets();
  }, [loadAssets]);

  const selection = useListSelection(
    assets.map((asset) => asset.id),
    { onDelete: (ids) => void remove(ids) },
  );
  const visibleSkus = useMemo(
    () => skus.filter((sku) => productId === null || sku.productId === productId),
    [productId, skus],
  );
  const grouped = useMemo(() => {
    const map = new Map<string, ImageAssetView[]>();
    for (const asset of assets) {
      const key = `${asset.skuCode} · ${asset.skuName}`;
      map.set(key, [...(map.get(key) ?? []), asset]);
    }
    return [...map.entries()];
  }, [assets]);

  async function importFolder(folder?: string, fallback = skuId) {
    setBusy(true);
    try {
      const path = folder ?? (await unwrap(commands.pickImageFolder()));
      if (!path) return;
      const report = await unwrap(commands.importImageFolder(path, fallback));
      await loadAssets();
      selection.clear();
      if (report.unmatched.length) {
        setUnmatchedImport({ folder: path, names: report.unmatched });
        toast.warning(`已导入 ${report.imported} 张，${report.unmatched.length} 个文件夹未匹配`, {
          description: report.unmatched.join("、"),
        });
      } else toast.success(`已导入 ${report.imported} 张图片`);
    } catch (error) {
      toast.error(`导入失败：${message(error)}`);
    } finally {
      setBusy(false);
    }
  }

  // 文件拖放监听只注册一次；ref 始终指向带有最新筛选与选择状态的导入动作。
  dropImport.current = (folder) => void importFolder(folder);
  useEffect(() => {
    let un: (() => void) | undefined;
    void subscribeFileDrop((paths) => {
      const folder = paths[0];
      if (folder) dropImport.current(folder);
    }).then((cleanup) => {
      un = cleanup;
    });
    return () => un?.();
  }, []);

  async function remove(ids = [...selection.selected]) {
    if (
      !ids.length ||
      !window.confirm(`把选中的 ${ids.length} 张空闲图片移入废纸篓？可在清理前还原。`)
    )
      return;
    try {
      const count = await unwrap(commands.deleteImageAssets(ids));
      await loadAssets();
      selection.clear();
      toast.success(`已将 ${count} 张图片移入废纸篓`);
    } catch (error) {
      toast.error(`删除失败：${message(error)}`);
    }
  }

  async function move(sku: number) {
    try {
      const count = await unwrap(commands.setImageAssetsSku([...selection.selected], sku));
      await loadAssets();
      selection.clear();
      toast.success(`已调整 ${count} 张图片的 SKU`);
    } catch (error) {
      toast.error(`调整失败：${message(error)}`);
    }
  }

  return (
    <PageScaffold
      title="图片素材库"
      caption="按 SKU 管理原图，空闲素材才可删除或改挂靠"
      right={
        <div className="row gap6">
          <button type="button" className="btn sm" onClick={() => void loadAssets()}>
            <RefreshCw className="ic12" />
            刷新
          </button>
          <button
            type="button"
            className="btn sm pri"
            disabled={busy}
            onClick={() => void importFolder()}
          >
            <FolderInput className="ic12" />
            {busy ? "导入中…" : "导入文件夹"}
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
            setSkuId(null);
            selection.clear();
          }}
        >
          <option value="">全部商品</option>
          {products.map((p) => (
            <option key={p.id} value={p.id}>
              {p.code} · {p.name}
            </option>
          ))}
        </select>
        <select
          className="inp"
          value={skuId ?? ""}
          onChange={(e) => {
            setSkuId(e.target.value ? Number(e.target.value) : null);
            selection.clear();
          }}
        >
          <option value="">全部 SKU</option>
          {visibleSkus.map((sku) => (
            <option key={sku.id} value={sku.id}>
              {sku.code} · {sku.name}
            </option>
          ))}
        </select>
        <select
          className="inp"
          value={assetState}
          onChange={(e) => {
            setAssetState(e.target.value);
            selection.clear();
          }}
        >
          <option value="free">空闲</option>
          <option value="held">已占用</option>
          <option value="used">已使用</option>
          <option value="">全部状态</option>
        </select>
        <span className="cnt">{assets.length} 张</span>
        <div className="f1" />
        {selection.count > 0 && (
          <>
            <span className="pub-selected">已选 {selection.count}</span>
            <select
              className="inp"
              defaultValue=""
              onChange={(e) => {
                if (e.target.value) void move(Number(e.target.value));
                e.target.value = "";
              }}
            >
              <option value="">更改 SKU…</option>
              {skus.map((sku) => (
                <option key={sku.id} value={sku.id}>
                  {sku.code} · {sku.name}
                </option>
              ))}
            </select>
            <button type="button" className="btn sm dng" onClick={() => void remove()}>
              <Trash2 className="ic12" />
              删除
            </button>
          </>
        )}
      </div>
      <div className="pub-asset-scroll" {...selection.containerProps}>
        {grouped.map(([group, items]) => (
          <section key={group} className="pub-asset-group">
            <header>
              <b>{group}</b>
              <span>{items.length} 张</span>
            </header>
            <div className="pub-image-grid">
              {items.map((asset) => (
                <button
                  type="button"
                  key={asset.id}
                  className={`pub-image-card ${selection.isSelected(asset.id) ? "sel" : ""}`}
                  onClick={(e) => selection.select(asset.id, e)}
                  title={asset.path}
                >
                  <NatThumb path={asset.thumb || asset.path} className="pub-thumb" />
                  <span className="pub-image-foot">
                    <i
                      className={`pub-dot ${asset.state === "free" ? "ok" : asset.state === "held" ? "warn" : ""}`}
                    />
                    {asset.state === "free" ? "空闲" : asset.state === "held" ? "占用" : "已使用"}
                    <em>{asset.source === "works" ? "验收" : "导入"}</em>
                  </span>
                </button>
              ))}
            </div>
          </section>
        ))}
        {assets.length === 0 && (
          <div className="bigempty">
            <FolderInput />
            <div className="fw6 mt6">把 SKU 文件夹拖进来</div>
            <div className="t3 mt6">
              文件夹名匹配 SKU 编码或别名；筛选具体 SKU 后也可作为兜底指认。
            </div>
          </div>
        )}
      </div>
      {unmatchedImport && (
        <Modal
          title="指认未匹配图片的 SKU"
          onClose={() => setUnmatchedImport(null)}
          footer={
            <>
              <div className="f1" />
              <button type="button" className="btn sm" onClick={() => setUnmatchedImport(null)}>
                稍后处理
              </button>
              <button
                type="button"
                className="btn sm pri"
                disabled={fallbackSku === null}
                onClick={() => {
                  const pending = unmatchedImport;
                  setUnmatchedImport(null);
                  void importFolder(pending.folder, fallbackSku);
                }}
              >
                归入所选 SKU
              </button>
            </>
          }
        >
          <div className="fs12 t2" style={{ lineHeight: 1.7 }}>
            未识别目录：{unmatchedImport.names.join("、")}
          </div>
          <select
            className="inp mt10"
            style={{ width: "100%" }}
            value={fallbackSku ?? ""}
            onChange={(event) =>
              setFallbackSku(event.target.value ? Number(event.target.value) : null)
            }
          >
            <option value="">选择目标 SKU</option>
            {skus.map((sku) => (
              <option key={sku.id} value={sku.id}>
                {sku.code} · {sku.name}
              </option>
            ))}
          </select>
        </Modal>
      )}
    </PageScaffold>
  );
}
import { Modal } from "@/components/ui/Modal";
