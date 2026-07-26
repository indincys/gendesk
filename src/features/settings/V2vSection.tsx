import {
  type CreditInfo,
  type ModelInfo,
  type SessionInfo,
  type V2vSettings,
  commands,
  unwrap,
} from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { AlertTriangle, Check, FolderOpen, RefreshCw } from "lucide-react";
import { useEffect, useState } from "react";
import { toast } from "sonner";

/**
 * 设置页「图生视频」区块。
 *
 * 两件事在这里定：**交接目录在哪**（skill 要把它写死）与**默认生成参数**。
 * 参数留空即不发高级 flag，走即梦 CLI 自己的默认路径 —— 那是最稳的默认，
 * 也不把模型名锁死在我们这一侧（CLI 换默认模型时我们跟着走）。
 */
export function V2vSection() {
  const [s, setS] = useState<V2vSettings | null>(null);
  const [models, setModels] = useState<ModelInfo[]>([]);
  const [credit, setCredit] = useState<CreditInfo | null>(null);
  const [sessions, setSessions] = useState<SessionInfo[] | null>(null);
  const [checking, setChecking] = useState(false);
  /** 当前设置实际会执行哪个文件（后端探测结果）；null = 没找到。 */
  const [resolved, setResolved] = useState<string | null>(null);

  const refreshResolved = () =>
    void unwrap(commands.resolveV2vBin())
      .then(setResolved)
      .catch(() => setResolved(null));

  useEffect(() => {
    void unwrap(commands.getV2vSettings())
      .then(setS)
      .catch(() => {});
    void unwrap(commands.v2vModels())
      .then(setModels)
      .catch(() => setModels([]));
    refreshResolved();
  }, []);

  const save = async (p: Partial<V2vSettings>) => {
    if (!s) return;
    try {
      setS(await unwrap(commands.updateV2vSettings({ ...s, ...p })));
    } catch (e) {
      // 后端拒绝（非法参数组合）→ 提示并回读，避免界面停在一个没存进去的值上。
      toast.error(String(e));
      setS(await unwrap(commands.getV2vSettings()));
    }
    if (p.bin !== undefined) refreshResolved();
  };

  const checkCredit = async () => {
    setChecking(true);
    try {
      setCredit(await unwrap(commands.v2vCredit()));
      // 顺带把会话列表取回来：这两件事都要跑一次 CLI，而人点「查余额」的时机
      // 恰好就是「CLI 现在是通的」那一刻。
      setSessions(await unwrap(commands.v2vSessions()).catch(() => []));
    } catch (e) {
      setCredit(null);
      toast.error(String(e));
    } finally {
      setChecking(false);
    }
  };

  useEffect(() => {
    // 会话列表不阻塞渲染；读不到就退化成手填数字（下面那个 input 仍在）。
    void unwrap(commands.v2vSessions())
      .then(setSessions)
      .catch(() => setSessions([]));
  }, []);

  if (!s) return null;
  const picked = models.find((m) => m.modelVersion === s.modelVersion);

  return (
    <section className="sec">
      <div className="sechead">
        <span className="fw6 fs13">图生视频</span>
        <span className="pcap">交接目录 · 即梦 CLI · 默认生成参数</span>
      </div>

      <div className="fs11 fw6 t3" style={{ letterSpacing: ".05em", marginBottom: 6 }}>
        交接目录 · Claude Code / Codex 侧的 skill 读写这里
      </div>
      <div className="fx ac gap10">
        <div className="pathwell f1">{s.handoffRoot}</div>
        <button
          type="button"
          className="btn sm"
          onClick={async () => {
            const dir = await unwrap(commands.pickHandoffRoot()).catch(() => null);
            if (dir) await save({ handoffRoot: dir });
          }}
        >
          <FolderOpen className="ic12" />
          更改目录
        </button>
      </div>
      <div className="fs11 t3 mt6" style={{ lineHeight: 1.8 }}>
        待改写的工单自动写到 <span className="chip">v2v/待改写/</span>（队列一变就重写，
        <b>不需要点任何导出</b>）；skill 把改写结果写回{" "}
        <span className="chip">v2v/已改写/&lt;组&gt;/rewrite.jsonl</span>，GenDesk 监听收录。
        <br />
        skill 只做一件事：把生图提示词改写成图生视频提示词。
        <b>提交、轮询、下载、重试、验收都在 GenDesk 里</b>——那些不是智能任务。
      </div>

      <div className="fs11 fw6 t3 mt14" style={{ letterSpacing: ".05em", marginBottom: 6 }}>
        即梦 CLI
      </div>
      <div className="fx ac gap10">
        <input
          className="inp f1"
          value={s.bin}
          placeholder="留空自动探测"
          onChange={(e) => setS({ ...s, bin: e.target.value })}
          onBlur={() => void save({ bin: s.bin ?? "" })}
        />
        <button
          type="button"
          className="btn sm"
          onClick={async () => {
            const f = await unwrap(commands.pickDreaminaBin()).catch(() => null);
            if (f) await save({ bin: f });
          }}
        >
          <FolderOpen className="ic12" />
          选择文件
        </button>
        <button type="button" className="btn sm gho" disabled={checking} onClick={checkCredit}>
          <RefreshCw className="ic12" />
          查余额
        </button>
        {credit != null && (
          <>
            <span className="bdg b-green">{credit.totalCredit} 额度</span>
            {credit.vipLevel && <span className="chip">{credit.vipLevel}</span>}
          </>
        )}
      </div>
      {/* 「路径填什么」不该由人回答：直接把解析结果摆出来，说清实际会执行哪个文件。 */}
      <div className="fs11 t3 mt6" style={{ lineHeight: 1.8 }}>
        {resolved ? (
          <span className="fx ac gap6">
            <Check className="ic12" style={{ color: "var(--ok)" }} />
            实际会执行：<span className="chip">{resolved}</span>
          </span>
        ) : (
          <span className="fx ac gap6" style={{ color: "var(--wr)" }}>
            <AlertTriangle className="ic12" />
            没探测到即梦 CLI。终端里跑 <span className="chip">which dreamina</span>
            ，把输出用「选择文件」选中或粘贴到上面。
          </span>
        )}
        <br />
        留空即自动探测（PATH 与 <span className="chip">~/.local/bin</span> 等常见安装位置）。
        <b>从访达启动的应用拿不到终端的 PATH</b>，所以「终端里能跑」不等于这里能找到 ——
        探测不到时填绝对路径最稳。需先在终端跑一次 <span className="chip">dreamina login</span>{" "}
        完成授权。
      </div>

      <div className="fx ac gap10 mt14">
        <span className="fs12 fw5">后台轮询</span>
        <div className="seg">
          <span
            className={cn("sgi", s.pollEnabled && "on")}
            onClick={() => void save({ pollEnabled: true })}
          >
            开
          </span>
          <span
            className={cn("sgi", !s.pollEnabled && "on")}
            onClick={() => void save({ pollEnabled: false })}
          >
            关
          </span>
        </div>
        <span className="fs11 t3">
          关掉后已提交的条目不再自动取回（排查时用）；额度已扣的任务不会丢，重新打开即接着轮询。
        </span>
      </div>

      {/* 超时上限。默认不限 —— 判死一条还在跑的任务代价是钱（额度已扣、即梦那边照跑），
          而多等的代价只是看板上多几条「已提交」。两边不对等。 */}
      <div className="fx ac gap10 mt10 wrap">
        <span className="fs12 fw5">判超时</span>
        <select
          className="inp"
          style={{ width: 150 }}
          value={s.timeoutHours ?? ""}
          onChange={(e) =>
            void save({ timeoutHours: e.target.value === "" ? null : Number(e.target.value) })
          }
        >
          <option value="">不限（推荐）</option>
          <option value={3}>3 小时</option>
          <option value={12}>12 小时</option>
          <option value={24}>24 小时</option>
        </select>
        <span className="fs11 t3" style={{ lineHeight: 1.7 }}>
          即梦排队可能很久（实测提交 72 分钟后仍在 <span className="chip">querying</span>）。
          <b>不限</b>意味着睡前提交、第二天醒来收片；轮询会自动退避（等满一小时后每 10
          分钟才问一次），挂着不费什么。
        </span>
      </div>

      <div className="fs11 fw6 t3 mt14" style={{ letterSpacing: ".05em", marginBottom: 6 }}>
        默认生成参数
      </div>
      <div className="fx ac gap10 wrap">
        <select
          className="inp"
          value={s.modelVersion}
          onChange={(e) =>
            // 换模型即清掉时长与分辨率：留着上一个模型的值必然撞它的约束。
            void save({ modelVersion: e.target.value, duration: null, videoResolution: "" })
          }
        >
          <option value="">默认（不发高级参数，跟随 CLI）</option>
          {models.map((m) => (
            <option key={m.modelVersion} value={m.modelVersion}>
              {m.modelVersion}
            </option>
          ))}
        </select>
        {picked && (
          <>
            <input
              className="inp"
              style={{ width: 110 }}
              type="number"
              min={picked.minDuration}
              max={picked.maxDuration}
              placeholder={`${picked.minDuration}–${picked.maxDuration} 秒`}
              value={s.duration ?? ""}
              onChange={(e) =>
                setS({ ...s, duration: e.target.value === "" ? null : Number(e.target.value) })
              }
              onBlur={() => void save({ duration: s.duration ?? null })}
            />
            <select
              className="inp"
              value={s.videoResolution}
              onChange={(e) => void save({ videoResolution: e.target.value })}
            >
              <option value="">{picked.resolutions[0]}（默认）</option>
              {picked.resolutions.map((r) => (
                <option key={r} value={r}>
                  {r}
                </option>
              ))}
            </select>
          </>
        )}
        {/* 通道 = 即梦会话。原先这里只是个裸数字输入框 ——「这个数字是哪条会话」
            在应用里根本无从得知，于是它等于不可用。读不到列表时才退回手填。 */}
        {sessions && sessions.length > 0 ? (
          <select
            className="inp"
            style={{ minWidth: 200 }}
            value={s.session ?? ""}
            onChange={(e) =>
              void save({ session: e.target.value === "" ? null : Number(e.target.value) })
            }
          >
            <option value="">默认会话（0）</option>
            {sessions.map((x) => (
              <option key={x.id} value={x.id}>
                {x.id} · {x.name}
              </option>
            ))}
          </select>
        ) : (
          <input
            className="inp"
            style={{ width: 130 }}
            type="number"
            min={0}
            placeholder="会话 id（可空）"
            value={s.session ?? ""}
            onChange={(e) =>
              setS({ ...s, session: e.target.value === "" ? null : Number(e.target.value) })
            }
            onBlur={() => void save({ session: s.session ?? null })}
          />
        )}
      </div>
      <div className="fs11 t3 mt6" style={{ lineHeight: 1.7 }}>
        改写 skill 若对某一条给了具体建议（这条动势大就给 8 秒），
        <b>以它为准</b>——那是看过图之后的判断，这里的值只作兜底。
        <br />
        视频流水线页顶部的「生成参数」按同一份设置显示<b>实际会发出去的命令行</b>与额度用量。
      </div>
    </section>
  );
}
