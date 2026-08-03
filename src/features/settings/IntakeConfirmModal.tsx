import { Modal } from "@/components/ui/Modal";
import { assetSrc } from "@/lib/img";
import {
  type JobPreview,
  type JobPreviewItem,
  commands,
  subscribeIntakeProgress,
  unwrap,
} from "@/lib/ipc";
import { AlertTriangle, PlayCircle } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { toast } from "sonner";

/**
 * 超阈值工单的开跑确认卡 —— **把挂靠关系摆出来，而不是摆一个数字**。
 *
 * 阈值是自动收录链路上唯一一处「停下来等人点头」的地方。原来那句
 * 「600 张，去设置页确认开跑」并不足以让人做判断：真正要核的是
 * **哪条词配了哪张图、会叫什么名字**——配错的代价是整批图跑出来全是错的，
 * 而那要到验收时才看得出来，那时钱已经花完了。
 *
 * 工单 v2 里一个条目 = 一条词 = 一个任务 = 一张图，故这里一条一块：
 * 左边是它挂的那张参考图，右边是语义名 / SKU / 参数 / 正文。
 *
 * 预览与真正收录走的是**同一个 `intake::plan`**，故这里看见的就是确认之后
 * 会发生的那一份，不存在两套解析各说各话。
 *
 * ## 缩略图为什么是懒加载的
 *
 * 一份 120 张 26 MP 相机图的工单，原来打开这张卡会把 767 MB 的图全部解码一遍
 * 再塞成 base64 —— 弹窗停在「正在读工单…」一两分钟，而后端每关一次再开一次
 * 就多挂一条停不下来的解码线程（实测 600% CPU / 608 MB RSS）。
 *
 * 现在每张图进了视口才去要，且后端有磁盘缓存 —— 滚回去是零成本，重开也是。
 * 组件卸载就不再发起新请求，这就是这条链路上「取消」能做到的全部：
 * 后端那次 `spawn_blocking` 本来就不可取消，所以真正的答案是**让多余的活
 * 根本不要开始**，而不是开始之后去追。
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
  const [progress, setProgress] = useState<{ done: number; total: number } | null>(null);

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

  // 确认之后 IPC 立刻返回，真正的收录在后台跑。这条订阅是那几十秒里唯一的动静。
  useEffect(() => {
    let cleanup: (() => void) | undefined;
    void subscribeIntakeProgress((e) => {
      setProgress({ done: e.done, total: e.total });
    }).then((fn) => {
      cleanup = fn;
    });
    return () => cleanup?.();
  }, []);

  const confirm = async () => {
    setBusy(true);
    try {
      await unwrap(commands.confirmIntakeJob(jobId));
      // 收录在后台继续，结果由 `intake://changed` 那条 toast 兜底报出来。
      toast.success("已确认开跑，正在导入…");
      onConfirmed();
      onClose();
    } catch (e) {
      toast.error(String(e));
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
            {busy && progress
              ? `正在导入参考图 ${progress.done}/${progress.total}…`
              : "确认前一条提示词、一张参考图都没进库；确认后整份生效并立即开始花额度"}
          </span>
          <div className="f1" />
          <button type="button" className="btn sm" onClick={onClose} disabled={busy}>
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
            <span className="mono">{preview.jobId}</span> · {preview.items.length} 个条目 ·{" "}
            {preview.refCount} 张参考图 · 会建 1 个批次（一次工单 = 一个 run）
            {preview.purpose && <span className="bdg b-amber">{preview.purpose}</span>}
            <br />
            下面是**每条词配了哪张图、会叫什么名字**，与开跑后实际生成的一致。
          </div>

          {preview.items.map((it) => (
            <div key={it.name} className="ikgroup">
              <div className="fx ac gap8 wrap">
                <span className="fw6 fs13">{it.name}</span>
                <span className="chip">{it.style}</span>
                <span className="chip">{it.sku}</span>
                <div className="f1" />
                <span className="fs11 t3 nowrap mono">{it.refFileName}</span>
              </div>
              {/* 实际发往接口的字段 —— 全自动收录没经过生成页那张确认卡，
                  「到底发出去没有」在别处无处可查。 */}
              <div className="fs11 t3 mono mt4">{wireLabel(it.wireJson)}</div>

              <div className="ikbody mt8">
                <div className="ikrefs">
                  <LazyRef jobId={jobId} item={it} />
                </div>
                <PromptBody text={it.prompt} />
              </div>
            </div>
          ))}
        </>
      )}
    </Modal>
  );
}

/** 一张参考图：进了视口才去要缩略图。 */
function LazyRef({ jobId, item }: { jobId: number; item: JobPreviewItem }) {
  const [src, setSrc] = useState<string | null>(null);
  const boxRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    const el = boxRef.current;
    if (!el) return;
    let alive = true;
    // rootMargin 提前一屏：滚动时图已经在那儿了，而不是滚到了才开始转。
    const io = new IntersectionObserver(
      (entries) => {
        if (!entries.some((e) => e.isIntersecting)) return;
        io.disconnect();
        void unwrap(commands.intakeRefThumb(jobId, item.refPath))
          .then((p) => {
            if (alive) setSrc(assetSrc(p) ?? null);
          })
          // 一张图读不出来不该让整份确认卡打不开：留占位框，其余照常。
          .catch(() => {});
      },
      { rootMargin: "300px" },
    );
    io.observe(el);
    return () => {
      alive = false;
      io.disconnect();
    };
  }, [jobId, item.refPath]);

  return (
    <div ref={boxRef} className="ikref" title={item.refFileName}>
      {src ? (
        <img src={src} alt={item.refFileName} loading="lazy" decoding="async" />
      ) : (
        <div className="ph" style={{ aspectRatio: 1, borderRadius: 8 }} />
      )}
      <div className="fs10 t3 nowrap ohide mono">{item.refFileName}</div>
    </div>
  );
}

/**
 * 一条提示词正文。长的先截断。
 *
 * 要核的是「这条词配了哪张图、会叫什么名字」，不是把 120 条几千字的长叙事
 * 全部读一遍；一次性铺开只会把要核的东西埋掉。
 */
const PROMPT_CLAMP = 160;

function PromptBody({ text }: { text: string }) {
  const [all, setAll] = useState(false);
  const long = text.length > PROMPT_CLAMP;
  return (
    <div className="ikprompts">
      <div className="ikprompt">
        <span className="fs11">{all || !long ? text : `${text.slice(0, PROMPT_CLAMP)}…`}</span>
      </div>
      {long && (
        <button type="button" className="ikmore" onClick={() => setAll((v) => !v)}>
          {all ? "收起" : `展开全文（${text.length} 字）`}
        </button>
      )}
    </div>
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
