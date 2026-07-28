import { Modal } from "@/components/ui/Modal";
import { assetSrc } from "@/lib/img";
import {
  type JobPreview,
  type JobPreviewRef,
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
 * **哪个提示词组配了哪几张参考图**——配错的代价是整批图跑出来全是错的，
 * 而那要到验收时才看得出来，那时钱已经花完了。
 *
 * 所以这里长得像生成页那张「已经挂好靠」的图：一组一块，组头是参数，
 * 下面左边是参考图缩略图、右边是这组的提示词。
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
                    <LazyRef key={r.path} jobId={jobId} refItem={r} />
                  ))}
                </div>
                <PromptList groupName={g.name} prompts={g.prompts} />
              </div>
            </div>
          ))}
        </>
      )}
    </Modal>
  );
}

/** 一张参考图：进了视口才去要缩略图。 */
function LazyRef({ jobId, refItem }: { jobId: number; refItem: JobPreviewRef }) {
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
        void unwrap(commands.intakeRefThumb(jobId, refItem.path))
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
  }, [jobId, refItem.path]);

  return (
    <div ref={boxRef} className="ikref" title={refItem.fileName}>
      {src ? (
        <img src={src} alt={refItem.fileName} loading="lazy" decoding="async" />
      ) : (
        <div className="ph" style={{ aspectRatio: 1, borderRadius: 8 }} />
      )}
      <div className="fs10 t3 nowrap ohide mono">{refItem.fileName}</div>
    </div>
  );
}

/**
 * 一组的提示词。默认只铺前三条。
 *
 * 要核的是「这组配了哪几张图」，不是把 120 组 × 每组几千字全部读一遍；
 * 一次性铺开 600 段长叙事只会把要核的东西埋掉。
 */
const PROMPT_PREVIEW = 3;

function PromptList({ groupName, prompts }: { groupName: string; prompts: string[] }) {
  const [all, setAll] = useState(false);
  const shown = all ? prompts : prompts.slice(0, PROMPT_PREVIEW);
  return (
    <div className="ikprompts">
      {shown.map((t, i) => (
        // biome-ignore lint/suspicious/noArrayIndexKey: 组内提示词允许重复，文本不能当键；这份列表只从尾部增减、永不重排，位置就是身份。
        <div key={`${groupName}-${i}`} className="ikprompt">
          <span className="fs10 t3 mono noshrink">{i + 1}</span>
          <span className="fs11">{t}</span>
        </div>
      ))}
      {prompts.length > PROMPT_PREVIEW && (
        <button type="button" className="ikmore" onClick={() => setAll((v) => !v)}>
          {all ? "收起" : `展开全部 ${prompts.length} 条`}
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
