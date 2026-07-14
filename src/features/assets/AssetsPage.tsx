import { Modal } from "@/components/ui/Modal";
import {
  type InboxItemView,
  type PackView,
  type SkuDetail,
  type SkuView,
  type TextItemView,
  commands,
  unwrap,
} from "@/lib/ipc";
import { packLifeVisual, tierVisual } from "@/lib/status";
import { cn } from "@/lib/utils";
import { usePublishStore } from "@/stores/publish";
import { ChevronLeft, ChevronRight, Plus, Search } from "lucide-react";
import { useCallback, useEffect, useState } from "react";

type Tier = "" | "hot" | "warm" | "cold";

/** 资产库页（原型 publish.dc.html 资产库 + SKU 详情 + 收件箱）。 */
export function AssetsPage() {
  const [detailId, setDetailId] = useState<number | null>(null);
  const [tab, setTab] = useState<"sku" | "inbox">("sku");

  if (detailId != null) {
    return <SkuDetailView id={detailId} onBack={() => setDetailId(null)} />;
  }
  return <AssetsList tab={tab} setTab={setTab} onOpen={setDetailId} />;
}

// ─────────────────────────────────────────────────────── SKU 列表 + 收件箱

function AssetsList({
  tab,
  setTab,
  onOpen,
}: {
  tab: "sku" | "inbox";
  setTab: (t: "sku" | "inbox") => void;
  onOpen: (id: number) => void;
}) {
  const badges = usePublishStore((s) => s.badges);
  const refreshBadges = usePublishStore((s) => s.refreshBadges);
  const [view, setView] = useState<"table" | "cards">("table");
  const [tier, setTier] = useState<Tier>("");
  const [warnOnly, setWarnOnly] = useState(false);
  const [showOff, setShowOff] = useState(false);
  const [query, setQuery] = useState("");
  const [rows, setRows] = useState<SkuView[]>([]);
  const [editing, setEditing] = useState<SkuView | "new" | null>(null);
  const [err, setErr] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const list = await unwrap(
        commands.listSkus({
          tier: tier || null,
          warnOnly: warnOnly || null,
          status: showOff ? "paused" : null,
          query: query || null,
        }),
      );
      setRows(list);
      setErr(null);
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    }
  }, [tier, warnOnly, showOff, query]);

  useEffect(() => {
    void load();
  }, [load]);

  const warnCount = badges.warn;

  return (
    <div className="col f1 ohide">
      <div className="phd">
        <span className="ptt">资产库</span>
        <span className="pcap">SKU 与内容资产 · 收件箱自动收录</span>
        <div className="f1" />
        <div className="seg">
          <span className={cn("sgi", tab === "sku" && "on")} onClick={() => setTab("sku")}>
            SKU 资产
          </span>
          <span className={cn("sgi", tab === "inbox" && "on")} onClick={() => setTab("inbox")}>
            收件箱
            {badges.unclaimed > 0 && (
              <span className="bdg b-amber" style={{ height: 16, padding: "0 5px" }}>
                {badges.unclaimed}
              </span>
            )}
          </span>
        </div>
      </div>

      {tab === "sku" ? (
        <>
          <div className="fx ac gap8" style={{ padding: "10px 18px 8px" }}>
            <div className="seg">
              {(
                [
                  ["", "全部"],
                  ["hot", "热款"],
                  ["warm", "温款"],
                  ["cold", "冷款"],
                ] as [Tier, string][]
              ).map(([t, label]) => (
                <span
                  key={label}
                  className={cn("sgi", tier === t && !warnOnly && !showOff && "on")}
                  onClick={() => {
                    setTier(t);
                    setWarnOnly(false);
                    setShowOff(false);
                  }}
                >
                  {label}
                </span>
              ))}
              <span
                className={cn("sgi", warnOnly && "on")}
                onClick={() => {
                  setWarnOnly((v) => !v);
                  setShowOff(false);
                }}
              >
                预警 {warnCount}
              </span>
              <span
                className={cn("sgi", showOff && "on")}
                onClick={() => {
                  setShowOff((v) => !v);
                  setWarnOnly(false);
                }}
              >
                停发
              </span>
            </div>
            <div className="srch">
              <Search className="ic12" />
              <input
                className="inp"
                style={{ width: 170 }}
                placeholder="搜索编码或款式名…"
                value={query}
                onChange={(e) => setQuery(e.target.value)}
              />
            </div>
            <div className="f1" />
            <div className="seg">
              <span
                className={cn("sgi", view === "table" && "on")}
                onClick={() => setView("table")}
              >
                表格
              </span>
              <span
                className={cn("sgi", view === "cards" && "on")}
                onClick={() => setView("cards")}
              >
                卡片
              </span>
            </div>
            <button type="button" className="btn sm" onClick={() => setEditing("new")}>
              <Plus className="ic12" />
              新建 SKU
            </button>
          </div>

          {err && (
            <div className="ban" style={{ margin: "0 18px" }}>
              {err}
            </div>
          )}

          <div className="f1" style={{ overflow: "auto", minHeight: 0 }}>
            {view === "table" ? (
              <>
                <div className="sgrid th">
                  <span>SKU 编码</span>
                  <span>款式 / 商品名</span>
                  <span>分层</span>
                  <span>三池余量</span>
                  <span>预警 / 状态</span>
                  <span>最近发布</span>
                  <span />
                </div>
                {rows.map((s) => (
                  <SkuRow key={s.id} s={s} onOpen={onOpen} />
                ))}
                {rows.length === 0 && <EmptyList />}
              </>
            ) : (
              <div className="skcards">
                {rows.map((s) => (
                  <SkuCard key={s.id} s={s} onOpen={onOpen} />
                ))}
                {rows.length === 0 && <EmptyList />}
              </div>
            )}
          </div>
        </>
      ) : (
        <InboxPanel onChanged={() => void refreshBadges()} />
      )}

      {editing && (
        <SkuEditModal
          sku={editing === "new" ? null : editing}
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null);
            void load();
            void refreshBadges();
          }}
        />
      )}
    </div>
  );
}

function EmptyList() {
  return (
    <div className="bigempty" style={{ padding: "56px 20px" }}>
      <div className="fs13 fw5 t2">暂无 SKU</div>
      <div className="fs12 t3">
        点右上「新建 SKU」创建；或在设置页配置根目录后由收件箱自动收录内容
      </div>
    </div>
  );
}

function PoolCounts({ s }: { s: SkuView }) {
  return (
    <span className="fx ac gap10">
      <span className={cn("pooln", s.warnMaterial && "pwarn")}>
        <b>素</b>
        {s.materialCount}
      </span>
      <span className={cn("pooln", s.warnTitle && "pwarn")}>
        <b>题</b>
        {s.titleCount}
      </span>
      <span className={cn("pooln", s.warnBody && "pwarn")}>
        <b>文</b>
        {s.bodyCount}
      </span>
    </span>
  );
}

function lastPublishLabel(ts: number | null): string {
  if (!ts) return "—";
  const d = new Date(ts * 1000);
  return `${d.getMonth() + 1}月${d.getDate()}日`;
}

function SkuRow({ s, onOpen }: { s: SkuView; onOpen: (id: number) => void }) {
  const t = tierVisual(s.tier, s.isGeneral);
  const off = s.status === "paused";
  return (
    <div className={cn("sgrid tr", off && "droff")} onClick={() => onOpen(s.id)}>
      <span className="pid">{s.code}</span>
      <span className="ohide">
        <span className="fw5 nowrap ohide" style={{ display: "block" }}>
          {s.styleName}
        </span>
        <span className="fs10 t3 nowrap ohide" style={{ display: "block" }}>
          {s.productName || "—"}
        </span>
      </span>
      <span>
        <span className={cn("bdg", t.badgeClass)}>{t.label}</span>
      </span>
      <PoolCounts s={s} />
      <span className="fx ac gap6">
        {s.warn && (
          <span className="bdg b-amber">
            <span className="dt" />
            余量低
          </span>
        )}
        {off && <span className="bdg b-gray">停发</span>}
      </span>
      <span className="fs11 t3 nowrap">{lastPublishLabel(s.lastPublished)}</span>
      <span className="t3 fs12">
        <ChevronRight className="ic12" />
      </span>
    </div>
  );
}

function SkuCard({ s, onOpen }: { s: SkuView; onOpen: (id: number) => void }) {
  const t = tierVisual(s.tier, s.isGeneral);
  const off = s.status === "paused";
  return (
    <div className={cn("skc", off && "droff")} onClick={() => onOpen(s.id)}>
      <div className="fx ac gap8">
        <span className="pid">{s.code}</span>
        <span className={cn("bdg", t.badgeClass)}>{t.label}</span>
        <div className="f1" />
        {off && <span className="bdg b-gray">停发</span>}
      </div>
      <div className="fw5 fs13 mt10 nowrap ohide">{s.styleName}</div>
      <div className="fs10 t3 nowrap ohide mt4">{s.productName || "—"}</div>
      <div className="mt10">
        <PoolCounts s={s} />
      </div>
      <div className="fx ac gap6 mt10">
        <span className="fs10 t3 f1 nowrap">最近 {lastPublishLabel(s.lastPublished)}</span>
        {s.warn && (
          <span className="bdg b-amber">
            <span className="dt" />
            余量低
          </span>
        )}
      </div>
    </div>
  );
}

// ─────────────────────────────────────────────────────── 收件箱面板

function InboxPanel({ onChanged }: { onChanged: () => void }) {
  const [claims, setClaims] = useState<InboxItemView[]>([]);
  const [fails, setFails] = useState<InboxItemView[]>([]);
  const [claimTarget, setClaimTarget] = useState<InboxItemView | null>(null);

  const load = useCallback(async () => {
    setClaims(await unwrap(commands.listInboxItems("unclaimed")));
    setFails(await unwrap(commands.listInboxItems("failed")));
  }, []);
  useEffect(() => {
    void load();
  }, [load]);

  const rescan = async () => {
    await unwrap(commands.rescanInbox());
    await load();
    onChanged();
  };
  const discard = async (id: number) => {
    await unwrap(commands.discardInboxItem(id));
    await load();
    onChanged();
  };
  const retry = async (id: number) => {
    await unwrap(commands.retryInboxItem(id));
    await load();
    onChanged();
  };

  return (
    <div className="pbody">
      <div className="cwrap" style={{ maxWidth: 860 }}>
        <div className="fx ac gap8" style={{ marginBottom: 12 }}>
          <div className="f1" />
          <button type="button" className="btn sm gho" onClick={() => void rescan()}>
            重扫收件箱
          </button>
        </div>
        <div className="card">
          <div className="chead">
            <span className="fw6 fs13">待认领</span>
            <span className="cnt">{claims.length} 条</span>
            <span className="pcap">无法关联到已知 SKU 的收件箱内容 · 不会被丢弃</span>
          </div>
          {claims.map((c) => (
            <div
              key={c.id}
              className="txrow"
              style={{ borderTop: "1px solid var(--line)", borderBottom: "none" }}
            >
              <span className="chip">{c.fileName}</span>
              {c.kind && <span className="bdg b-gray">{c.kind}</span>}
              <span className="fs11 t3 f1 nowrap ohide">{c.detail ?? ""}</span>
              <button type="button" className="btn sm" onClick={() => setClaimTarget(c)}>
                指认 SKU
              </button>
              <button type="button" className="btn sm gho dng" onClick={() => void discard(c.id)}>
                丢弃
              </button>
            </div>
          ))}
          {claims.length === 0 && (
            <div
              className="txrow t3 fs12"
              style={{ borderTop: "1px solid var(--line)", borderBottom: "none" }}
            >
              没有待认领的内容
            </div>
          )}
        </div>

        <div className="card mt14">
          <div className="chead">
            <span className="fw6 fs13">解析失败</span>
            <span className="cnt">{fails.length} 条</span>
            <span className="pcap">格式不符合收录规范 · 原文件已保留</span>
          </div>
          {fails.map((c) => (
            <div
              key={c.id}
              className="txrow"
              style={{ borderTop: "1px solid var(--line)", borderBottom: "none" }}
            >
              <span className="chip">{c.fileName}</span>
              <span className="terr f1">{c.detail ?? "解析失败"}</span>
              <button type="button" className="btn sm" onClick={() => void retry(c.id)}>
                重试解析
              </button>
              <button type="button" className="btn sm gho dng" onClick={() => void discard(c.id)}>
                丢弃
              </button>
            </div>
          ))}
          {fails.length === 0 && (
            <div
              className="txrow t3 fs12"
              style={{ borderTop: "1px solid var(--line)", borderBottom: "none" }}
            >
              没有解析失败的文件
            </div>
          )}
        </div>

        <div className="fs11 t3 mt14" style={{ lineHeight: 1.8 }}>
          收件箱监听根目录 <span className="chip">收件箱/</span> — Claude/Codex 生成的 TXT 与外部 AI
          图片落盘后自动收录：按 文件头【SKU】 › 文件名前缀 › 文件夹名 三处冗余识别归属；
          成功收录的原文件移入 <span className="chip">收件箱/已收录/</span> 归档。
        </div>
      </div>

      {claimTarget && (
        <ClaimModal
          item={claimTarget}
          onClose={() => setClaimTarget(null)}
          onDone={() => {
            setClaimTarget(null);
            void load();
            onChanged();
          }}
        />
      )}
    </div>
  );
}

function ClaimModal({
  item,
  onClose,
  onDone,
}: {
  item: InboxItemView;
  onClose: () => void;
  onDone: () => void;
}) {
  const [skus, setSkus] = useState<SkuView[]>([]);
  useEffect(() => {
    void unwrap(commands.listSkus({ tier: null, warnOnly: null, status: null, query: null })).then(
      setSkus,
    );
  }, []);
  const pick = async (code: string) => {
    await unwrap(commands.claimInboxItem(item.id, code));
    onDone();
  };
  return (
    <Modal
      title="指认 SKU"
      onClose={onClose}
      headerExtra={<span className="chip">{item.fileName}</span>}
      footer={
        <>
          <span className="fs11 t3">指认后内容立即入该 SKU 对应内容池</span>
          <div className="f1" />
          <button type="button" className="btn sm" onClick={onClose}>
            取消
          </button>
        </>
      }
    >
      <div style={{ padding: 8 }}>
        {skus
          .filter((s) => !s.isGeneral)
          .map((s) => {
            const t = tierVisual(s.tier);
            return (
              <div key={s.id} className="pickrow" onClick={() => void pick(s.code)}>
                <span className="pid">{s.code}</span>
                <span className="fw5 fs12 f1 nowrap ohide">{s.styleName}</span>
                <span className={cn("bdg", t.badgeClass)}>{t.label}</span>
              </div>
            );
          })}
      </div>
    </Modal>
  );
}

// ─────────────────────────────────────────────────────── SKU 详情

function SkuDetailView({ id, onBack }: { id: number; onBack: () => void }) {
  const [detail, setDetail] = useState<SkuDetail | null>(null);
  const [packs, setPacks] = useState<PackView[]>([]);
  const [titles, setTitles] = useState<TextItemView[]>([]);
  const [bodies, setBodies] = useState<TextItemView[]>([]);
  const [poolTab, setPoolTab] = useState<"pk" | "ti" | "bo" | "hi">("pk");
  const [editOpen, setEditOpen] = useState(false);
  const [packOpen, setPackOpen] = useState<PackView | null>(null);
  const [addText, setAddText] = useState<"title" | "body" | null>(null);

  const load = useCallback(async () => {
    setDetail(await unwrap(commands.getSkuDetail(id)));
    setPacks(await unwrap(commands.listAssetPacks(id)));
    setTitles(await unwrap(commands.listTextItems(id, "title")));
    setBodies(await unwrap(commands.listTextItems(id, "body")));
  }, [id]);
  useEffect(() => {
    void load();
  }, [load]);

  if (!detail) return <div className="col f1" />;
  const s = detail.sku;
  const t = tierVisual(s.tier, s.isGeneral);
  const off = s.status === "paused";

  const toggleStatus = async () => {
    await unwrap(commands.setSkuStatus(id, off ? "active" : "paused"));
    await load();
  };

  const importMedia = async () => {
    const paths = await unwrap(commands.pickImageFiles());
    if (paths.length === 0) return;
    await unwrap(commands.importMediaFiles(id, paths));
    await load();
  };

  return (
    <div className="col f1 ohide">
      <div className="phd">
        <button type="button" className="icb" onClick={onBack} aria-label="返回">
          <ChevronLeft className="ic12" />
        </button>
        <span className="pid">{s.code}</span>
        <span className="ptt">{s.styleName}</span>
        <span className={cn("bdg", t.badgeClass)}>{t.label}</span>
        {off && <span className="bdg b-gray">停发 · 不参与排期</span>}
        {s.warn && (
          <span className="bdg b-amber">
            <span className="dt" />
            余量低
          </span>
        )}
        <div className="f1" />
        {!s.isGeneral && (
          <>
            <button type="button" className="btn sm gho" onClick={() => setEditOpen(true)}>
              编辑档案
            </button>
            <button type="button" className="btn sm gho" onClick={() => void toggleStatus()}>
              {off ? "恢复在售" : "停发"}
            </button>
          </>
        )}
      </div>

      <div className="pbody">
        <div className="cwrap">
          {!s.isGeneral && (
            <div className="card" style={{ padding: "13px 16px" }}>
              <div className="arow">
                <b>商品名称</b>
                <span>{s.productName || "—"}</span>
                <b>固定话题标签</b>
                <span className="fx ac gap6 wrap">
                  {s.topics.map((tag) => (
                    <span key={tag} className="tagchip">
                      #{tag}
                    </span>
                  ))}
                  <span className="fs11 t3">前 5 个进任务单话题列 · 全平台共用</span>
                </span>
                <b>最近发布</b>
                <span className="fs12">{lastPublishLabel(s.lastPublished)}</span>
              </div>
            </div>
          )}

          <div className="fx ac gap8 mt14">
            <div className="seg">
              <span
                className={cn("sgi", poolTab === "pk" && "on")}
                onClick={() => setPoolTab("pk")}
              >
                素材池 {s.materialCount}
              </span>
              <span
                className={cn("sgi", poolTab === "ti" && "on")}
                onClick={() => setPoolTab("ti")}
              >
                标题池 {s.titleCount}
              </span>
              <span
                className={cn("sgi", poolTab === "bo" && "on")}
                onClick={() => setPoolTab("bo")}
              >
                正文池 {s.bodyCount}
              </span>
              <span
                className={cn("sgi", poolTab === "hi" && "on")}
                onClick={() => setPoolTab("hi")}
              >
                发布历史
              </span>
            </div>
            <div className="f1" />
            {poolTab === "pk" && (
              <>
                <span className="fs11 t3">生命周期由使用台账自动驱动</span>
                <button type="button" className="btn sm" onClick={() => void importMedia()}>
                  导入素材
                </button>
              </>
            )}
            {poolTab === "ti" && (
              <button type="button" className="btn sm" onClick={() => setAddText("title")}>
                手动新增
              </button>
            )}
            {poolTab === "bo" && (
              <button type="button" className="btn sm" onClick={() => setAddText("body")}>
                手动新增
              </button>
            )}
          </div>

          <div className="card mt10">
            {poolTab === "pk" && <PackGrid packs={packs} onOpen={setPackOpen} />}
            {poolTab === "ti" && <TextList items={titles} onChanged={load} />}
            {poolTab === "bo" && <TextList items={bodies} onChanged={load} bodyEmpty />}
            {poolTab === "hi" && <HistoryList detail={detail} />}
          </div>
        </div>
      </div>

      {editOpen && (
        <SkuEditModal
          sku={s}
          onClose={() => setEditOpen(false)}
          onSaved={() => {
            setEditOpen(false);
            void load();
          }}
        />
      )}
      {packOpen && (
        <PackModal
          pack={packOpen}
          onClose={() => setPackOpen(null)}
          onChanged={() => {
            setPackOpen(null);
            void load();
          }}
        />
      )}
      {addText && (
        <AddTextModal
          skuId={id}
          kind={addText}
          onClose={() => setAddText(null)}
          onSaved={() => {
            setAddText(null);
            void load();
          }}
        />
      )}
    </div>
  );
}

function PackGrid({ packs, onOpen }: { packs: PackView[]; onOpen: (p: PackView) => void }) {
  if (packs.length === 0) {
    return (
      <div className="bigempty" style={{ padding: "44px 20px" }}>
        <div className="fs13 fw5 t2">素材池为空</div>
        <div className="fs12 t3">
          收件箱放入该 SKU 的图片/视频会自动入池；也可点上方「导入素材」
        </div>
      </div>
    );
  }
  return (
    <div className="packgrid">
      {packs.map((p) => {
        const life = packLifeVisual(p.derived);
        const dim = p.derived === "retired" || p.derived === "exhausted";
        return (
          <div key={p.id} className={cn("packc", dim && "dim")} onClick={() => onOpen(p)}>
            <div className="packimg" style={{ background: "var(--inset)" }}>
              <span className="packn">{p.kind === "video" ? "视频" : `图 ${p.fileCount}`}</span>
              <span className="phl t3 fs11">{p.kind === "video" ? "▶" : "🖼"}</span>
            </div>
            <div className="fx ac gap6 mt6">
              <span className={cn("bdg", life.badgeClass)}>{life.label}</span>
              {p.locked && <span className="fs10 t3 nowrap ohide">已锁定</span>}
            </div>
          </div>
        );
      })}
    </div>
  );
}

function TextList({
  items,
  onChanged,
  bodyEmpty,
}: {
  items: TextItemView[];
  onChanged: () => Promise<void> | void;
  bodyEmpty?: boolean;
}) {
  if (items.length === 0) {
    return (
      <div className="bigempty" style={{ padding: "44px 20px" }}>
        <div className="fs13 fw5 t2">{bodyEmpty ? "正文池为空" : "标题池为空"}</div>
        <div className="fs12 t3">
          {bodyEmpty
            ? "该 SKU 若不排图文任务则无需正文；图文任务需要 标题 + 正文"
            : "收件箱收录该 SKU 的标题文件会自动入池；也可手动新增"}
        </div>
      </div>
    );
  }
  const toggle = async (id: number, enabled: boolean) => {
    await unwrap(commands.setTextItemEnabled(id, enabled));
    await onChanged();
  };
  return (
    <>
      {items.map((x) => (
        <div
          key={x.id}
          className="txrow"
          style={bodyEmpty ? { alignItems: "flex-start", padding: "10px 16px" } : undefined}
        >
          <span
            className={cn("f1", !bodyEmpty && "nowrap ohide")}
            style={bodyEmpty ? { lineHeight: 1.7, userSelect: "text" } : undefined}
          >
            {x.text}
          </span>
          <span className="bdg b-gray">{x.platformZh}</span>
          {!x.enabled && <span className="chip">已停用</span>}
          <span className="fs11 t3 nowrap" style={{ width: 56, textAlign: "right" }}>
            用过 {x.useCount} 次
          </span>
          <span className="tract">
            <button
              type="button"
              className="btn sm gho"
              onClick={() => void toggle(x.id, !x.enabled)}
            >
              {x.enabled ? "停用" : "启用"}
            </button>
          </span>
        </div>
      ))}
    </>
  );
}

function HistoryList({ detail }: { detail: SkuDetail }) {
  if (detail.history.length === 0) {
    return (
      <div className="bigempty" style={{ padding: "44px 20px" }}>
        <div className="fs13 fw5 t2">尚无发布记录</div>
        <div className="fs12 t3">执行器回执对账成功后，使用台账会记录到这里</div>
      </div>
    );
  }
  return (
    <>
      {detail.history.map((h) => (
        <div key={h.taskCode} className="txrow">
          <span className="fs11 t3 nowrap" style={{ width: 96 }}>
            {h.date}
          </span>
          <span className="bdg b-gray">{h.platform}</span>
          <span className="pid">{h.taskCode}</span>
          <span className="f1" />
          {h.url && (
            <a className="fs11" href={h.url} target="_blank" rel="noreferrer">
              发布链接 ↗
            </a>
          )}
        </div>
      ))}
    </>
  );
}

// ─────────────────────────────────────────────────────── 弹层

function PackModal({
  pack,
  onClose,
  onChanged,
}: {
  pack: PackView;
  onClose: () => void;
  onChanged: () => void;
}) {
  const life = packLifeVisual(pack.derived);
  const retire = async () => {
    await unwrap(commands.retirePack(pack.id));
    onChanged();
  };
  const restore = async () => {
    await unwrap(commands.restorePack(pack.id));
    onChanged();
  };
  const canRetire = pack.derived !== "retired" && !pack.locked;
  return (
    <Modal
      title={<span className="mono">{pack.dirRel.split("/").pop()}</span>}
      width="w640"
      onClose={onClose}
      headerExtra={
        <>
          <span className={cn("bdg", life.badgeClass)}>{life.label}</span>
          {pack.locked && <span className="bdg b-blue">被未关闭任务单引用 · 已锁定</span>}
        </>
      }
      footer={
        <>
          <span className="fs11 t3">
            新入库 → 可用 → 已用尽（窗口期满自动回可用）· 退役为人工终态
          </span>
          <div className="f1" />
          {pack.derived === "retired" ? (
            <button type="button" className="btn sm" onClick={() => void restore()}>
              恢复可用
            </button>
          ) : (
            canRetire && (
              <button type="button" className="btn sm gho dng" onClick={() => void retire()}>
                退役
              </button>
            )
          )}
          <button type="button" className="btn sm" onClick={onClose}>
            关闭
          </button>
        </>
      }
    >
      <div className="fs11 fw6 t3" style={{ letterSpacing: ".05em" }}>
        包内文件 · ASCII 规范化命名
      </div>
      <div className="mt6 col gap4">
        {pack.files.map((f) => (
          <span key={f.name} className="chip" style={{ alignSelf: "flex-start" }}>
            {f.name}
            {f.origName && f.origName !== f.name ? ` ← ${f.origName}` : ""}
          </span>
        ))}
      </div>
      {pack.availableAt && (
        <div className="fs12 mt14" style={{ lineHeight: 1.8 }}>
          冷却中，预计 {lastPublishLabel(pack.availableAt)} 回可用。
        </div>
      )}
    </Modal>
  );
}

const TAG_MAX = 5;

function SkuEditModal({
  sku,
  onClose,
  onSaved,
}: {
  sku: SkuView | null;
  onClose: () => void;
  onSaved: () => void;
}) {
  const isNew = sku == null;
  const [code, setCode] = useState(sku?.code ?? "");
  const [name, setName] = useState(sku?.styleName ?? "");
  const [product, setProduct] = useState(sku?.productName ?? "");
  const [tier, setTier] = useState<Tier>((sku?.tier as Tier) || "warm");
  const [tags, setTags] = useState<string[]>(sku?.topics ?? []);
  const [newTag, setNewTag] = useState("");
  const [err, setErr] = useState<string | null>(null);

  const save = async () => {
    try {
      if (isNew) {
        await unwrap(
          commands.createSku({
            code: code.trim(),
            styleName: name.trim(),
            productName: product || null,
            tier,
            topics: tags,
            platforms: null,
            note: null,
          }),
        );
      } else {
        await unwrap(
          commands.updateSku(sku.id, {
            styleName: name,
            productName: product,
            tier,
            topics: tags,
            platforms: null,
            note: null,
          }),
        );
      }
      onSaved();
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    }
  };

  const addTag = () => {
    const t = newTag.trim().replace(/^#/, "");
    if (t && tags.length < TAG_MAX && !tags.includes(t)) setTags([...tags, t]);
    setNewTag("");
  };

  return (
    <Modal
      title={isNew ? "新建 SKU" : "编辑档案"}
      onClose={onClose}
      footer={
        <>
          <span className="fs11 t3">
            收件箱 TXT 带【话题】时：无标签则采纳前 5，已有则忽略并提示差异
          </span>
          <div className="f1" />
          <button type="button" className="btn" onClick={onClose}>
            取消
          </button>
          <button type="button" className="btn pri" onClick={() => void save()}>
            保存
          </button>
        </>
      }
    >
      <div className="col gap10">
        {err && <div className="terr">{err}</div>}
        <div className="col gap4">
          <span className="fs11 t3">SKU 编码（ASCII · 唯一）</span>
          <input
            className="inp mono"
            value={code}
            placeholder="SF-YD-201"
            disabled={!isNew}
            onChange={(e) => setCode(e.target.value)}
          />
        </div>
        <div className="col gap4">
          <span className="fs11 t3">款式名</span>
          <input className="inp" value={name} onChange={(e) => setName(e.target.value)} />
        </div>
        <div className="col gap4">
          <span className="fs11 t3">商品名</span>
          <input className="inp" value={product} onChange={(e) => setProduct(e.target.value)} />
        </div>
        <div className="col gap4">
          <span className="fs11 t3">冷热分层</span>
          <div className="seg">
            {(
              [
                ["hot", "热款"],
                ["warm", "温款"],
                ["cold", "冷款"],
              ] as [Tier, string][]
            ).map(([v, label]) => (
              <span key={v} className={cn("sgi", tier === v && "on")} onClick={() => setTier(v)}>
                {label}
              </span>
            ))}
          </div>
        </div>
        <div className="col gap4">
          <span className="fs11 t3">固定话题标签（有序 · 前 5 进任务单）</span>
          <div className="fx ac gap6 wrap">
            {tags.map((t) => (
              <span key={t} className="tagchip">
                #{t}
                <span className="rmx" onClick={() => setTags(tags.filter((x) => x !== t))}>
                  ×
                </span>
              </span>
            ))}
            {tags.length < TAG_MAX && (
              <input
                className="inp"
                style={{ width: 110, height: 22 }}
                value={newTag}
                placeholder="+ 标签"
                onChange={(e) => setNewTag(e.target.value)}
                onKeyDown={(e) => e.key === "Enter" && addTag()}
                onBlur={addTag}
              />
            )}
          </div>
        </div>
      </div>
    </Modal>
  );
}

function AddTextModal({
  skuId,
  kind,
  onClose,
  onSaved,
}: {
  skuId: number;
  kind: "title" | "body";
  onClose: () => void;
  onSaved: () => void;
}) {
  const [text, setText] = useState("");
  const [platform, setPlatform] = useState("general");
  const [platforms, setPlatforms] = useState<{ code: string; zh: string }[]>([]);
  useEffect(() => {
    void commands.publishPlatforms().then((r) => {
      if (r.status === "ok") setPlatforms(r.data);
    });
  }, []);
  const save = async () => {
    if (!text.trim()) return;
    await unwrap(commands.addTextItem({ skuId, kind, text: text.trim(), platform }));
    onSaved();
  };
  return (
    <Modal
      title={kind === "title" ? "新增标题" : "新增正文"}
      onClose={onClose}
      footer={
        <>
          <div className="f1" />
          <button type="button" className="btn" onClick={onClose}>
            取消
          </button>
          <button type="button" className="btn pri" onClick={() => void save()}>
            保存
          </button>
        </>
      }
    >
      <div className="col gap10">
        <div className="col gap4">
          <span className="fs11 t3">{kind === "title" ? "标题文本" : "正文文本"}</span>
          <textarea
            className="inp"
            rows={kind === "title" ? 2 : 5}
            value={text}
            onChange={(e) => setText(e.target.value)}
          />
        </div>
        <div className="col gap4">
          <span className="fs11 t3">平台标签</span>
          <select className="inp" value={platform} onChange={(e) => setPlatform(e.target.value)}>
            <option value="general">通用</option>
            {platforms.map((p) => (
              <option key={p.code} value={p.code}>
                {p.zh}
              </option>
            ))}
          </select>
        </div>
      </div>
    </Modal>
  );
}
