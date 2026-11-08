import { ConfirmModal } from "@/components/ui/Modal";
import { commands, unwrap } from "@/lib/ipc";
import { useEngineStore } from "@/stores/engine";
import { type ReactNode, useState } from "react";
import { toast } from "sonner";

/**
 * 「重启安装」按钮。生图任务正在跑时先确认：重启会中断它们、恢复后重新排队，
 * 已经发出去的那次请求可能照样计费。视频走远端队列，重启不影响。
 */
export function UpdateInstallButton({
  className,
  children,
}: {
  className: string;
  children: ReactNode;
}) {
  const running = useEngineStore((s) =>
    Object.values(s.summaries).reduce((n, b) => n + b.running, 0),
  );
  const version = useEngineStore((s) => s.updateVersion);
  const [confirming, setConfirming] = useState(false);

  const install = () =>
    void unwrap(commands.installUpdate()).catch((e) =>
      toast.error(e instanceof Error ? e.message : String(e)),
    );

  return (
    <>
      <button
        type="button"
        className={className}
        onClick={() => (running > 0 ? setConfirming(true) : install())}
      >
        {children}
      </button>
      {confirming && (
        <ConfirmModal
          title={`重启安装${version ? ` v${version}` : ""}`}
          desc={`还有 ${running} 张图正在生成。现在重启会中断它们，恢复后重新排队，已发出的请求可能重复计费。建议等这批跑完或先暂停队列再安装。`}
          confirmLabel="仍然重启"
          danger
          onConfirm={install}
          onClose={() => setConfirming(false)}
        />
      )}
    </>
  );
}
