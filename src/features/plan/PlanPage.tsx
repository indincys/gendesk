import { Modal } from "@/components/ui/Modal";
import { NatThumb } from "@/features/_shared/NatThumb";
import { PageScaffold } from "@/features/_shared/PageScaffold";
import {
  type CopyItemView,
  type ImageAssetView,
  type PostView,
  type ProductView,
  type SheetConfigView,
  type SheetDetailView,
  type SheetSummaryView,
  commands,
  unwrap,
} from "@/lib/ipc";
import {
  Archive,
  ExternalLink,
  PackageCheck,
  Plus,
  RefreshCw,
  RotateCcw,
  Trash2,
} from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { toast } from "sonner";

const PLATFORM_LABELS: Record<string, string> = {
  douyin: "抖音",
  xhs: "小红书",
  kuaishou: "快手",
  shipinhao: "视频号",
};

function localDate() {
  const date = new Date();
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
}

function message(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

export function PlanPage() {
  const [date, setDate] = useState(localDate());
  const [products, setProducts] = useState<ProductView[]>([]);
  const [configs, setConfigs] = useState<SheetConfigView[]>([]);
  const [sheets, setSheets] = useState<SheetSummaryView[]>([]);
  const [activeId, setActiveId] = useState<number | null>(null);
  const [detail, setDetail] = useState<SheetDetailView | null>(null);
  const [images, setImages] = useState<ImageAssetView[]>([]);
  const [titles, setTitles] = useState<CopyItemView[]>([]);
  const [bodies, setBodies] = useState<CopyItemView[]>([]);
  const [configEdit, setConfigEdit] = useState<SheetConfigView | null | "new">(null);
  const [busy, setBusy] = useState<string | null>(null);

  const loadBase = useCallback(async () => {
    try {
      const [p, c, s] = await Promise.all([
        unwrap(commands.listProducts()),
        unwrap(commands.listSheetConfigs()),
        unwrap(commands.listTaskSheets()),
      ]);
      setProducts(p);
      setConfigs(c);
      setSheets(s);
      setActiveId((id) =>
        id !== null && s.some((sheet) => sheet.id === id) ? id : (s[0]?.id ?? null),
      );
    } catch (error) {
      toast.error(`任务单加载失败：${message(error)}`);
    }
  }, []);

  const loadDetail = useCallback(async (id: number) => {
    try {
      const d = await unwrap(commands.getTaskSheet(id));
      setDetail(d);
      const [a, t, b] = await Promise.all([
        unwrap(commands.listImageAssets(d.summary.productId, null, "free")),
        unwrap(commands.listCopyItems(d.summary.productId, "title")),
        unwrap(commands.listCopyItems(d.summary.productId, "body")),
      ]);
      setImages(a);
      setTitles(t);
      setBodies(b);
    } catch (error) {
      toast.error(`任务单详情加载失败：${message(error)}`);
    }
  }, []);

  useEffect(() => {
    void loadBase();
  }, [loadBase]);
  useEffect(() => {
    if (activeId !== null) void loadDetail(activeId);
    else setDetail(null);
  }, [activeId, loadDetail]);

  async function generate() {
    setBusy("generate");
    try {
      const ids = await unwrap(commands.generateSheets(date));
      await loadBase();
      if (ids[0]) setActiveId(ids[0]);
      toast.success(`已生成 ${ids.length} 份任务单`);
    } catch (error) {
      toast.error(`生成失败：${message(error)}`);
    } finally {
      setBusy(null);
    }
  }

  async function action(label: string, call: () => Promise<unknown>, success: string) {
    if (!detail) return;
    setBusy(label);
    try {
      await call();
      await loadBase();
      await loadDetail(detail.summary.id);
      toast.success(success);
    } catch (error) {
      toast.error(`${success}失败：${message(error)}`);
    } finally {
      setBusy(null);
    }
  }

  async function cancelActive() {
    if (!detail || !window.confirm("取消整份草稿并释放所有图片与文案？")) return;
    setBusy("cancel");
    try {
      await unwrap(commands.cancelSheet(detail.summary.id));
      setDetail(null);
      setActiveId(null);
      await loadBase();
      toast.success("草稿已取消，素材已释放");
    } catch (error) {
      toast.error(`取消失败：${message(error)}`);
    } finally {
      setBusy(null);
    }
  }

  async function closeActive() {
    if (!detail) return;
    setBusy("close");
    try {
      const result = await unwrap(commands.closeTaskSheet(detail.summary.id));
      await loadBase();
      await loadDetail(detail.summary.id);
      if (result.deleteFailures.length) {
        toast.error(`有 ${result.deleteFailures.length} 个文件未能删除，任务单尚未关闭，可重试`);
      } else {
        toast.success(`已关闭并淘汰 ${result.deletedFiles} 个成功素材`);
      }
    } catch (error) {
      toast.error(`关闭失败：${message(error)}`);
    } finally {
      setBusy(null);
    }
  }

  const active = detail?.summary;
  return (
    <PageScaffold
      title="图文任务单"
      caption="整篇组稿、四平台全局排期、JSON 导出与回执闭环"
      right={
        <div className="row gap6">
          <input
            type="date"
            className="inp"
            value={date}
            onChange={(e) => setDate(e.target.value)}
          />
          <button type="button" className="btn sm" onClick={() => setConfigEdit("new")}>
            <Plus className="ic12" />
            配置
          </button>
          <button
            type="button"
            className="btn sm pri"
            disabled={busy !== null}
            onClick={() => void generate()}
          >
            {busy === "generate" ? "生成中…" : "生成任务单"}
          </button>
        </div>
      }
    >
      <div className="pub-sheet-layout">
        <aside className="pub-sheet-rail">
          <div className="pub-rail-head">
            <span>任务单</span>
            <button type="button" className="icb" onClick={() => void loadBase()}>
              <RefreshCw className="ic12" />
            </button>
          </div>
          {sheets.map((sheet) => (
            <button
              type="button"
              key={sheet.id}
              className={`pub-sheet-row ${activeId === sheet.id ? "on" : ""}`}
              onClick={() => setActiveId(sheet.id)}
            >
              <span className="pub-sheet-date">{sheet.date}</span>
              <b className="nowrap ohide">{sheet.productName}</b>
              <span className={`bdg ${statusTone(sheet.status)}`}>{statusName(sheet.status)}</span>
              <small>
                {sheet.postCount} 篇
                {sheet.shortages.length ? ` · 缺 ${sheet.shortages.length}` : ""}
              </small>
            </button>
          ))}
          <div className="pub-config-head">每日配置</div>
          {configs.map((config) => (
            <button
              type="button"
              className="pub-config-row"
              key={config.id}
              onClick={() => setConfigEdit(config)}
            >
              <b>{config.productName}</b>
              <span>
                {config.postsPerDay} 篇 · {config.imagesPerPost} 图 ·{" "}
                {config.platforms.length || "默认"} 平台
              </span>
            </button>
          ))}
        </aside>
        <section className="pub-sheet-main">
          {detail && active ? (
            <>
              <header className="pub-sheet-head">
                <div>
                  <div className="row gap8">
                    <span className={`bdg ${statusTone(active.status)}`}>
                      {statusName(active.status)}
                    </span>
                    <h2>{active.title}</h2>
                  </div>
                  <div className="pub-meta">
                    {active.postCount} 篇 · {detail.posts.reduce((n, p) => n + p.tasks.length, 0)}{" "}
                    个平台任务{active.exportDir ? ` · ${active.exportDir}` : ""}
                  </div>
                </div>
                <div className="row gap6">
                  {active.exportDir && (
                    <button
                      type="button"
                      className="btn sm"
                      disabled={busy !== null}
                      onClick={() =>
                        void action(
                          "open",
                          () => unwrap(commands.openTaskSheetDir(active.id)),
                          "打开目录",
                        )
                      }
                    >
                      <ExternalLink className="ic12" />
                      目录
                    </button>
                  )}
                  {active.status === "draft" && (
                    <>
                      <button
                        type="button"
                        className="btn sm er"
                        disabled={busy !== null}
                        onClick={() => void cancelActive()}
                      >
                        <Trash2 className="ic12" />
                        取消草稿
                      </button>
                      <button
                        type="button"
                        className="btn sm"
                        disabled={busy !== null}
                        onClick={() =>
                          void action(
                            "append",
                            () => unwrap(commands.appendPost(active.id)),
                            "增加一篇",
                          )
                        }
                      >
                        <Plus className="ic12" />
                        增加一篇
                      </button>
                      <button
                        type="button"
                        className="btn sm"
                        disabled={busy !== null}
                        onClick={() =>
                          void action(
                            "regen",
                            () => unwrap(commands.regenerateSheet(active.id)),
                            "重新组稿",
                          )
                        }
                      >
                        <RotateCcw className="ic12" />
                        保留编辑并重组
                      </button>
                      <button
                        type="button"
                        className="btn sm pri"
                        disabled={busy !== null}
                        onClick={() =>
                          void action(
                            "confirm",
                            () => unwrap(commands.confirmSheet(active.id)),
                            "确认任务单",
                          )
                        }
                      >
                        确认
                      </button>
                    </>
                  )}
                  {active.status === "confirmed" && (
                    <>
                      <button
                        type="button"
                        className="btn sm"
                        disabled={busy !== null}
                        onClick={() =>
                          void action(
                            "reopen",
                            () => unwrap(commands.reopenSheet(active.id)),
                            "退回编辑",
                          )
                        }
                      >
                        退回编辑
                      </button>
                      <button
                        type="button"
                        className="btn sm pri"
                        disabled={busy !== null}
                        onClick={() =>
                          void action(
                            "export",
                            () => unwrap(commands.exportTaskSheet(active.id)),
                            "导出任务包",
                          )
                        }
                      >
                        <PackageCheck className="ic12" />
                        导出 JSON
                      </button>
                    </>
                  )}
                  {active.status === "exported" && (
                    <button
                      type="button"
                      className="btn sm"
                      disabled={busy !== null}
                      title="先停止 RPA 并移除 READY.txt；仅在尚无回执时恢复损坏包"
                      onClick={() => {
                        if (
                          window.confirm(
                            "请先停止 RPA 并移除 READY.txt。确认任务包已不会被执行，并把 used 素材退回 held？",
                          )
                        ) {
                          void action(
                            "recover-export",
                            () => unwrap(commands.recoverMissingExport(active.id)),
                            "已恢复为待导出",
                          );
                        }
                      }}
                    >
                      恢复损坏包
                    </button>
                  )}
                  {["exported", "reconciling"].includes(active.status) && (
                    <button
                      type="button"
                      className="btn sm pri"
                      disabled={busy !== null}
                      onClick={() =>
                        void action(
                          "receipt",
                          () => unwrap(commands.collectSheetReceipts(active.id)),
                          "收取回执",
                        )
                      }
                    >
                      收取回执
                    </button>
                  )}
                  {["exported", "reconciling"].includes(active.status) && (
                    <button
                      type="button"
                      className="btn sm"
                      disabled={busy !== null}
                      onClick={() => void closeActive()}
                    >
                      <Archive className="ic12" />
                      关闭
                    </button>
                  )}
                </div>
              </header>
              {active.shortages.length > 0 && (
                <div className="pub-shortage">
                  <b>素材不足，已按现有库存生成</b>
                  {active.shortages.map((s, index) => (
                    <span key={`${s.kind}-${index}`}>
                      {s.detail}：需要 {s.needed}，可用 {s.available}
                    </span>
                  ))}
                </div>
              )}
              <div className="pub-post-list">
                {detail.posts.map((post) => (
                  <PostCard
                    key={post.id}
                    post={post}
                    readonly={active.status !== "draft"}
                    images={images}
                    titles={titles}
                    bodies={bodies}
                    onChanged={() => loadDetail(active.id)}
                  />
                ))}
              </div>
            </>
          ) : (
            <div className="bigempty">
              <div className="fw6">选择任务单，或为 {date} 生成一批</div>
              <div className="t3 mt6">同一天所有商品共用一条全局排期时间轴。</div>
            </div>
          )}
        </section>
      </div>
      {configEdit && (
        <ConfigModal
          value={configEdit === "new" ? null : configEdit}
          products={products}
          onClose={() => setConfigEdit(null)}
          onSaved={loadBase}
        />
      )}
    </PageScaffold>
  );
}

function statusName(status: string) {
  return (
    (
      {
        draft: "草稿",
        confirmed: "已确认",
        exported: "已导出",
        reconciling: "回执中",
        closed: "已关闭",
      } as Record<string, string>
    )[status] ?? status
  );
}
function statusTone(status: string) {
  return status === "draft" ? "b-amber" : status === "closed" ? "b-gray" : "b-blue";
}

function PostCard({
  post,
  readonly,
  images,
  titles,
  bodies,
  onChanged,
}: {
  post: PostView;
  readonly: boolean;
  images: ImageAssetView[];
  titles: CopyItemView[];
  bodies: CopyItemView[];
  onChanged: () => Promise<void>;
}) {
  const [dragged, setDragged] = useState<number | null>(null);
  const run = async (promise: Promise<unknown>, label = "更新") => {
    try {
      await unwrap(promise as ReturnType<typeof commands.deletePost>);
      await onChanged();
    } catch (error) {
      toast.error(`${label}失败：${message(error)}`);
    }
  };
  const reorder = (target: number) => {
    if (dragged === null || dragged === target) return;
    const ids = post.images.map((image) => image.assetId);
    const [moved] = ids.splice(dragged, 1);
    if (moved === undefined) return;
    ids.splice(target, 0, moved);
    void run(commands.reorderPostImages(post.id, ids), "图片排序");
  };
  return (
    <article className={`pub-post-card ${post.edited ? "edited" : ""}`}>
      <header>
        <span className="pub-code lg">{post.contentCode}</span>
        <span className="bdg b-gray">{post.kind === "mixed" ? "混搭" : "单款"}</span>
        {post.edited && <span className="bdg b-blue">已手改</span>}
        <div className="f1" />
        {!readonly && (
          <button
            type="button"
            className="icb er"
            title="删除整篇"
            onClick={() => {
              if (window.confirm("删除整篇并释放图片与文案？"))
                void run(commands.deletePost(post.id), "删除");
            }}
          >
            <Trash2 className="ic12" />
          </button>
        )}
      </header>
      <div className="pub-post-body">
        <div className="pub-post-images">
          {post.images.map((image, index) => (
            <div
              key={image.assetId}
              className="pub-post-image"
              draggable={!readonly}
              onDragStart={() => setDragged(index)}
              onDragOver={(e) => e.preventDefault()}
              onDrop={() => reorder(index)}
            >
              <NatThumb path={image.thumb || image.path} className="pub-post-thumb" />
              <span>
                {index + 1} · {image.skuCode}
              </span>
              {!readonly && (
                <select
                  className="inp"
                  value=""
                  title="替换图片"
                  onChange={(e) => {
                    if (e.target.value)
                      void run(
                        commands.replacePostImage(post.id, index, Number(e.target.value)),
                        "替换图片",
                      );
                  }}
                >
                  <option value="">替换…</option>
                  {images.map((a) => (
                    <option key={a.id} value={a.id}>
                      {a.id} · {a.skuCode}
                    </option>
                  ))}
                </select>
              )}
            </div>
          ))}
        </div>
        <div className="pub-post-copy">
          <label>
            <span>
              标题 <i>{post.title.length} 字</i>
            </span>
            <div className="pub-copy-editor">
              <textarea
                key={`${post.id}-title-${post.title}`}
                className="inp"
                defaultValue={post.title}
                readOnly={readonly}
                onBlur={(e) => {
                  if (e.target.value !== post.title)
                    void run(commands.updatePostText(post.id, "title", e.target.value), "标题更新");
                }}
              />
              {!readonly && (
                <select
                  className="inp"
                  value=""
                  onChange={(e) => {
                    if (e.target.value)
                      void run(
                        commands.replacePostCopy(post.id, "title", Number(e.target.value)),
                        "替换标题",
                      );
                  }}
                >
                  <option value="">从库替换…</option>
                  {titles
                    .filter((item) => item.state === "free")
                    .map((item) => (
                      <option key={item.id} value={item.id}>
                        {item.text.slice(0, 28)}
                      </option>
                    ))}
                </select>
              )}
            </div>
          </label>
          <label>
            <span>
              正文 <i>{post.body.length} 字</i>
            </span>
            <div className="pub-copy-editor">
              <textarea
                key={`${post.id}-body-${post.body}`}
                className="inp body"
                defaultValue={post.body}
                readOnly={readonly}
                onBlur={(e) => {
                  if (e.target.value !== post.body)
                    void run(commands.updatePostText(post.id, "body", e.target.value), "正文更新");
                }}
              />
              {!readonly && (
                <select
                  className="inp"
                  value=""
                  onChange={(e) => {
                    if (e.target.value)
                      void run(
                        commands.replacePostCopy(post.id, "body", Number(e.target.value)),
                        "替换正文",
                      );
                  }}
                >
                  <option value="">从库替换…</option>
                  {bodies
                    .filter((item) => item.state === "free")
                    .map((item) => (
                      <option key={item.id} value={item.id}>
                        {item.text.slice(0, 28)}
                      </option>
                    ))}
                </select>
              )}
            </div>
          </label>
          <label>
            <span>话题</span>
            <input
              key={`${post.id}-topics-${post.topics.join("|")}`}
              className="inp"
              defaultValue={post.topics
                .map((topic) => (topic.startsWith("#") ? topic : `#${topic}`))
                .join(" ")}
              readOnly={readonly}
              onBlur={(e) => {
                const topics = e.target.value.split(/[\s,，#]+/).filter(Boolean);
                if (topics.join("|") !== post.topics.join("|"))
                  void run(commands.updatePostTopics(post.id, topics), "话题更新");
              }}
            />
          </label>
        </div>
      </div>
      <footer>
        {post.tasks.map((task) => (
          <span
            key={task.id}
            className={`pub-task-chip ${task.status}`}
            title={task.resultMsg ?? undefined}
          >
            <b>{task.platformZh}</b>
            {task.scheduledAt.slice(5)}
            <i>
              {task.status === "pending"
                ? "待执行"
                : task.status === "done"
                  ? "成功"
                  : task.status === "failed"
                    ? `失败 · ${task.failKind ?? "其他"}`
                    : task.status}
            </i>
          </span>
        ))}
      </footer>
    </article>
  );
}

function ConfigModal({
  value,
  products,
  onClose,
  onSaved,
}: {
  value: SheetConfigView | null;
  products: ProductView[];
  onClose: () => void;
  onSaved: () => Promise<void>;
}) {
  const initialProduct = products.find((p) => p.id === value?.productId) ?? products[0] ?? null;
  const [productId, setProductId] = useState(value?.productId ?? initialProduct?.id ?? 0);
  const [name, setName] = useState(value?.name ?? "日常图文");
  const [skuScope, setSkuScope] = useState<number[]>(value?.skuScope ?? []);
  const [platforms, setPlatforms] = useState<string[]>(value?.platforms ?? []);
  const [posts, setPosts] = useState(value?.postsPerDay ?? 5);
  const [images, setImages] = useState(value?.imagesPerPost ?? 5);
  const [mixed, setMixed] = useState(value?.mixedCount ?? 1);
  const [anchors, setAnchors] = useState((value?.anchors ?? ["10:00", "14:00", "18:00"]).join(" "));
  const [jitter, setJitter] = useState(value?.jitterMin ?? 15);
  const [gap, setGap] = useState(value?.minGapMin ?? 3);
  const [targetDay, setTargetDay] = useState(value?.targetDay ?? "next");
  const [enabled, setEnabled] = useState(value?.enabled ?? true);
  const product = products.find((p) => p.id === productId) ?? null;
  const save = async () => {
    try {
      await unwrap(
        commands.saveSheetConfig(value?.id ?? null, {
          productId,
          name,
          skuScope,
          platforms,
          postsPerDay: posts,
          imagesPerPost: images,
          mixedCount: Math.min(mixed, posts),
          anchors: anchors.split(/[\s,，]+/).filter(Boolean),
          jitterMin: jitter,
          minGapMin: gap,
          targetDay,
          enabled,
        }),
      );
      await onSaved();
      onClose();
      toast.success("每日配置已保存");
    } catch (error) {
      toast.error(`配置保存失败：${message(error)}`);
    }
  };
  return (
    <Modal
      title={value ? "编辑每日任务单配置" : "新增每日任务单配置"}
      width="w700"
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
            disabled={!productId || !name.trim()}
            onClick={() => void save()}
          >
            保存
          </button>
        </>
      }
    >
      <div className="pub-form-grid">
        <label>
          <span>商品</span>
          <select
            className="inp"
            value={productId}
            disabled={Boolean(value)}
            onChange={(e) => {
              setProductId(Number(e.target.value));
              setSkuScope([]);
            }}
          >
            <option value={0}>请选择</option>
            {products.map((p) => (
              <option key={p.id} value={p.id}>
                {p.code} · {p.name}
              </option>
            ))}
          </select>
        </label>
        <label>
          <span>配置名</span>
          <input className="inp" value={name} onChange={(e) => setName(e.target.value)} />
        </label>
        <label>
          <span>每日篇数</span>
          <input
            type="number"
            className="inp"
            min={1}
            value={posts}
            onChange={(e) => setPosts(Number(e.target.value))}
          />
        </label>
        <label>
          <span>每篇图片</span>
          <input
            type="number"
            className="inp"
            min={1}
            value={images}
            onChange={(e) => setImages(Number(e.target.value))}
          />
        </label>
        <label>
          <span>混搭篇数</span>
          <input
            type="number"
            className="inp"
            min={0}
            max={posts}
            value={mixed}
            onChange={(e) => setMixed(Number(e.target.value))}
          />
        </label>
        <label>
          <span>目标日</span>
          <select className="inp" value={targetDay} onChange={(e) => setTargetDay(e.target.value)}>
            <option value="same">当天</option>
            <option value="next">次日</option>
          </select>
        </label>
        <label className="span2">
          <span>时间锚点（HH:MM）</span>
          <input className="inp" value={anchors} onChange={(e) => setAnchors(e.target.value)} />
        </label>
        <label>
          <span>随机抖动（分钟）</span>
          <input
            type="number"
            className="inp"
            min={0}
            value={jitter}
            onChange={(e) => setJitter(Number(e.target.value))}
          />
        </label>
        <label>
          <span>同平台最小间隔</span>
          <input
            type="number"
            className="inp"
            min={1}
            value={gap}
            onChange={(e) => setGap(Number(e.target.value))}
          />
        </label>
        <fieldset className="pub-fieldset span2">
          <legend>平台（不选则沿用商品平台）</legend>
          {Object.entries(PLATFORM_LABELS).map(([code, label]) => (
            <label key={code} className="pub-check">
              <input
                type="checkbox"
                checked={platforms.includes(code)}
                onChange={(e) =>
                  setPlatforms(
                    e.target.checked ? [...platforms, code] : platforms.filter((p) => p !== code),
                  )
                }
              />
              {label}
            </label>
          ))}
        </fieldset>
        <fieldset className="pub-fieldset span2">
          <legend>SKU 范围（不选则全部）</legend>
          {product?.skus.map((sku) => (
            <label key={sku.id} className="pub-check">
              <input
                type="checkbox"
                checked={skuScope.includes(sku.id)}
                onChange={(e) =>
                  setSkuScope(
                    e.target.checked
                      ? [...skuScope, sku.id]
                      : skuScope.filter((id) => id !== sku.id),
                  )
                }
              />
              {sku.code} · {sku.name}
            </label>
          ))}
        </fieldset>
        <label className="pub-check">
          <input type="checkbox" checked={enabled} onChange={(e) => setEnabled(e.target.checked)} />
          启用自动生成
        </label>
      </div>
    </Modal>
  );
}
