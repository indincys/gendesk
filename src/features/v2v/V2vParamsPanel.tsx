import { Modal } from "@/components/ui/Modal";
import {
  type CreditStats,
  type EffectiveParams,
  type ModelInfo,
  type SessionInfo,
  type V2vSettings,
  commands,
  unwrap,
} from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { AlertTriangle, Check, RefreshCw, Wand2 } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { toast } from "sonner";

/**
 * 生成参数与额度面板。
 *
 * 用户的原话是「无法在软件内**有效**编辑和查看视频生成的参数」。原来的界面确实有几个
 * 下拉框（在设置页），但它们回答不了实际的问题：
 *
 * - **走哪个模型**：留空意味着「跟随 CLI 默认」，而那个默认是什么，界面上看不出来。
 *   故这里显示的是**归一化之后**的三件套，外加一条与真正 exec 同源的示例命令行。
 * - **哪个通道**：`--session` 原来只是个裸数字输入框。这里直接列出即梦的会话。
 * - **积分额度 / 用量**：余额来自远端账户，消耗来自本机出片时收到的扣费回执。
 *   **不合并成一个百分比** —— 两个数字来自两个地方，编一个比值出来会让它们的差异
 *   变得无法解释（别处也可能在花同一个账户的额度）。
 * - **改起来要能整批改**：参数恰恰是最常成组调整的东西（「这一组都换成 vip 1080p」），
 *   而原来只能一条一条开详情弹窗。
 */
export function V2vParamsPanel({
  models,
  selectedReady,
  onClose,
  onApplied,
}: {
  models: ModelInfo[];
  /** 当前在「待改写/待提交」列勾选中的条目 id（批量覆盖的作用对象）。 */
  selectedReady: number[];
  onClose: () => void;
  onApplied: () => void;
}) {
  const [s, setS] = useState<V2vSettings | null>(null);
  const [eff, setEff] = useState<EffectiveParams | null>(null);
  const [stats, setStats] = useState<CreditStats | null>(null);
  const [sessions, setSessions] = useState<SessionInfo[] | null>(null);
  const [loadingCredit, setLoadingCredit] = useState(false);
  const [busy, setBusy] = useState(false);

  const reload = useCallback(async () => {
    const [set, e] = await Promise.all([
      unwrap(commands.getV2vSettings()),
      unwrap(commands.v2vEffectiveParams()),
    ]);
    setS(set);
    setEff(e);
  }, []);

  const refreshCredit = useCallback(async () => {
    setLoadingCredit(true);
    try {
      setStats(await unwrap(commands.v2vCreditStats()));
    } catch (err) {
      toast.error(String(err));
    } finally {
      setLoadingCredit(false);
    }
  }, []);

  useEffect(() => {
    void reload().catch(() => {});
    // 余额与会话都要跑一次 CLI（秒级），故与面板主体并行加载，不挡住渲染。
    void refreshCredit();
    void unwrap(commands.v2vSessions())
      .then(setSessions)
      .catch(() => setSessions([]));
  }, [reload, refreshCredit]);

  const save = async (p: Partial<V2vSettings>) => {
    if (!s) return;
    setBusy(true);
    try {
      setS(await unwrap(commands.updateV2vSettings({ ...s, ...p })));
      setEff(await unwrap(commands.v2vEffectiveParams()));
    } catch (e) {
      // 后端拒绝（非法组合）→ 提示并回读，别把界面停在一个没存进去的值上。
      toast.error(String(e));
      await reload().catch(() => {});
    } finally {
      setBusy(false);
    }
  };

  const applyToSelected = async () => {
    if (!s || selectedReady.length === 0) return;
    setBusy(true);
    try {
      const n = await unwrap(
        commands.setV2vClipParams(
          selectedReady,
          blank(s.modelVersion),
          s.duration ?? null,
          blank(s.videoResolution),
        ),
      );
      toast.success(`已覆盖 ${n} 条的生成参数`);
      onApplied();
    } catch (e) {
      toast.error(String(e));
    } finally {
      setBusy(false);
    }
  };

  if (!s) return null;
  const picked = models.find((m) => m.modelVersion === s.modelVersion);

  return (
    <Modal
      title="生成参数与额度"
      width="w700"
      onClose={onClose}
      headerExtra={
        stats?.balance != null ? (
          <span className="bdg b-green">余额 {stats.balance}</span>
        ) : (
          <span className="chip">余额 —</span>
        )
      }
      footer={
        <>
          <span className="fs11 t3">
            改写 skill 对单条给出的建议优先于这里的默认值（那是看过图之后的判断）
          </span>
          <div className="f1" />
          {selectedReady.length > 0 && (
            <button
              type="button"
              className="btn sm"
              disabled={busy}
              onClick={applyToSelected}
              title="把上面这套参数写进选中的条目，覆盖 skill 给的建议"
            >
              <Wand2 className="ic12" />
              应用到选中的 {selectedReady.length} 条
            </button>
          )}
          <button type="button" className="btn sm pri" onClick={onClose}>
            完成
          </button>
        </>
      }
    >
      <div style={{ padding: 4 }}>
        {/* ── 实际生效的参数 ───────────────────────────── */}
        <div className="fs11 fw6 t3" style={{ letterSpacing: ".05em", marginBottom: 6 }}>
          实际发往即梦的参数
        </div>
        <div className="fx ac gap8 wrap">
          <select
            className="inp sm"
            style={{ minWidth: 190 }}
            value={s.modelVersion ?? ""}
            disabled={busy}
            onChange={(e) =>
              // 换模型即清掉时长与分辨率：留着上一个模型的值必然撞它的约束，
              // 而报错要发生在花钱之前，不该等到提交。
              void save({ modelVersion: e.target.value, duration: null, videoResolution: "" })
            }
          >
            <option value="">跟随 CLI 默认（不发高级参数）</option>
            {models.map((m) => (
              <option key={m.modelVersion} value={m.modelVersion}>
                {m.modelVersion}（{m.minDuration}–{m.maxDuration}s · {m.resolutions.join("/")}）
              </option>
            ))}
          </select>
          {picked && (
            <>
              <input
                className="inp sm"
                style={{ width: 96 }}
                type="number"
                min={picked.minDuration}
                max={picked.maxDuration}
                placeholder={`${picked.minDuration}–${picked.maxDuration}s`}
                value={s.duration ?? ""}
                onChange={(e) =>
                  setS({ ...s, duration: e.target.value === "" ? null : Number(e.target.value) })
                }
                onBlur={() => void save({ duration: s.duration ?? null })}
              />
              <select
                className="inp sm"
                value={s.videoResolution ?? ""}
                disabled={busy}
                onChange={(e) => void save({ videoResolution: e.target.value })}
              >
                {picked.resolutions.map((r) => (
                  <option key={r} value={r}>
                    {r}
                  </option>
                ))}
              </select>
            </>
          )}
        </div>

        {eff?.error ? (
          <div className="fs11 mt8" style={{ color: "var(--er)", lineHeight: 1.7 }}>
            <AlertTriangle className="ic12" /> {eff.error}
          </div>
        ) : (
          <div className="fs11 t3 mt8" style={{ lineHeight: 1.8 }}>
            {eff?.usesCliDefaults ? (
              <>
                当前<b>一个高级参数都不发</b>，模型、时长、分辨率全由即梦 CLI
                自己决定（最稳，也不把模型名锁死在我们这边）。
              </>
            ) : (
              <>
                模型 <span className="chip">{eff?.modelVersion}</span> · 时长{" "}
                <span className="chip">{eff?.duration}s</span> · 分辨率{" "}
                <span className="chip">{eff?.videoResolution}</span>
                {eff?.duration != null &&
                  s.duration == null &&
                  "（时长你没填，按该模型的最短值补的）"}
              </>
            )}
          </div>
        )}
        {eff?.sampleCommand && (
          <>
            <div className="fs11 t3 mt8">提交时实际执行的命令（图片与提示词逐条替换）：</div>
            <div className="cmdwell mt6">{eff.sampleCommand}</div>
          </>
        )}
        <div className="fs11 t3 mt6" style={{ lineHeight: 1.7 }}>
          {eff?.resolvedBin ? (
            <span className="fx ac gap6">
              <Check className="ic12" style={{ color: "var(--ok)" }} />
              CLI：<span className="chip">{eff.resolvedBin}</span>
            </span>
          ) : (
            <span className="fx ac gap6" style={{ color: "var(--wr)" }}>
              <AlertTriangle className="ic12" />
              没探测到即梦 CLI —— 去设置页「图生视频」填它的绝对路径。
            </span>
          )}
        </div>

        {/* ── 通道（会话） ─────────────────────────────── */}
        <div className="fs11 fw6 t3 mt14" style={{ letterSpacing: ".05em", marginBottom: 6 }}>
          通道 · 即梦会话
        </div>
        <div className="fx ac gap8 wrap">
          <select
            className="inp sm"
            style={{ minWidth: 220 }}
            value={s.session ?? ""}
            disabled={busy}
            onChange={(e) =>
              void save({ session: e.target.value === "" ? null : Number(e.target.value) })
            }
          >
            <option value="">默认会话（0）</option>
            {(sessions ?? []).map((x) => (
              <option key={x.id} value={x.id}>
                {x.id} · {x.name}
                {x.pinned ? " · 置顶" : ""}
              </option>
            ))}
          </select>
          <button
            type="button"
            className="btn xs gho"
            onClick={() =>
              void unwrap(commands.v2vSessions())
                .then(setSessions)
                .catch((e) => toast.error(String(e)))
            }
          >
            <RefreshCw className="ic12" />
            刷新
          </button>
          {sessions?.length === 0 && (
            <span className="fs11 t3">读不到会话列表（未登录或 CLI 输出格式变了），可留默认</span>
          )}
        </div>
        <div className="fs11 t3 mt6" style={{ lineHeight: 1.7 }}>
          会话是即梦那边归置生成历史的容器，只影响任务落在哪条历史里，不影响画面。
        </div>

        {/* ── 额度 ─────────────────────────────────────── */}
        <div className="fx ac gap8 mt14" style={{ marginBottom: 6 }}>
          <span className="fs11 fw6 t3" style={{ letterSpacing: ".05em" }}>
            积分额度
          </span>
          <button
            type="button"
            className="btn xs gho"
            disabled={loadingCredit}
            onClick={() => void refreshCredit()}
          >
            <RefreshCw className={cn("ic12", loadingCredit && "spin")} />
            刷新
          </button>
        </div>
        {stats?.balanceError && (
          <div className="fs11 mt6" style={{ color: "var(--wr)", lineHeight: 1.7 }}>
            查不到余额：{stats.balanceError}
          </div>
        )}
        <div className="statgrid">
          <Stat label="账户余额" value={stats?.balance == null ? "—" : String(stats.balance)} />
          <Stat label="累计已用" value={String(stats?.spentTotal ?? 0)} />
          <Stat label="近 7 天" value={String(stats?.spentWeek ?? 0)} />
          <Stat label="近 24 小时" value={String(stats?.spentDay ?? 0)} />
        </div>
        <div className="statgrid mt8">
          <Stat label="成片（值回票价）" value={String(stats?.spentPass ?? 0)} tone="ok" />
          <Stat label="未通过（白花的）" value={String(stats?.spentRej ?? 0)} tone="er" />
          <Stat label="待验收（未定论）" value={String(stats?.spentPending ?? 0)} />
          <Stat label="计入条数" value={String(stats?.countedClips ?? 0)} />
        </div>
        <div className="fs11 t3 mt8" style={{ lineHeight: 1.8 }}>
          消耗只统计**出片时收到扣费回执**的条目 —— 提交那一刻并不知道这一条会花多少，
          所以任何「预估用量」都是编的。
          {stats?.vipLevel && (
            <>
              <br />
              当前账号等级 <span className="chip">{stats.vipLevel}</span>
              {stats.userId != null && <> · ID {stats.userId}</>}
            </>
          )}
        </div>
      </div>
    </Modal>
  );
}

/** 空串折成 null：空输入框不该变成 `--model_version=` 这种必被拒的空 flag。 */
function blank(v: string | undefined): string | null {
  const t = (v ?? "").trim();
  return t === "" ? null : t;
}

function Stat({ label, value, tone }: { label: string; value: string; tone?: "ok" | "er" }) {
  return (
    <div className="statcell">
      <div className="fs10 t3 nowrap ohide">{label}</div>
      <div
        className="fs16 fw6"
        style={{ color: tone === "ok" ? "var(--ok)" : tone === "er" ? "var(--er)" : undefined }}
      >
        {value}
      </div>
    </div>
  );
}
