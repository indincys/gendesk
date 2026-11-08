import { type PosterSettings, commands, unwrap } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { usePosterStore } from "@/stores/poster";
import { useEffect, useState } from "react";
import { toast } from "sonner";

const stateLabels: Record<string, string> = {
  preparing: "审稿处理中",
  awaiting: "待确认",
  skipped: "暂不导入",
  queued: "正在导入",
  done: "已完成",
  failed: "待重试",
};

export function PosterSection() {
  const settings = usePosterStore((s) => s.settings);
  const batches = usePosterStore((s) => s.batches);
  const refresh = usePosterStore((s) => s.refresh);
  const review = usePosterStore((s) => s.review);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    void refresh().catch((e) => toast.error(String(e)));
  }, [refresh]);

  const save = async (patch: Partial<PosterSettings>) => {
    if (!settings) return;
    setBusy(true);
    try {
      await unwrap(commands.updatePosterSettings({ ...settings, ...patch }));
      await refresh();
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const pick = async (kind: "app" | "library") => {
    if (!settings || busy) return;
    setBusy(true);
    try {
      const picked = await unwrap(
        kind === "app" ? commands.pickPosterApp() : commands.pickPosterLibrary(),
      );
      if (!picked) return;
      const changed = {
        ...settings,
        ...(kind === "app" ? { executable: picked } : { libraryRoot: picked }),
      };
      await unwrap(commands.updatePosterSettings(changed));
      await refresh();
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  if (!settings) return null;
  return (
    <section className="sec">
      <div className="sechead">
        <span className="fw6 fs13">Poster 图片接入</span>
        <span className="pcap">可选功能 · 默认关闭</span>
      </div>
      <div className="fx ac gap10">
        <span id="poster-opt-in" className="fs12 fw6 f1">
          审稿后询问导入 Poster
        </span>
        <button
          type="button"
          className={cn("sw", settings.enabled && "on")}
          role="switch"
          aria-checked={!!settings.enabled}
          aria-labelledby="poster-opt-in"
          disabled={busy}
          onClick={() => void save({ enabled: !settings.enabled })}
        >
          <i />
        </button>
      </div>
      <div className="fs11 t3 mt6" style={{ lineHeight: 1.8 }}>
        {settings.enabled
          ? "已开启：每批审稿通过后询问是否导入，确认后静默进入对应商品图片库。"
          : "关闭时只完成审稿，不弹出导入提示。需要时开启，再逐批选择是否导入。"}
      </div>
      {settings.enabled && (
        <>
          <div className="fs11 fw6 t3">Poster 应用</div>
          <div className="fx ac gap10 mt6">
            <div className="pathwell f1">{settings.executable}</div>
            <button
              type="button"
              className="btn sm"
              disabled={busy}
              onClick={() => void pick("app")}
            >
              选择应用
            </button>
          </div>
          <div className="fs11 t3 mt12">
            Poster 直接引用作品目录中的成品，不另建图片副本。已导入的成品请保留原文件。
          </div>
          {batches.length > 0 && (
            <div className="mt12">
              {batches.slice(0, 20).map((batch) => (
                <div className="mt6" key={batch.id}>
                  <div className="fx ac gap8">
                    <span className="fs12 f1">
                      {new Date(batch.createdAt * 1000).toLocaleString()} · {batch.count} 张 ·{" "}
                      {stateLabels[batch.state] ?? batch.state}
                    </span>
                    {batch.state === "awaiting" && (
                      <button type="button" className="btn sm" onClick={() => review(batch.id)}>
                        查看确认
                      </button>
                    )}
                  </div>
                  {batch.lastError && <div className="fs11 t2 mt6">{batch.lastError}</div>}
                </div>
              ))}
              {batches.some((b) => b.state === "failed") && (
                <button
                  type="button"
                  className="btn sm mt12"
                  onClick={async () => {
                    await unwrap(commands.retryPosterImports()).catch((e) =>
                      toast.error(String(e)),
                    );
                    toast("正在重试已确认的批次");
                  }}
                >
                  重试导入
                </button>
              )}
            </div>
          )}
        </>
      )}
    </section>
  );
}
