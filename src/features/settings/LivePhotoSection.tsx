import { Toggle } from "@/components/ui/Stepper";
import { type LivePhotoStatus, commands, subscribeLivePhoto, unwrap } from "@/lib/ipc";
import { FolderOpen } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { toast } from "sonner";

export function LivePhotoSection() {
  const [status, setStatus] = useState<LivePhotoStatus | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(() => unwrap(commands.getLivePhotoStatus()).then(setStatus), []);

  useEffect(() => {
    const subscription = subscribeLivePhoto(() => void load().catch(() => {}));
    void subscription.then(load).catch(() => {});
    return () => void subscription.then((unlisten) => unlisten());
  }, [load]);

  const update = async (action: () => Promise<unknown>) => {
    if (busy) return;
    setBusy(true);
    try {
      await action();
      await load();
    } catch (error) {
      toast.error(String(error));
    } finally {
      setBusy(false);
    }
  };

  if (!status?.supported) return null;

  return (
    <section className="sec">
      <div className="sechead">
        <span className="fw6 fs13">实况照片</span>
        <div className="f1" />
        <button
          type="button"
          className="btn sm gho"
          onClick={() =>
            void unwrap(commands.openLivePhotoDir()).catch((e) => toast.error(String(e)))
          }
        >
          <FolderOpen className="ic12" />
          打开实况照片目录
        </button>
      </div>
      <div className="fx ac gap10 wrap">
        <Toggle
          label="验收通过后自动转实况图"
          on={status.enabled}
          onClick={() => void update(() => unwrap(commands.setLivePhotoEnabled(!status.enabled)))}
        />
        <span className="fs12 t2">验收通过后自动转实况图</span>
        {status.pending > 0 && <span className="fs11 t3">{status.pending} 张等待或正在转换</span>}
        {status.failed > 0 && (
          <button
            type="button"
            className="btn sm"
            disabled={busy}
            onClick={() => void update(() => unwrap(commands.retryLivePhotos()))}
          >
            重试 {status.failed} 张失败图片
          </button>
        )}
      </div>
      <div className="fs11 t3 mt6">
        新验收的图片由本机 make_live.py 后台转换。同一生成批次的 JPG 和 MOV 成对放在「成品 /
        实况照片」下，文件夹按提交名称和批次编号命名。
      </div>
      {status.error && <div className="fs11 t3 mt6">转换失败 · {status.error}</div>}
    </section>
  );
}
