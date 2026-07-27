import { V2vVideo } from "@/features/v2v/V2vVideo";
import { type Row, delivered, deriveRows, fmtAgo, shortModel } from "@/features/v2v/model";
import { assetSrc } from "@/lib/img";
import {
  type ClipView,
  type EffectiveParams,
  type ModelInfo,
  commands,
  subscribeV2v,
  unwrap,
} from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { useUiStore } from "@/stores/ui";
import { Clapperboard, FolderOpen, RefreshCw, Search } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { toast } from "sonner";

/**
 * 视频成片 —— **交付台账**。
 *
 * ## 为什么它必须是独立的一页
 *
 * 因为验收通过的片子**已经不是流水线的事了**。它们混在工作台里的时候，18 条成片和
 * 3 条待办长得一样重，「这里还剩多少活」这个问题得靠人在心里做减法才答得出。
 *
 * ## 它现在回答的是交付，不是入库（v0.22.0）
 *
 * 成片全是 B-roll 素材，不适合直接发布 —— 「入资产库」那条路径从一开始就不该存在，
 * 已整个拆掉。这一页现在只回答四个问题：
 *
 * 1. **片子在哪**：`exportPath`，可在文件管理器里直接打开那个目录。
 * 2. **交付成功没有**：验收时的拷贝失败**不回滚验收**（判定是人做的，文件可以补），
 *    于是「通过了却没落地」是个完全合法、在此之前又完全看不见的状态。
 *    它是这条链上唯一一处会无声断掉的地方，故也是侧栏徽章，且给「重新交付」。
 * 3. **花了多少额度**。
 * 4. **哪些没通过**：作为一档筛选 —— 那些条目不该变得无处可寻，
 *    只是它们的片子已经进了废纸篓，这里留的是账而不是画面。
 */
type Tab = "pass" | "rej";
/** 交付筛选轴（取代原来的「资产库」轴）。 */
type Deliv = "all" | "missing";

export function V2vClipsPage() {
  const go = useUiStore((s) => s.go);
  const [clips, setClips] = useState<ClipView[]>([]);
  const [models, setModels] = useState<ModelInfo[]>([]);
  const [eff, setEff] = useState<EffectiveParams | null>(null);
  const [tab, setTab] = useState<Tab>("pass");
  const [deliv, setDeliv] = useState<Deliv>("all");
  const [outDir, setOutDir] = useState<string>("");
  const [group, setGroup] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [sel, setSel] = useState<Set<number>>(new Set());
  const [cur, setCur] = useState<number | null>(null);
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
    // 交付目录直接摆在页头：「片子在哪」不该靠猜，而它是用户可改的。
    void unwrap(commands.v2vClipsDir())
      .then(setOutDir)
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
      if (deliv === "missing" && delivered(r.clip)) return false;
      if (q === "") return true;
      return (
        r.clip.promptCode.toLowerCase().includes(q) ||
        r.clip.groupName.toLowerCase().includes(q) ||
        (r.clip.videoPrompt ?? "").toLowerCase().includes(q) ||
        r.clip.sourcePrompt.toLowerCase().includes(q)
      );
    });
  }, [rows, tab, group, deliv, query]);

  const passRows = useMemo(() => rows.filter((r) => r.stage === "pass"), [rows]);
  const undelivered = passRows.filter((r) => !delivered(r.clip)).length;
  const spent = passRows.reduce((n, r) => n + (r.clip.creditCount ?? 0), 0);

  const curRow =
    (cur == null ? null : visible.find((r) => r.clip.id === cur)) ?? visible[0] ?? null;

  /**
   * 重新交付：把成片再拷一次到当前交付目录。
   *
   * 之所以补得出来：`clips/clip{id}.mp4` 那份是流水线自己的资产，从来只拷不移。
   * 三种情况都走它 —— 验收那一刻拷贝失败、交付目录被改到别处、人手动删了那份。
   */
  const redeliver = useCallback(
    async (ids: number[]) => {
      if (ids.length === 0) return;
      setBusy(true);
      try {
        const n = await unwrap(commands.redeliverV2vClips(ids));
        if (n > 0) toast.success(`已重新交付 ${n} 条到输出目录`);
        else toast("没有可交付的条目（这些条目本身就没有成片文件）");
        setSel(new Set());
        await load();
      } catch (e) {
        toast.error(e instanceof Error ? e.message : String(e));
      } finally {
        setBusy(false);
      }
    },
    [load],
  );

  /** 选中项里真正缺交付的那些 —— 已交付的重拷一次没有意义。 */
  const missing = useMemo(
    () =>
      [...sel].filter((id) => {
        const r = rows.find((x) => x.clip.id === id);
        return r?.stage === "pass" && !delivered(r.clip);
      }),
    [sel, rows],
  );

  return (
    <div className="col f1 ohide">
      <div className="vhd">
        <span className="ptt">视频成片</span>
        <span className="fs11 t3 nowrap">
          成片 <b className="mono t1">{passRows.length}</b> · 交付失败{" "}
          <b className="mono" style={{ color: undelivered > 0 ? "var(--er)" : "var(--t1)" }}>
            {undelivered}
          </b>{" "}
          · 累计扣费 <b className="mono t1">{spent}</b>
        </span>
        <div className="f1" />
        {/* 交付目录直接摆出来：「片子在哪」是这一页存在的第一个理由。 */}
        <span className="fs10 t3 nowrap ohide" style={{ maxWidth: 320 }} title={outDir}>
          交付到 {outDir || "—"}
        </span>
        <button
          type="button"
          className="btn xs"
          onClick={() =>
            void unwrap(commands.openClipsOutputDir()).catch((e) => toast.error(String(e)))
          }
          title={outDir}
        >
          <FolderOpen className="ic12" />
          打开
        </button>
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
            setDeliv("all");
          }}
          title="验收未通过的记录。成片本体已进废纸篓，这里留的是账与提示词。"
        >
          未通过<span className="n">{rows.filter((r) => r.stage === "rej").length}</span>
        </button>
        {tab === "pass" && undelivered > 0 && (
          <>
            <span className="lb" style={{ marginLeft: 6 }}>
              交付
            </span>
            {(
              [
                ["all", "全部"],
                ["missing", `交付失败 ${undelivered}`],
              ] as [Deliv, string][]
            ).map(([k, label]) => (
              <button
                key={k}
                type="button"
                className={cn("vchip", deliv === k && "on")}
                onClick={() => setDeliv(k)}
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
        <div className="pbody" style={{ minWidth: 0, padding: "10px 12px" }}>
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
          <ClipSide row={curRow} busy={busy} onRedeliver={() => void redeliver([curRow.clip.id])} />
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
              disabled={busy || missing.length === 0}
              onClick={() => void redeliver(missing)}
              title="把成片再拷一次到交付目录（clips/ 里那份一直都在，从来只拷不移）"
            >
              <RefreshCw className="ic12" />
              重新交付 {missing.length} 条
            </button>
            {missing.length < sel.size && (
              <span className="fs11 t3">{sel.size - missing.length} 条已交付</span>
            )}
          </>
        ) : (
          <span className="fs11 t3 nowrap ohide">
            {visible.length} 条 ·{" "}
            {tab === "pass"
              ? "验收通过即自动拷进交付目录；拷贝失败不会回滚验收，可随时重新交付"
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
        {r.stage === "pass" && !delivered(c) && <span className="tag er">交付失败</span>}
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
function ClipSide({
  row,
  busy,
  onRedeliver,
}: { row: Row; busy: boolean; onRedeliver: () => void }) {
  const c = row.clip;
  const video = assetSrc(c.videoPath);
  return (
    <div className="vinsp">
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
      {/* 交付路径是这一栏最要紧的一件事：clips/clip{id}.mp4 那个内部名字人在 Finder
          里认不出谁是谁，而这里给的是真正交付出去的那份。 */}
      {row.stage === "pass" && (
        <>
          <div className="vsec">这一条交付到哪了</div>
          {delivered(c) ? (
            <div className="pathwell mt5" style={{ wordBreak: "break-all" }}>
              {c.exportPath}
            </div>
          ) : (
            <div className="vhint er mt5">
              验收通过了，但成片没能拷进交付目录 —— 验收那一刻的拷贝失败**不回滚验收**
              （判定是人做的，文件可以补）。原片还在，点下面重新交付。
            </div>
          )}
          <button
            type="button"
            className={cn("btn sm mt5", !delivered(c) && "pri")}
            style={{ width: "100%" }}
            disabled={busy || c.videoPath == null}
            onClick={onRedeliver}
            title="把成片再拷一次到当前交付目录"
          >
            <RefreshCw className="ic12" />
            {delivered(c) ? "重新交付一份" : "重新交付"}
          </button>
        </>
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
