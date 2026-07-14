import { ConfirmModal, Modal } from "@/components/ui/Modal";
import { Toggle } from "@/components/ui/Stepper";
import { useDebouncedValue } from "@/features/_shared/useDebouncedValue";
import { assetSrc } from "@/lib/img";
import {
  type InboxItemView,
  type MappingImportReport,
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
import { useCallback, useEffect, useMemo, useState } from "react";
import { toast } from "sonner";

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
  const inboxRev = usePublishStore((s) => s.inboxRev);
  const [view, setView] = useState<"table" | "cards">("table");
  const [tier, setTier] = useState<Tier>("");
  const [warnOnly, setWarnOnly] = useState(false);
  const [showOff, setShowOff] = useState(false);
  const [query, setQuery] = useState("");
  // 打字不再每一键都发一次全量查询（D6）。
  const debouncedQuery = useDebouncedValue(query, 300);
  const [rows, setRows] = useState<SkuView[]>([]);
  const [editing, setEditing] = useState<SkuView | "new" | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [msg, setMsg] = useState<string | null>(null);
  const [preview, setPreview] = useState<{ path: string; report: MappingImportReport } | null>(
    null,
  );

  // 映射导入：先预检（不落库）弹窗给用户看清「将新建/更新/冲突」，确认后才写库。
  const pickMappings = async () => {
    try {
      const path = await unwrap(commands.pickMappingFile());
      if (!path) return;
      const report = await unwrap(commands.importSkuMappings(path, true));
      setPreview({ path, report });
      setErr(null);
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    }
  };

  const applyMappings = async (path: string) => {
    try {
      const r = await unwrap(commands.importSkuMappings(path, false));
      setPreview(null);
      const parts: string[] = [];
      if (r.created > 0) parts.push(`新建 ${r.created} 个 SKU`);
      if (r.updated > 0) parts.push(`更新 ${r.updated} 个`);
      if (r.unchanged > 0) parts.push(`无变化 ${r.unchanged} 个`);
      if (r.aliasSet > 0) parts.push(`别名 ${r.aliasSet}`);
      if (r.topicsSet > 0) parts.push(`话题 ${r.topicsSet}`);
      setMsg(`映射导入完成：${parts.join(" · ") || "无可导入行"}`);
      setErr(null);
      await load();
      await refreshBadges();
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    }
  };

  const saveTemplate = async () => {
    try {
      const p = await unwrap(commands.saveSkuMappingTemplate());
      if (p) setMsg(`模板已保存：${p}`);
      setErr(null);
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    }
  };

  const load = useCallback(async () => {
    try {
      const list = await unwrap(
        commands.listSkus({
          tier: tier || null,
          warnOnly: warnOnly || null,
          status: showOff ? "paused" : null,
          query: debouncedQuery || null,
        }),
      );
      setRows(list);
      setErr(null);
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    }
  }, [tier, warnOnly, showOff, debouncedQuery]);

  // 收件箱有新收录 → SKU 三池余量变了，列表跟着刷新（事件驱动，不轮询）。
  useEffect(() => {
    void load();
  }, [load, inboxRev]);

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
            <button
              type="button"
              className="btn sm gho"
              onClick={() => void saveTemplate()}
              title="导出带表头的空白映射表（Excel 可直接打开）"
            >
              模板
            </button>
            <button
              type="button"
              className="btn sm gho"
              onClick={() => void pickMappings()}
              title="批量建 SKU / 补别名话题（xlsx / csv / txt）"
            >
              导入映射
            </button>
            <button type="button" className="btn sm" onClick={() => setEditing("new")}>
              <Plus className="ic12" />
              新建 SKU
            </button>
          </div>

          {msg && (
            <div
              className="ban"
              style={{ margin: "0 18px", whiteSpace: "pre-wrap" }}
              onClick={() => setMsg(null)}
            >
              {msg}
            </div>
          )}

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

      {preview && (
        <MappingPreviewModal
          path={preview.path}
          report={preview.report}
          onClose={() => setPreview(null)}
          onConfirm={() => void applyMappings(preview.path)}
        />
      )}
    </div>
  );
}

/** 映射导入预检：确认前先把「将新建/更新/冲突」摊开给用户看。 */
function MappingPreviewModal({
  path,
  report,
  onClose,
  onConfirm,
}: {
  path: string;
  report: MappingImportReport;
  onClose: () => void;
  onConfirm: () => void;
}) {
  const { created, updated, unchanged, conflicts, errors } = report;
  const nothing = created === 0 && updated === 0;
  const file = path.split(/[/\\]/).pop() ?? path;

  return (
    <Modal
      title="映射导入预检"
      width="w640"
      onClose={onClose}
      footer={
        <>
          <button type="button" className="btn sm gho" onClick={onClose}>
            取消
          </button>
          <button type="button" className="btn sm" disabled={nothing} onClick={onConfirm}>
            {nothing ? "无可导入内容" : `确认导入（新建 ${created} · 更新 ${updated}）`}
          </button>
        </>
      }
    >
      <div className="col gap10">
        <div className="fs11 t3">
          {file} · {report.encoding} ·{" "}
          {report.hadHeader ? "已识别表头" : "无表头，按「编码, 别名, 话题」三列解析"}
        </div>
        <div className="statrow" style={{ padding: 0 }}>
          <Stat label="新建 SKU" value={created} color="var(--ok)" />
          <Stat label="更新" value={updated} color="var(--ok)" />
          <Stat label="无变化" value={unchanged} />
          <Stat
            label="冲突"
            value={conflicts.length}
            color={conflicts.length ? "var(--wr)" : undefined}
          />
          <Stat
            label="错误行"
            value={errors.length}
            color={errors.length ? "var(--er)" : undefined}
          />
        </div>
        {created > 0 && (
          <ReportList
            title={`将新建的 SKU（${created}）`}
            items={report.createdCodes}
            more={created - report.createdCodes.length}
          />
        )}
        {conflicts.length > 0 && (
          <ReportList
            title={`冲突：仅该格跳过，行内其余字段照常导入（${conflicts.length}）`}
            items={conflicts}
          />
        )}
        {errors.length > 0 && (
          <ReportList title={`错误：该行或该格不导入（${errors.length}）`} items={errors} />
        )}
        <div className="fs11 t3">留空的单元格不会覆盖库里已有的值。</div>
      </div>
    </Modal>
  );
}

function Stat({
  label,
  value,
  color,
}: { label: string; value: number; color?: string | undefined }) {
  return (
    <div className="statcard">
      <div className="stnum" style={color ? { color } : undefined}>
        {value}
      </div>
      <div className="stlbl">{label}</div>
    </div>
  );
}

function ReportList({ title, items, more = 0 }: { title: string; items: string[]; more?: number }) {
  return (
    <details>
      <summary className="fs12 fw6" style={{ cursor: "pointer" }}>
        {title}
      </summary>
      <div
        className="fs11 t3"
        style={{ maxHeight: 180, overflow: "auto", paddingTop: 6, lineHeight: 1.7 }}
      >
        {items.map((t) => (
          <div key={t}>{t}</div>
        ))}
        {more > 0 && <div>…另有 {more} 个</div>}
      </div>
    </details>
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

/**
 * 日期标签。跨年时补上年份——「12月3日」在 7 月看到，到底是去年的还是今年的，
 * 光看月日说不清（冷却中的包尤其要紧，那关系到它什么时候回可用）。
 */
function lastPublishLabel(ts: number | null): string {
  if (!ts) return "—";
  const d = new Date(ts * 1000);
  const now = new Date();
  const sameYear = d.getFullYear() === now.getFullYear();
  const md = `${d.getMonth() + 1}月${d.getDate()}日`;
  return sameYear ? md : `${d.getFullYear()}年${md}`;
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

/** inbox_items.kind → 中文标签（媒体条目以文件夹为单位）。 */
function inboxKindLabel(kind: string): string {
  if (kind === "media") return "图片/视频";
  if (kind === "title") return "标题";
  if (kind === "body") return "正文";
  return kind;
}

function InboxPanel({ onChanged }: { onChanged: () => void }) {
  const [claims, setClaims] = useState<InboxItemView[]>([]);
  const [fails, setFails] = useState<InboxItemView[]>([]);
  const [claimTarget, setClaimTarget] = useState<InboxItemView | null>(null);
  const inboxRev = usePublishStore((s) => s.inboxRev);

  const load = useCallback(async () => {
    setClaims(await unwrap(commands.listInboxItems("unclaimed")));
    setFails(await unwrap(commands.listInboxItems("failed")));
  }, []);
  // watcher 收录（2s 防抖后）→ 面板自动跟着更新。
  useEffect(() => {
    void load();
  }, [load, inboxRev]);

  /** 操作一律带错误处理：失败静默是最难查的那种 bug（用户以为点了没反应）。 */
  const act = async (fn: () => Promise<unknown>, ok?: string) => {
    try {
      await fn();
      if (ok) toast.success(ok);
      await load();
      onChanged();
    } catch (e) {
      toast.error(String(e));
    }
  };

  const rescan = () =>
    act(async () => {
      const r = await unwrap(commands.rescanInbox());
      toast.success(`重扫完成：入库 ${r.ingested} · 待认领 ${r.unclaimed} · 失败 ${r.failed}`);
    });
  const discard = (id: number) =>
    act(() => unwrap(commands.discardInboxItem(id)), "已丢弃（文件移入 收件箱/已丢弃/）");
  const retry = (id: number) => act(() => unwrap(commands.retryInboxItem(id)));

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
            <span className="pcap">
              无法关联到已知 SKU 的收件箱内容（TXT 按文件、图片/视频按文件夹）
            </span>
          </div>
          {claims.map((c) => (
            <div
              key={c.id}
              className="txrow"
              style={{ borderTop: "1px solid var(--line)", borderBottom: "none" }}
            >
              <span className="chip">{c.fileName}</span>
              {c.kind && <span className="bdg b-gray">{inboxKindLabel(c.kind)}</span>}
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
          成功收录的原文件移入 <span className="chip">收件箱/已收录/</span> 归档， 丢弃的移入{" "}
          <span className="chip">收件箱/已丢弃/</span>（文件仍在，只是不再收录）。
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
  const [q, setQ] = useState("");
  useEffect(() => {
    void unwrap(commands.listSkus({ tier: null, warnOnly: null, status: null, query: null })).then(
      setSkus,
    );
  }, []);
  const pick = async (code: string) => {
    try {
      await unwrap(commands.claimInboxItem(item.id, code));
      onDone();
    } catch (e) {
      toast.error(String(e));
    }
  };
  // 100+ SKU 时靠肉眼在长列表里找是不可能的（D6）。
  const filtered = useMemo(() => {
    const key = q.trim().toLowerCase();
    return skus
      .filter((s) => !s.isGeneral)
      .filter(
        (s) =>
          !key ||
          s.code.toLowerCase().includes(key) ||
          s.styleName.toLowerCase().includes(key) ||
          s.productName.toLowerCase().includes(key),
      );
  }, [skus, q]);

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
        <input
          className="inp"
          style={{ width: "100%", marginBottom: 8 }}
          placeholder="搜索编码 / 款式名 / 商品名…"
          value={q}
          onChange={(e) => setQ(e.target.value)}
        />
        {filtered.map((s) => {
          const t = tierVisual(s.tier);
          return (
            <div key={s.id} className="pickrow" onClick={() => void pick(s.code)}>
              <span className="pid">{s.code}</span>
              <span className="fw5 fs12 f1 nowrap ohide">{s.styleName}</span>
              {/* 三池余量：指认前先看一眼这个 SKU 缺不缺料 */}
              <span className="fs10 t3 nowrap">
                素材 {s.materialCount} · 标题 {s.titleCount} · 正文 {s.bodyCount}
              </span>
              <span className={cn("bdg", t.badgeClass)}>{t.label}</span>
            </div>
          );
        })}
        {filtered.length === 0 && <div className="fs12 t3 pa8">没有匹配的 SKU</div>}
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
        const thumb = p.thumbPath ? assetSrc(p.thumbPath) : null;
        return (
          <div key={p.id} className={cn("packc", dim && "dim")} onClick={() => onOpen(p)}>
            <div
              className="packimg"
              style={
                thumb
                  ? {
                      backgroundImage: `url("${thumb}")`,
                      backgroundSize: "cover",
                      backgroundPosition: "center",
                    }
                  : { background: "var(--inset)" }
              }
            >
              <span className="packn">{p.kind === "video" ? "视频" : `图 ${p.fileCount}`}</span>
              {!thumb && <span className="phl t3 fs11">{p.kind === "video" ? "▶" : "🖼"}</span>}
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
  const [editing, setEditing] = useState<TextItemView | null>(null);
  const [deleting, setDeleting] = useState<TextItemView | null>(null);

  const toggle = async (id: number, enabled: boolean) => {
    try {
      await unwrap(commands.setTextItemEnabled(id, enabled));
      await onChanged();
    } catch (e) {
      toast.error(String(e));
    }
  };
  const del = async (id: number) => {
    try {
      await unwrap(commands.deleteTextItem(id));
      toast.success("已删除");
      setDeleting(null);
      await onChanged();
    } catch (e) {
      toast.error(String(e));
      setDeleting(null);
    }
  };

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
          {/* 来源：收件箱自动收录 vs 手动新增 —— 排查「这条哪来的」时很有用 */}
          <span className="chip">{x.source === "inbox" ? "收件箱" : "手动"}</span>
          {!x.enabled && <span className="chip">已停用</span>}
          <span className="fs11 t3 nowrap" style={{ width: 56, textAlign: "right" }}>
            用过 {x.useCount} 次
          </span>
          <span className="tract">
            <button type="button" className="btn sm gho" onClick={() => setEditing(x)}>
              编辑
            </button>
            <button
              type="button"
              className="btn sm gho"
              onClick={() => void toggle(x.id, !x.enabled)}
            >
              {x.enabled ? "停用" : "启用"}
            </button>
            <button type="button" className="btn sm gho dng" onClick={() => setDeleting(x)}>
              删
            </button>
          </span>
        </div>
      ))}

      {editing && (
        <EditTextModal
          item={editing}
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null);
            void onChanged();
          }}
        />
      )}
      {deleting && (
        <ConfirmModal
          title="删除文本条目"
          desc={
            deleting.useCount > 0
              ? `这条已被用过 ${deleting.useCount} 次。被任务单引用的条目删不掉（会让历史记录失去内容）；那种情况请改用「停用」。`
              : "删除后不可恢复。"
          }
          confirmLabel="确认删除"
          onConfirm={() => void del(deleting.id)}
          onClose={() => setDeleting(null)}
        />
      )}
    </>
  );
}

/** 文本条目编辑：正文/标题内容 + 平台标签。 */
function EditTextModal({
  item,
  onClose,
  onSaved,
}: {
  item: TextItemView;
  onClose: () => void;
  onSaved: () => void;
}) {
  const [text, setText] = useState(item.text);
  const [platform, setPlatform] = useState(item.platform);
  const [platforms, setPlatforms] = useState<{ code: string; zh: string }[]>([]);
  useEffect(() => {
    void commands.publishPlatforms().then((r) => {
      if (r.status === "ok") setPlatforms(r.data);
    });
  }, []);

  const save = async () => {
    if (!text.trim()) return;
    try {
      await unwrap(commands.updateTextItem(item.id, { text: text.trim(), platform }));
      onSaved();
    } catch (e) {
      toast.error(String(e));
    }
  };

  return (
    <Modal
      title={item.kind === "title" ? "编辑标题" : "编辑正文"}
      onClose={onClose}
      footer={
        <>
          <span className="fs11 t3">平台标签决定它能发到哪些平台（通用 = 全平台可用）</span>
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
      <div className="col gap10">
        <textarea
          className="inp"
          rows={item.kind === "title" ? 2 : 6}
          value={text}
          onChange={(e) => setText(e.target.value)}
        />
        <div className="col gap4">
          <span className="fs11 t3">平台标签</span>
          <select className="inp" value={platform} onChange={(e) => setPlatform(e.target.value)}>
            <option value="general">通用（全平台）</option>
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

/** 同目录下的兄弟文件绝对路径（后端只给了缩略图一条绝对路径，其余成员据此拼）。 */
function siblingPath(abs: string, name: string): string {
  const i = Math.max(abs.lastIndexOf("/"), abs.lastIndexOf("\\"));
  return i < 0 ? name : abs.slice(0, i + 1) + name;
}

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
  const [note, setNote] = useState(pack.note);
  const [cover, setCover] = useState(pack.cover);
  const [saving, setSaving] = useState(false);

  const retire = async () => {
    await unwrap(commands.retirePack(pack.id));
    onChanged();
  };
  const restore = async () => {
    await unwrap(commands.restorePack(pack.id));
    onChanged();
  };
  const activate = async () => {
    await unwrap(commands.activatePack(pack.id));
    onChanged();
  };
  const save = async () => {
    setSaving(true);
    try {
      await unwrap(commands.updatePack(pack.id, { note, cover }));
      onChanged();
    } finally {
      setSaving(false);
    }
  };

  const canRetire = pack.derived !== "retired" && !pack.locked;
  const dirty = note !== pack.note || cover !== pack.cover;
  // 图片成员可当封面；视频文件不行。
  const coverChoices = pack.files.filter((f) => /\.(jpe?g|png|webp)$/i.test(f.name));
  const imgFiles = coverChoices;
  const thumbPath = pack.thumbPath;

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
          <span className="fs11 t3">入库即可用 → 已用尽（窗口期满自动回可用）· 退役为人工终态</span>
          <div className="f1" />
          {pack.derived === "new" && (
            <button type="button" className="btn sm pri" onClick={() => void activate()}>
              标为可用
            </button>
          )}
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
          <button
            type="button"
            className={cn("btn sm", dirty && "pri")}
            disabled={!dirty || saving}
            onClick={() => void save()}
          >
            保存
          </button>
          <button type="button" className="btn sm" onClick={onClose}>
            关闭
          </button>
        </>
      }
    >
      {thumbPath && imgFiles.length > 0 && (
        <div
          className="mt6"
          style={{
            display: "grid",
            gridTemplateColumns: "repeat(auto-fill, minmax(84px, 1fr))",
            gap: 6,
          }}
        >
          {imgFiles.map((f) => (
            <img
              key={f.name}
              src={assetSrc(siblingPath(thumbPath, f.name))}
              alt={f.name}
              loading="lazy"
              title={f.name}
              style={{
                width: "100%",
                aspectRatio: "1",
                objectFit: "cover",
                borderRadius: 6,
                border: f.name === cover ? "2px solid var(--ac)" : "1px solid var(--line)",
              }}
            />
          ))}
        </div>
      )}

      <div className="fs11 fw6 t3 mt14" style={{ letterSpacing: ".05em" }}>
        包内文件 · ASCII 规范化命名
      </div>
      <div className="mt6 col gap4">
        {pack.files.map((f) => (
          <span key={f.name} className="chip" style={{ alignSelf: "flex-start" }}>
            {f.name}
            {f.origName && f.origName !== f.name ? ` ← ${f.origName}` : ""}
            {f.name === cover && " · 封面"}
          </span>
        ))}
      </div>

      <div className="fs11 fw6 t3 mt14" style={{ letterSpacing: ".05em" }}>
        封面
      </div>
      <div className="fx ac gap6 mt6" style={{ flexWrap: "wrap" }}>
        <button
          type="button"
          className={cn("btn sm", cover == null ? "pri" : "gho")}
          onClick={() => setCover(null)}
        >
          无封面
        </button>
        {coverChoices.map((f) => (
          <button
            key={f.name}
            type="button"
            className={cn("btn sm", cover === f.name ? "pri" : "gho")}
            onClick={() => setCover(f.name)}
          >
            {f.name}
          </button>
        ))}
        {coverChoices.length === 0 && <span className="fs11 t3">包内没有可作封面的图片</span>}
      </div>

      <div className="fs11 fw6 t3 mt14" style={{ letterSpacing: ".05em" }}>
        备注
      </div>
      <textarea
        className="inp mt6"
        rows={2}
        value={note}
        placeholder="给这个素材包留一句话（如「客户指定首图」「审核不过，勿再用」）"
        onChange={(e) => setNote(e.target.value)}
      />

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
  const [alias, setAlias] = useState(sku?.folderAlias ?? "");
  const [newTag, setNewTag] = useState("");
  const [note, setNote] = useState(sku?.note ?? "");
  // null = 跟随全局矩阵；数组 = 该 SKU 只发这些平台。
  const [override, setOverride] = useState<string[] | null>(sku?.platforms ?? null);
  const [allPlatforms, setAllPlatforms] = useState<{ code: string; zh: string }[]>([]);
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    void commands.publishPlatforms().then((r) => {
      if (r.status === "ok") setAllPlatforms(r.data);
    });
  }, []);

  const togglePlatform = (code: string) => {
    const cur = override ?? [];
    setOverride(cur.includes(code) ? cur.filter((c) => c !== code) : [...cur, code]);
  };

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
            platforms: override,
            note: note || null,
            folderAlias: alias.trim() || null,
          }),
        );
      } else {
        await unwrap(
          commands.updateSku(sku.id, {
            styleName: name,
            productName: product,
            tier,
            topics: tags,
            // Some(None) = 清除覆盖 → 跟随全局；Some(Some) = 设置覆盖。
            platforms: override,
            note,
            folderAlias: alias.trim(),
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
          <span className="fs11 t3">收件箱文件夹别名（可选 · 中文亦可 · 唯一）</span>
          <input
            className="inp"
            value={alias}
            placeholder="A-敖瑞鹏-01"
            onChange={(e) => setAlias(e.target.value)}
          />
          <span className="fs10 t3">
            收件箱子文件夹用此名时自动归到本 SKU（如 A-敖瑞鹏-01 → {code || "本 SKU"}）
          </span>
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

        <div className="col gap4">
          <div className="fx ac gap8">
            <span className="fs11 t3 f1">发布平台</span>
            <span className="fs11 t3">跟随全局矩阵</span>
            <Toggle
              on={override == null}
              onClick={() => setOverride(override == null ? [] : null)}
            />
          </div>
          {override != null && (
            <div className="fx ac gap10 wrap mt6">
              {allPlatforms.map((p) => (
                <div key={p.code} className="fx ac gap6">
                  <Toggle on={override.includes(p.code)} onClick={() => togglePlatform(p.code)} />
                  <span className="fs12">{p.zh}</span>
                </div>
              ))}
              {override.length === 0 && (
                <span className="fs11 terr">一个平台都没选 → 该 SKU 不会被排期</span>
              )}
            </div>
          )}
        </div>

        <div className="col gap4">
          <span className="fs11 t3">备注</span>
          <textarea
            className="inp"
            rows={2}
            value={note}
            placeholder="只在档案卡展示，不进任务单"
            onChange={(e) => setNote(e.target.value)}
          />
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
