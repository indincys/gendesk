import { Modal } from "@/components/ui/Modal";
import { type JobPreview, commands, unwrap } from "@/lib/ipc";
import { AlertTriangle, PlayCircle } from "lucide-react";
import { useEffect, useState } from "react";
import { toast } from "sonner";

/**
 * 超阈值工单的开跑确认卡 —— **把挂靠关系摆出来，而不是摆一个数字**。
 *
 * 阈值是自动收录链路上唯一一处「停下来等人点头」的地方。原来那句
 * 「600 张，去设置页确认开跑」并不足以让人做判断：真正要核的是
 * **哪个提示词组配了哪几张参考图**——配错的代价是整批图跑出来全是错的，
 * 而那要到验收时才看得出来，那时钱已经花完了。
 *
 * 所以这里长得像生成页那张「已经挂好靠」的图：一组一块，组头是参数，
 * 下面左边是参考图缩略图、右边是这组的提示词。
 *
 * 预览与真正收录走的是**同一个 `intake::plan`**，故这里看见的就是确认之后
 * 会发生的那一份，不存在两套解析各说各话。
 */
export function IntakeConfirmModal({
  jobId,
  onClose,
  onConfirmed,
}: {
  jobId: number;
  onClose: () => void;
  onConfirmed: () => void;
}) {
  const [preview, setPreview] = useState<JobPreview | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let alive = true;
    void unwrap(commands.previewIntakeJob(jobId))
      .then((p) => {
        if (alive) setPreview(p);
      })
      .catch((e) => {
        if (alive) setError(String(e));
      });
    return () => {
      alive = false;
    };
  }, [jobId]);

  const confirm = async () => {
    setBusy(true);
    try {
      await unwrap(commands.confirmIntakeJob(jobId));
      toast.success("已确认开跑");
      onConfirmed();
      onClose();
    } catch (e) {
      toast.error(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      title="确认开跑这份工单"
      width="w700"
      onClose={onClose}
      headerExtra={
        preview ? (
          <span className="bdg b-amber">
            {preview.taskCount} 张 · 超过上限 {preview.threshold}
          </span>
        ) : undefined
      }
      footer={
        <>
          <span className="fs11 t3">
            确认前一条提示词、一张参考图都没进库；确认后整份生效并立即开始花额度
          </span>
          <div className="f1" />
          <button type="button" className="btn sm" onClick={onClose}>
            先不跑
          </button>
          <button
            type="button"
            className="btn pri sm"
            disabled={busy || !preview}
            onClick={() => void confirm()}
          >
            <PlayCircle className="ic12" />
            确认开跑 · {preview?.taskCount ?? "—"} 张
          </button>
        </>
      }
    >
      {error ? (
        <div className="fx ac gap8" style={{ color: "var(--er)" }}>
          <AlertTriangle className="ic12" />
          <span className="fs12">{error}</span>
        </div>
      ) : !preview ? (
        <div className="fs12 t3">正在读工单…</div>
      ) : (
        <>
          <div className="fs11 t3" style={{ lineHeight: 1.8, marginBottom: 12 }}>
            <span className="mono">{preview.jobId}</span> · {preview.groups.length} 个提示词组 ·
            会建 {preview.batchCount} 个批次
            {preview.batchCount > 1 && "（各组参数不同，塞不进同一批）"}
            <br />
            下面是这份工单里**提示词组与参考图的对应关系**，与开跑后实际生成的一致。
          </div>

          {preview.groups.map((g) => (
            <div key={g.name} className="ikgroup">
              <div className="fx ac gap8 wrap">
                <span className="fw6 fs13">{g.name}</span>
                {g.prefix && <span className="chip">{g.prefix}</span>}
                {g.purposes.map((p) => (
                  <span key={p} className="bdg b-amber">
                    {p}
                  </span>
                ))}
                <div className="f1" />
                <span className="fs11 t3 nowrap">
                  {g.prompts.length} 条 × {g.refs.length} 图{g.draws > 1 && ` × 抽卡 ${g.draws}`} ={" "}
                  <b style={{ color: "var(--acc2)" }}>{g.taskCount} 张</b>
                </span>
              </div>
              {/* 实际发往接口的字段 —— 全自动收录没经过生成页那张确认卡，
                  「到底发出去没有」在别处无处可查。 */}
              <div className="fs11 t3 mono mt4">{wireLabel(g.wireJson)}</div>

              <div className="ikbody mt8">
                <div className="ikrefs">
                  {g.refs.map((r) => (
                    <div key={r.fileName} className="ikref" title={r.fileName}>
                      {r.thumbDataUri ? (
                        <img src={r.thumbDataUri} alt={r.fileName} />
                      ) : (
                        <div className="ph" style={{ aspectRatio: 1, borderRadius: 8 }} />
                      )}
                      <div className="fs10 t3 nowrap ohide mono">{r.fileName}</div>
                    </div>
                  ))}
                </div>
                <div className="ikprompts">
                  {g.prompts.map((t, i) => (
                    <div key={`${g.name}-${i}`} className="ikprompt">
                      <span className="fs10 t3 mono noshrink">{i + 1}</span>
                      <span className="fs11">{t}</span>
                    </div>
                  ))}
                </div>
              </div>
            </div>
          ))}
        </>
      )}
    </Modal>
  );
}

/** 与设置页最近工单那一行同一口径：键名就是 multipart 字段名。 */
function wireLabel(wireJson: string): string {
  try {
    const wire = JSON.parse(wireJson) as Record<string, unknown>;
    const parts = Object.entries(wire).map(([k, v]) => `${k}=${String(v)}`);
    return parts.length ? parts.join(" · ") : "无显式参数（跟随模型默认）";
  } catch {
    return "";
  }
}
