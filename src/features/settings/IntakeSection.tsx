import { type IntakeSettings, type JobView, commands, unwrap } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { AlertTriangle, Check, FolderOpen, PlayCircle, RefreshCw, RotateCcw } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { toast } from "sonner";

/**
 * 设置页「Claude Code 收件」区块。
 *
 * 这里回答三个问题，别处回答不了：
 * **投单往哪个目录放**（skill 要把它写死）、**刚才那几份工单怎么样了**、
 * **实际发往接口的是哪几个字段**。
 *
 * 最后一个尤其重要：收录是全自动的，人没有经过生成页那张确认卡，
 * 「我在工单里写了 9:16，到底发出去没有」在别处无处可查。
 */
export function IntakeSection() {
  const [s, setS] = useState<IntakeSettings | null>(null);
  const [jobs, setJobs] = useState<JobView[]>([]);
  const [dir, setDir] = useState<string>("");
  const [scanning, setScanning] = useState(false);

  const refresh = useCallback(async () => {
    setJobs(await unwrap(commands.listIntakeJobs(20)).catch(() => []));
    setDir(await unwrap(commands.intakePendingDir()).catch(() => ""));
  }, []);

  useEffect(() => {
    void unwrap(commands.getIntakeSettings())
      .then(setS)
      .catch(() => {});
    void refresh();
  }, [refresh]);

  const save = async (p: Partial<IntakeSettings>) => {
    if (!s) return;
    try {
      setS(await unwrap(commands.updateIntakeSettings({ ...s, ...p })));
      await refresh();
    } catch (e) {
      toast.error(String(e));
      setS(await unwrap(commands.getIntakeSettings()));
    }
  };

  const scan = async () => {
    setScanning(true);
    try {
      const done = await unwrap(commands.scanIntakeNow());
      toast(done.length ? `收录了 ${done.length} 份工单` : "收件目录里没有待处理的工单");
      await refresh();
    } catch (e) {
      toast.error(String(e));
    } finally {
      setScanning(false);
    }
  };

  const retry = async (id: number) => {
    try {
      await unwrap(commands.retryIntakeJob(id));
      await refresh();
    } catch (e) {
      toast.error(String(e));
    }
  };

  // 「确认开跑」做的事就是在工单目录里写下 `确认.txt` —— 与你在 Claude Code 里
  // touch 一下走的是同一段代码，不可能一条路对、另一条路错。
  const confirm = async (id: number) => {
    try {
      const done = await unwrap(commands.confirmIntakeJob(id));
      const ok = done.find((j) => j.status === "done");
      toast(ok ? `已开跑 · ${ok.taskCount} 张` : "已确认");
      await refresh();
    } catch (e) {
      toast.error(String(e));
    }
  };

  if (!s) return null;

  return (
    <section className="sec">
      <div className="sechead">
        <span className="fw6 fs13">Claude Code 收件</span>
        <span className="pcap">投单目录 · 自动导入并开跑 · 工单台账</span>
      </div>

      <div className="fx ac gap10">
        <span className="fs12 fw5">自动收录</span>
        <div className="seg">
          <span
            className={cn("sgi", s.enabled && "on")}
            onClick={() => void save({ enabled: true })}
          >
            开
          </span>
          <span
            className={cn("sgi", !s.enabled && "on")}
            onClick={() => void save({ enabled: false })}
          >
            关
          </span>
        </div>
        <span className="fs11 t3">
          开着时：skill 写完 <span className="chip">READY.txt</span> → 2 秒内自动导入提示词与参考图
          → <b>直接建批开跑</b>。验收仍在验收页人工完成。
        </span>
      </div>

      <div className="fs11 fw6 t3 mt14" style={{ letterSpacing: ".05em", marginBottom: 6 }}>
        投单目录 · skill 把工单目录写到这里
      </div>
      <div className="fx ac gap10">
        <div className="pathwell f1">{dir || "—"}</div>
        <button
          type="button"
          className="btn sm"
          onClick={async () => {
            const picked = await unwrap(commands.pickIntakeRoot()).catch(() => null);
            if (picked) await save({ root: picked });
          }}
        >
          <FolderOpen className="ic12" />
          更改目录
        </button>
        <button
          type="button"
          className="btn sm gho"
          onClick={() => void unwrap(commands.openIntakeDir()).catch(() => {})}
        >
          打开
        </button>
        <button type="button" className="btn sm gho" disabled={scanning} onClick={scan}>
          <RefreshCw className="ic12" />
          立即扫描
        </button>
      </div>
      <div className="fs11 t3 mt6" style={{ lineHeight: 1.8 }}>
        与图生视频共用同一个交接根（一个目录、配一次）。一份工单 = 一个目录：
        <span className="chip">提示词.txt</span> + <span className="chip">images/</span> + 最后写的{" "}
        <span className="chip">READY.txt</span>。挂靠与参数写在提示词文档的组头里（
        <span className="chip">参考图:</span> <span className="chip">比例:</span>{" "}
        <span className="chip">抽卡:</span> <span className="chip">用途:</span>）。
        <br />
        <b>应用没开也不丢单</b>：下次启动会把攒着的工单一并收进来。成功的目录移到{" "}
        <span className="chip">生图/_已收录/</span>（内含 <span className="chip">结果.txt</span>
        ）；失败与待确认的留在原地并写下回执，改完点「重试」。
      </div>

      {/* 阈值：花钱的闸门必须是机制，而不是投单那一侧的自觉——那边是另一个模型。 */}
      <div className="fx ac gap10 mt14 wrap">
        <span className="fs12 fw5">自动开跑上限</span>
        <input
          className="inp"
          type="number"
          min={0}
          step={50}
          style={{ width: 110 }}
          value={s.taskThreshold ?? 500}
          onChange={(e) => setS({ ...s, taskThreshold: Number(e.target.value) })}
          onBlur={() => void save({ taskThreshold: s.taskThreshold ?? 500 })}
        />
        <span className="fs11 t3" style={{ lineHeight: 1.7 }}>
          单份工单超过这么多<b>张图</b>就不自动跑，转「待确认」——
          <b>此时一条提示词、一张参考图都不会进库</b>，确认后才整份生效。
          <br />
          确认有两个口子、做的是同一件事：这里点「确认开跑」，或在工单目录里建一个空文件{" "}
          <span className="chip">确认.txt</span>。填 0 = 不限。
        </span>
      </div>

      <div className="fs11 fw6 t3 mt14" style={{ letterSpacing: ".05em", marginBottom: 6 }}>
        最近工单
      </div>
      {jobs.length === 0 ? (
        <div className="fs11 t3">还没有收到过工单。</div>
      ) : (
        <div className="fx col gap6">
          {jobs.map((j) => (
            <div key={j.id} className="fx ac gap10 wrap" style={{ lineHeight: 1.7 }}>
              {j.status === "done" ? (
                <Check className="ic12" style={{ color: "var(--ok)" }} />
              ) : (
                <AlertTriangle
                  className="ic12"
                  style={{ color: j.status === "error" ? "var(--er)" : "var(--wr)" }}
                />
              )}
              <span className="fs12 fw5">{j.jobId}</span>
              {j.status === "done" && (
                <>
                  {/* 一份工单可能建出多个批次：各组比例/抽卡不同就会拆批。 */}
                  {j.batchIds.map((b) => (
                    <span key={b} className="bdg b-green">
                      批次 #{b}
                    </span>
                  ))}
                  <span className="fs11 t3">
                    {j.groupCount} 组 · {j.refCount} 图 · {j.taskCount} 张
                  </span>
                  {/* 展示与执行同一来源：这串就是构建 multipart 用的那份 wire 记录。
                      全自动收录没经过生成页那张确认卡，这里是唯一能回查的地方。 */}
                  <span className="fs11 t3 mono">{j.wireJson.map(wireLabel).join(" ｜ ")}</span>
                </>
              )}
              {j.status === "hold" && (
                <>
                  <span className="fs11" style={{ color: "var(--wr)" }}>
                    {j.message}（还没导入任何东西）
                  </span>
                  <button type="button" className="btn sm" onClick={() => void confirm(j.id)}>
                    <PlayCircle className="ic12" />
                    确认开跑
                  </button>
                </>
              )}
              {(j.status === "error" || j.status === "running") && (
                <>
                  <span className="fs11" style={{ color: "var(--er)" }}>
                    {j.message || "收录中断（重试可再来一次）"}
                  </span>
                  <button type="button" className="btn sm gho" onClick={() => void retry(j.id)}>
                    <RotateCcw className="ic12" />
                    重试
                  </button>
                </>
              )}
            </div>
          ))}
        </div>
      )}
    </section>
  );
}

/**
 * 「实际发往接口的字段」一行文案。
 *
 * 入参就是后端构建请求用的那份 wire 记录（键名 = multipart 字段名），与生成页
 * 确认卡上那行同源、同口径 —— 两条入口对同一个问题不该给出两种说法。
 */
function wireLabel(wireJson: string): string {
  try {
    const wire = JSON.parse(wireJson) as Record<string, unknown>;
    const parts = Object.entries(wire).map(([k, v]) => `${k}=${String(v)}`);
    return parts.length ? parts.join(" · ") : "无显式参数";
  } catch {
    return "";
  }
}
