import { Modal } from "@/components/ui/Modal";
import { DescriptionHint } from "@/components/ui/Tooltip";
import {
  type EffectiveParams,
  type ModelInfo,
  type QueueStats,
  type SessionInfo,
  type V2vSettings,
  commands,
  unwrap,
} from "@/lib/ipc";
import { AlertTriangle, Check, RefreshCw } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { toast } from "sonner";

/**
 * 默认参数与账户。
 *
 * ## 这一层**只管全局默认**（v0.22.0）
 *
 * 它原来用同一组下拉框做两件事：一边改全局默认，一边「应用到选中的 N 条」——
 * 一套控件两种作用域，且没有任何视觉区分，于是「我到底改的是哪个」无从判断。
 * 批量覆盖已经搬去它该在的地方（选中条目后底栏的参数条、提交确认卡），
 * 这里只剩一种含义：**新条目的默认值**。
 *
 * 三处作用域各自在自己的标签上说清楚：
 * 这里说「默认」· 详情栏说「这一条」· 底栏与提交卡说「这 N 条」。
 *
 * 它回答的仍是原来那几个问题：
 * - **走哪个模型**：留空意味着「跟随 CLI 默认」，而那个默认是什么，界面上看不出来。
 *   故显示的是**归一化之后**的三件套，外加一条与真正 exec 同源的示例命令行。
 * - **哪个通道**：`--session` 原来只是个裸数字输入框。这里直接列出即梦的会话。
 *   会话是账号级的，故它**只**在这里 —— 不做成每条可改。
 */
export function V2vParamsPanel({
  models,
  queue,
  onClose,
}: {
  models: ModelInfo[];
  /** 并发段要说的是**现在**的样子（发现窗口、最近拒收点、本地排队），故取实时快照。 */
  queue?: QueueStats | null;
  onClose: () => void;
}) {
  const [s, setS] = useState<V2vSettings | null>(null);
  const [eff, setEff] = useState<EffectiveParams | null>(null);
  const [balance, setBalance] = useState<number | null>(null);
  const [sessions, setSessions] = useState<SessionInfo[] | null>(null);
  const [busy, setBusy] = useState(false);

  const reload = useCallback(async () => {
    const [set, e] = await Promise.all([
      unwrap(commands.getV2vSettings()),
      unwrap(commands.v2vEffectiveParams()),
    ]);
    setS(set);
    setEff(e);
  }, []);

  useEffect(() => {
    void reload().catch(() => {});
    void unwrap(commands.v2vBalance())
      .then(setBalance)
      .catch(() => setBalance(null));
    void unwrap(commands.v2vSessions())
      .then(setSessions)
      .catch(() => setSessions([]));
  }, [reload]);

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

  if (!s) return null;
  const picked = models.find((m) => m.modelVersion === s.modelVersion);
  // `#[serde(default)]` 让 specta 把它标成可选，但 Rust 那边一定会填 —— 取个局部变量
  // 收窄类型，而不是在 TS 里再抄一份默认值（那份必然与 Rust 的默认值分叉）。
  const af = s.autofill;

  return (
    <Modal
      title="默认参数与账户"
      width="w700"
      onClose={onClose}
      headerExtra={
        balance != null ? (
          <span className="bdg b-green">余额 {balance}</span>
        ) : (
          <span className="chip">余额 —</span>
        )
      }
      footer={
        <>
          <div className="f1" />
          <button type="button" className="btn sm pri" onClick={onClose}>
            完成
          </button>
        </>
      }
    >
      <div style={{ padding: 4 }}>
        {/* ── 实际生效的参数 ───────────────────────────── */}
        <div className="fs11 fw6 t3" style={{ letterSpacing: ".05em", marginBottom: 6 }}>
          新条目的默认参数 · 实际发往即梦的字段
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
                {m.modelVersion}（{m.minDuration}–{m.maxDuration}s · {m.resolutions.join("/")} ·{" "}
                {m.creditAtMin === null ? "单价未实测" : `${m.creditAtMin} 额度起/条`}）
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
          <div className="fs11 t3 mt8">
            {eff?.usesCliDefaults ? (
              <>跟随 CLI 默认</>
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
        <div className="fs11 t3 mt6" style={{ lineHeight: 1.7 }}>
          {eff?.resolvedBin ? (
            <span className="fx ac gap6">
              <Check className="ic12" style={{ color: "var(--ok)" }} />
              CLI：<span className="chip">{eff.resolvedBin}</span>
            </span>
          ) : (
            <span className="fx ac gap6" style={{ color: "var(--wr)" }}>
              <AlertTriangle className="ic12" />
              没探测到即梦 CLI —— 去设置页「视频生成」填它的绝对路径。
            </span>
          )}
        </div>

        {/* ── 同时在跑上限（即梦逐通道的并发闸门） ─────────── */}
        <div
          className="fs11 fw6 t3 mt14 fx ac gap4"
          style={{ letterSpacing: ".05em", marginBottom: 6 }}
        >
          通道并发
          <DescriptionHint label="通道并发说明">
            所有模型通道默认独立自适应；远端容量变化时会自动降级并重新向上探测
          </DescriptionHint>
        </div>
        <div className="fx ac gap8 wrap">
          <span className="bdg b-green">全部通道 · 自动</span>
          <span className="fs11 t3">单轮安全探测上限 100 条</span>
          {queue?.observedLimit != null && (
            <span className="bdg b-amber">
              最近拒收点 {queue.observedLimit}
              <DescriptionHint label="最近拒收点说明">
                默认通道最近在 {queue.observedLimit} 条附近拒收；冷却后会自动重新向上探测
              </DescriptionHint>
            </span>
          )}
          {(queue?.queued ?? 0) > 0 && (
            <span className="fs11 t3">当前本地排队共 {queue?.queued} 条</span>
          )}
        </div>
        <div className="fs11 t3 mt6" style={{ lineHeight: 1.7 }}>
          不再保存人工并发数。每条模型通道从健康扣费回执继续向上发现；一旦远端返回
          <span className="chip">ret=1310</span> 且没有扣费记录，只暂停并降低该通道， 被拒任务回到
          FIFO 候补。冷却后自动重开探测窗口，容量恢复时会自己升回去。
        </div>
        {/* 逐通道现状：这个上限是按通道算的，那就必须能当场看到每条通道各占了多少。 */}
        {(queue?.channels?.length ?? 0) > 0 && (
          <div className="fx ac gap8 wrap" style={{ marginTop: 8 }}>
            {queue?.channels.map((c) => (
              <span key={c.modelVersion || "(default)"} className="fs11 t3">
                <b className="t1">{c.label}</b> {c.running}/{c.limit} 在跑
                {c.observedLimit != null && ` · 最近拒收点 ${c.observedLimit}`}
                {c.queued > 0 && ` · 本地 ${c.queued}`}
              </span>
            ))}
          </div>
        )}

        {/* ── 常驻队列（自动补单） ───────────────────────── */}
        {af && (
          <>
            <div
              className="fs11 fw6 t3 mt14 fx ac gap4"
              style={{ letterSpacing: ".05em", marginBottom: 6 }}
            >
              常驻队列
              <DescriptionHint label="常驻队列说明">
                保持指定数量的非 VIP 任务运行，完成一条后自动补一条
              </DescriptionHint>
            </div>
            <div className="fx ac gap8 wrap">
              <label className="fx ac gap6 fs12">
                <input
                  type="checkbox"
                  checked={af.enabled}
                  disabled={busy}
                  onChange={(e) => void save({ autofill: { ...af, enabled: e.target.checked } })}
                />
                开启常驻队列
              </label>
              <span className="fs11 t3">常驻</span>
              <input
                className="inp sm"
                style={{ width: 64 }}
                type="number"
                min={1}
                max={20}
                value={af.depth}
                onChange={(e) => setS({ ...s, autofill: { ...af, depth: Number(e.target.value) } })}
                onBlur={() => void save({ autofill: af })}
              />
              <span className="fs11 t3">条在跑 · 模型</span>
              <select
                className="inp sm"
                style={{ minWidth: 170 }}
                value={af.modelVersion}
                disabled={busy}
                onChange={(e) =>
                  void save({
                    autofill: {
                      ...af,
                      modelVersion: e.target.value,
                      duration: null,
                      videoResolution: "",
                    },
                  })
                }
              >
                {models
                  // VIP 通道不进这个选择器：这条队列的全部前提就是便宜。
                  // 后端保存时也会拒 —— 选择器不该是唯一的闸门。
                  .filter((m) => !m.vip)
                  .map((m) => (
                    <option key={m.modelVersion} value={m.modelVersion}>
                      {m.modelVersion}
                      {m.creditAtMin === null ? "" : `（${m.creditAtMin} 额度起/条）`}
                    </option>
                  ))}
              </select>
            </div>
            <div className="fx ac gap8 wrap mt6">
              <span className="fs11 t3">存量低于</span>
              <input
                className="inp sm"
                style={{ width: 64 }}
                type="number"
                min={0}
                value={af.lowWater}
                onChange={(e) =>
                  setS({ ...s, autofill: { ...af, lowWater: Number(e.target.value) } })
                }
                onBlur={() => void save({ autofill: af })}
              />
              <span className="fs11 t3">条时通知 · 每日额度上限</span>
              <input
                className="inp sm"
                style={{ width: 84 }}
                type="number"
                min={0}
                value={af.dailyCredits}
                onChange={(e) =>
                  setS({ ...s, autofill: { ...af, dailyCredits: Number(e.target.value) } })
                }
                onBlur={() => void save({ autofill: af })}
              />
              <span className="fs11 t3">{(af.dailyCredits ?? 0) > 0 ? "额度" : "不限"}</span>
            </div>
          </>
        )}

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
          {sessions?.length === 0 && <span className="fs11 t3">会话不可用</span>}
        </div>
      </div>
    </Modal>
  );
}
