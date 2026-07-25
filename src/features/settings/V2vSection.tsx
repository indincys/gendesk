import { type ModelInfo, type V2vSettings, commands, unwrap } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { FolderOpen, RefreshCw } from "lucide-react";
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
  const [credit, setCredit] = useState<number | null>(null);
  const [checking, setChecking] = useState(false);

  useEffect(() => {
    void unwrap(commands.getV2vSettings())
      .then(setS)
      .catch(() => {});
    void unwrap(commands.v2vModels())
      .then(setModels)
      .catch(() => setModels([]));
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
  };

  const checkCredit = async () => {
    setChecking(true);
    try {
      setCredit(await unwrap(commands.v2vCredit()));
    } catch (e) {
      setCredit(null);
      toast.error(String(e));
    } finally {
      setChecking(false);
    }
  };

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
          placeholder="dreamina（走 PATH）或绝对路径"
          onChange={(e) => setS({ ...s, bin: e.target.value })}
          onBlur={() => void save({ bin: s.bin ?? "" })}
        />
        <button type="button" className="btn sm gho" disabled={checking} onClick={checkCredit}>
          <RefreshCw className="ic12" />
          查余额
        </button>
        {credit != null && <span className="bdg b-green">{credit} 额度</span>}
      </div>
      <div className="fs11 t3 mt6" style={{ lineHeight: 1.7 }}>
        需先在终端跑一次 <span className="chip">dreamina login</span> 完成授权。 CLI 的 flags
        会随版本变，故提交前会把<b>即将执行的完整命令行</b>摆在确认卡里。
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
      </div>
      <div className="fs11 t3 mt6" style={{ lineHeight: 1.7 }}>
        改写 skill 若对某一条给了具体建议（这条动势大就给 8 秒），
        <b>以它为准</b>——那是看过图之后的判断，这里的值只作兜底。
      </div>
    </section>
  );
}
