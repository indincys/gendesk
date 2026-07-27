import { Modal } from "@/components/ui/Modal";
import { V2vVideo } from "@/features/v2v/V2vVideo";
import { type Row, deriveRows, fmtAgo, shortModel } from "@/features/v2v/model";
import { assetSrc } from "@/lib/img";
import {
  type ClipView,
  type EffectiveParams,
  type ModelInfo,
  type SkuView,
  commands,
  subscribeV2v,
  unwrap,
} from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { useUiStore } from "@/stores/ui";
import { Clapperboard, FolderOpen, Layers, Search } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { toast } from "sonner";

/**
 * 视频成片库 —— 验收通过的视频总览。
 *
 * ## 为什么它必须是独立的一页
 *
 * 因为验收通过的片子**已经不是流水线的事了**。它们混在工作台里的时候，18 条成片和
 * 3 条待办长得一样重，「这里还剩多少活」这个问题得靠人在心里做减法才答得出。
 * 拆开之后两边各自都变简单：工作台只剩在制的，这一页只回答成片自己的问题 ——
 * **哪些还没进资产库**（发布链在这里断掉且毫无声响）、这条花了多少、片子在哪。
 *
 * 「未通过」也在这里，作为一档筛选：那些条目不该变得无处可寻，
 * 只是它们的片子已经进了废纸篓，这里留的是记录而不是画面。
 */
type Tab = "pass" | "rej";
type Lib = "all" | "in" | "out";

export function V2vClipsPage() {
  const go = useUiStore((s) => s.go);
  const [clips, setClips] = useState<ClipView[]>([]);
  const [models, setModels] = useState<ModelInfo[]>([]);
  const [eff, setEff] = useState<EffectiveParams | null>(null);
  const [tab, setTab] = useState<Tab>("pass");
  const [lib, setLib] = useState<Lib>("all");
  const [group, setGroup] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [sel, setSel] = useState<Set<number>>(new Set());
  const [cur, setCur] = useState<number | null>(null);
  const [assetPick, setAssetPick] = useState<number[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [now, setNow] = useState(() => Math.floor(Date.now() / 1000));

  const load = useCallback(async () => {
    try {
      setClips(await unwrap(commands.listV2vClips(["pass", "rej"])));
    } catch (e) {
      if (e instanceof Error) toast.error(e.message);
    }
  }, []);

  useEffect(() => {
    void load();
    void unwrap(commands.v2vModels())
      .then(setModels)
      .catch(() => setModels([]));
    void unwrap(commands.v2vEffectiveParams())
      .then(setEff)
      .catch(() => {});
  }, [load]);

  useEffect(() => {
    let un: (() => void) | undefined;
    void subscribeV2v({ onChanged: () => void load() }).then((f) => {
      un = f;
    });
    return () => un?.();
  }, [load]);

  useEffect(() => {
    const t = setInterval(() => setNow(Math.floor(Date.now() / 1000)), 30_000);
    return () => clearInterval(t);
  }, []);

  const rows = useMemo(() => deriveRows(clips, models, eff, now), [clips, models, eff, now]);

  const groups = useMemo(() => {
    const m = new Map<string, number>();
    for (const r of rows) {
      if (r.stage !== tab) continue;
      const k = r.clip.groupName || "未分组";
      m.set(k, (m.get(k) ?? 0) + 1);
    }
    return [...m.entries()].sort((a, b) => b[1] - a[1]);
  }, [rows, tab]);

  const visible = useMemo(() => {
    const q = query.trim().toLowerCase();
    return rows.filter((r) => {
      if (r.stage !== tab) return false;
      if (group != null && (r.clip.groupName || "未分组") !== group) return false;
      if (lib === "in" && !r.clip.inAssetLib) return false;
      if (lib === "out" && r.clip.inAssetLib) return false;
      if (q === "") return true;
      return (
        r.clip.promptCode.toLowerCase().includes(q) ||
        r.clip.groupName.toLowerCase().includes(q) ||
        (r.clip.videoPrompt ?? "").toLowerCase().includes(q) ||
        r.clip.sourcePrompt.toLowerCase().includes(q)
      );
    });
  }, [rows, tab, group, lib, query]);

  const passRows = useMemo(() => rows.filter((r) => r.stage === "pass"), [rows]);
  const notInLib = passRows.filter((r) => !r.clip.inAssetLib).length;
  const spent = passRows.reduce((n, r) => n + (r.clip.creditCount ?? 0), 0);

  const curRow =
    (cur == null ? null : visible.find((r) => r.clip.id === cur)) ?? visible[0] ?? null;

  const packInto = useCallback(
    async (ids: number[], skuId: number) => {
      setBusy(true);
      try {
        let ok = 0;
        for (const id of ids) {
          if (await unwrap(commands.packFromClip(skuId, id))) ok += 1;
        }
        setAssetPick(null);
        setSel(new Set());
        if (ok > 0) toast.success(`已入资产库 ${ok} 个视频素材包`);
        else toast("没有可打包的条目（只有验收通过且有成片文件的才行）");
        await load();
      } catch (e) {
        toast.error(e instanceof Error ? e.message : String(e));
      } finally {
        setBusy(false);
      }
    },
    [load],
  );

  const packable = useMemo(
    () =>
      [...sel].filter((id) => {
        const r = rows.find((x) => x.clip.id === id);
        return r?.stage === "pass" && !r.clip.inAssetLib;
      }),
    [sel, rows],
  );

  return (
    <div className="col f1 ohide">
      <div className="vhd">
        <span className="ptt">视频成片</span>
        <span className="fs11 t3 nowrap">
          成片 <b className="mono t1">{passRows.length}</b> · 未入资产库{" "}
          <b className="mono" style={{ color: notInLib > 0 ? "var(--wr2)" : "var(--t1)" }}>
            {notInLib}
          </b>{" "}
          · 累计扣费 <b className="mono t1">{spent}</b>
        </span>
        <div className="f1" />
        <button type="button" className="btn xs gho" onClick={() => go("v2v")}>
          <Clapperboard className="ic12" />
          回流水线
        </button>
      </div>

      <div className="vfilt">
        <span className="fx ac gap6" style={{ position: "relative" }}>
          <Search
            className="ic12 t3"
            style={{ position: "absolute", left: 7, pointerEvents: "none" }}
          />
          <input
            className="inp sm"
            style={{ width: 190, paddingLeft: 24 }}
            placeholder="编号 / 组 / 提示词…"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
        </span>
        <button
          type="button"
          className={cn("vchip", tab === "pass" && "on")}
          onClick={() => {
            setTab("pass");
            setSel(new Set());
          }}
        >
          成片<span className="n">{rows.filter((r) => r.stage === "pass").length}</span>
        </button>
        <button
          type="button"
          className={cn("vchip", tab === "rej" && "on")}
          onClick={() => {
            setTab("rej");
            setSel(new Set());
            setLib("all");
          }}
          title="验收未通过的记录。成片本体已进废纸篓，这里留的是账与提示词。"
        >
          未通过<span className="n">{rows.filter((r) => r.stage === "rej").length}</span>
        </button>
        {tab === "pass" && (
          <>
            <span className="lb" style={{ marginLeft: 6 }}>
              资产库
            </span>
            {(
              [
                ["all", "全部"],
                ["out", "未入库"],
                ["in", "已入库"],
              ] as [Lib, string][]
            ).map(([k, label]) => (
              <button
                key={k}
                type="button"
                className={cn("vchip", lib === k && "on")}
                onClick={() => setLib(k)}
              >
                {label}
              </button>
            ))}
          </>
        )}
        <div className="f1" />
        <select
          className="inp sm"
          style={{ maxWidth: 200 }}
          value={group ?? ""}
          onChange={(e) => setGroup(e.target.value === "" ? null : e.target.value)}
        >
          <option value="">全部分组（{groups.length}）</option>
          {groups.map(([g, n]) => (
            <option key={g} value={g}>
              {g}（{n}）
            </option>
          ))}
        </select>
      </div>

      <div className="f1 fx" style={{ minHeight: 0 }}>
        <div className="sc f1" style={{ minWidth: 0, padding: "10px 12px" }}>
          {visible.length === 0 ? (
            <div className="bigempty">
              <Clapperboard className="ic" style={{ width: 26, height: 26, opacity: 0.5 }} />
              <div className="fs13 fw5 t2">
                {tab === "pass" ? "还没有验收通过的视频" : "没有未通过的记录"}
              </div>
              <div className="fs12 t3" style={{ maxWidth: 420, lineHeight: 1.7 }}>
                验收通过的视频会自动来到这一页；流水线工作台只留还在制的条目。
              </div>
            </div>
          ) : (
            <div className="clipgrid">
              {visible.map((r) => (
                <ClipCard
                  key={r.clip.id}
                  r={r}
                  cur={r.clip.id === curRow?.clip.id}
                  checked={sel.has(r.clip.id)}
                  now={now}
                  onPick={() => setCur(r.clip.id)}
                  onCheck={() =>
                    setSel((old) => {
                      const n = new Set(old);
                      if (n.has(r.clip.id)) n.delete(r.clip.id);
                      else n.add(r.clip.id);
                      return n;
                    })
                  }
                />
              ))}
            </div>
          )}
        </div>

        {curRow && (
          <ClipSide row={curRow} busy={busy} onPack={() => setAssetPick([curRow.clip.id])} />
        )}
      </div>

      <div className="vfoot">
        {sel.size > 0 ? (
          <>
            <button type="button" className="btn xs" onClick={() => setSel(new Set())}>
              已选 <b>{sel.size}</b> ✕
            </button>
            <button
              type="button"
              className="btn xs pri"
              disabled={busy || packable.length === 0}
              onClick={() => setAssetPick(packable)}
              title="打包成视频型素材包（1 视频 + 封面）接上发布链"
            >
              <Layers className="ic12" />
              入资产库 {packable.length} 条
            </button>
            {packable.length < sel.size && (
              <span className="fs11 t3">{sel.size - packable.length} 条已在库或无成片</span>
            )}
          </>
        ) : (
          <span className="fs11 t3 nowrap ohide">
            {visible.length} 条 ·{" "}
            {tab === "pass"
              ? "勾选后可批量入资产库；入库之后发布计划才排得到它"
              : "未通过的成片已进废纸篓，这里留的是账与提示词"}
          </span>
        )}
        <div className="f1" />
        <button
          type="button"
          className="btn xs gho"
          onClick={() => void unwrap(commands.openDataDir()).catch((e) => toast.error(String(e)))}
        >
          <FolderOpen className="ic12" />
          打开数据目录
        </button>
      </div>

      {assetPick && (
        <SkuPick
          count={assetPick.length}
          onClose={() => setAssetPick(null)}
          onPick={(skuId) => void packInto(assetPick, skuId)}
        />
      )}
    </div>
  );
}

function ClipCard({
  r,
  cur,
  checked,
  now,
  onPick,
  onCheck,
}: {
  r: Row;
  cur: boolean;
  checked: boolean;
  now: number;
  onPick: () => void;
  onCheck: () => void;
}) {
  const c = r.clip;
  const thumb = assetSrc(c.posterPath ?? c.thumbPath);
  return (
    <div
      className={cn("clipcard", cur && "cur", checked && "sel")}
      onClick={onPick}
      onKeyDown={(e) => e.key === "Enter" && onPick()}
      role="button"
      tabIndex={-1}
    >
      <div className="th">
        {thumb ? <img src={thumb} alt="" /> : <span className="fs10 t3">无封面</span>}
        <span
          className={cn("vbox", checked && "on")}
          onClick={(e) => {
            e.stopPropagation();
            onCheck();
          }}
          onKeyDown={(e) => {
            if (e.key === " ") {
              e.stopPropagation();
              onCheck();
            }
          }}
          role="checkbox"
          aria-checked={checked}
          aria-label="选中"
          tabIndex={-1}
        >
          {checked ? "✓" : ""}
        </span>
        {r.stage === "pass" && !c.inAssetLib && <span className="tag wr">未入库</span>}
        {c.inAssetLib && <span className="tag ok">已入库</span>}
      </div>
      <div className="fx ac gap5" style={{ marginTop: 5 }}>
        <span className="pid">{c.promptCode}</span>
        <span className="f1" />
        <span className="fs10 mono t3">{c.creditCount == null ? "—" : `${c.creditCount}`}</span>
      </div>
      <div className="fs10 t3 nowrap ohide">
        {c.groupName || "未分组"}
        {c.reviewedAt != null && ` · ${fmtAgo(now - c.reviewedAt)}`}
      </div>
    </div>
  );
}

/** 右栏：选中那条的画面与账。与流水线详情栏同形，但只回答成片自己的问题。 */
function ClipSide({ row, busy, onPack }: { row: Row; busy: boolean; onPack: () => void }) {
  const c = row.clip;
  const video = assetSrc(c.videoPath);
  return (
    <div className="vinsp sc">
      <div className="fx ac gap6">
        <span className="pid">{c.promptCode}</span>
        <span className="f1" />
        <span className="fs10 t3 nowrap ohide">
          {c.batchId == null ? "无批次" : `#${c.batchId}`} · {c.groupName || "未分组"}
        </span>
      </div>
      {video ? (
        <V2vVideo className="mt8" src={video} fps={c.fps} portrait videoKey={c.id} />
      ) : (
        <div className="vstage pt mt8">
          <span className="vstagenote">
            {row.stage === "rej" ? "成片已进废纸篓" : "成片文件不在了"}
          </span>
        </div>
      )}
      {row.stage === "pass" && (
        <button
          type="button"
          className="btn sm pri mt8"
          style={{ width: "100%" }}
          disabled={busy || c.inAssetLib}
          onClick={onPack}
        >
          <Layers className="ic12" />
          {c.inAssetLib ? "已入资产库" : "入资产库"}
        </button>
      )}

      <div className="vsec">这一条的账</div>
      <div className="vfacts mt5">
        <Fact k="实际扣费" v={c.creditCount == null ? "—" : `${c.creditCount} 额度`} />
        <Fact k="计费型号" v={c.benefitType ?? "—"} />
        <Fact k="我们发的" v={row.modelFull == null ? "CLI 默认" : shortModel(row.modelFull)} />
        <Fact
          k="规格"
          v={
            c.width == null
              ? "—"
              : `${c.width}×${c.height}${c.durationSec != null ? ` · ${c.durationSec.toFixed(1)}s` : ""}`
          }
        />
        <Fact k="尝试" v={`第 ${Math.max(1, c.attempt)} 次`} />
        <Fact k="放行" v={c.autoSubmitted ? "常驻队列" : "手动"} />
      </div>

      <div className="vsec">视频提示词</div>
      <div className="vprompt mt4">{c.videoPrompt ?? "（无）"}</div>
      <div className="vsec">生图提示词</div>
      <div className="vprompt mt4">{c.sourcePrompt}</div>
      <div style={{ height: 8 }} />
    </div>
  );
}

function Fact({ k, v }: { k: string; v: string }) {
  return (
    <div className="vfact">
      <div className="k">{k}</div>
      <div className="v" title={v}>
        {v}
      </div>
    </div>
  );
}

function SkuPick({
  count,
  onClose,
  onPick,
}: {
  count: number;
  onClose: () => void;
  onPick: (skuId: number) => void;
}) {
  const [skus, setSkus] = useState<SkuView[]>([]);
  useEffect(() => {
    void unwrap(commands.listSkus({ tier: null, warnOnly: null, status: null, query: null }))
      .then(setSkus)
      .catch(() => setSkus([]));
  }, []);
  return (
    <Modal
      title="入资产库 · 选择目标 SKU"
      onClose={onClose}
      headerExtra={<span className="chip">{count} 条成片</span>}
      footer={
        <>
          <span className="fs11 t3">每条成片建一个视频型素材包（视频 + 封面），原成片保留</span>
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
          .map((s) => (
            <div
              key={s.id}
              className="pickrow"
              onClick={() => onPick(s.id)}
              onKeyDown={(e) => e.key === "Enter" && onPick(s.id)}
              role="button"
              tabIndex={0}
            >
              <span className="pid">{s.code}</span>
              <span className="fw5 fs12 f1 nowrap ohide">{s.styleName}</span>
            </div>
          ))}
        {skus.length === 0 && (
          <div className="fs12 t3" style={{ padding: 12 }}>
            尚无 SKU，请先在资产库创建
          </div>
        )}
      </div>
    </Modal>
  );
}
