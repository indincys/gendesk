import { Modal } from "@/components/ui/Modal";
import { assetSrc } from "@/lib/img";
import { commands, unwrap } from "@/lib/ipc";
import { Ellipsis, FolderOpen } from "lucide-react";
import { type ReactNode, useEffect, useRef, useState } from "react";
import { toast } from "sonner";

export async function copyText(value: string, label: string) {
  try {
    await navigator.clipboard.writeText(value);
    toast.success(`已复制${label}`);
  } catch (error) {
    toast.error(`复制失败：${String(error)}`);
  }
}

/** 返回未成功导出的原件路径，取消时保留当前选择。 */
export async function exportOriginals(paths: string[]): Promise<string[] | null> {
  if (paths.length === 0) return null;
  try {
    const dir = await unwrap(commands.pickOutputDir());
    if (!dir) return null;
    const result = await unwrap(commands.exportMediaFiles(paths, dir));
    const failed = result.filter((item) => item.error !== null);
    const exported = result.length - failed.length;
    const message = `已导出 ${exported}/${result.length} 个原件`;
    const action =
      exported > 0 ? { label: "在文件管理器中显示", onClick: () => reveal(dir) } : undefined;
    if (failed.length > 0) {
      toast.warning(message, {
        action,
        description: failed
          .slice(0, 6)
          .map((item) => `${item.sourcePath.split(/[/\\\\]/).at(-1)} · ${item.error}`)
          .join("\n"),
      });
    } else {
      toast.success(message, { action });
    }
    return failed.map((item) => item.sourcePath);
  } catch (error) {
    toast.error(`导出失败：${String(error)}`);
    return null;
  }
}

function reveal(path: string) {
  void unwrap(commands.openPathInFolder(path)).catch((error) =>
    toast.error(`无法在文件管理器显示：${String(error)}`),
  );
}

export function MediaActions({
  path,
  label = "原件",
  paths,
  onPreview,
  children,
}: {
  path: string;
  label?: string;
  paths?: string[] | undefined;
  onPreview?: () => void;
  children?: ReactNode;
}) {
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const first = useRef<HTMLButtonElement>(null);
  const [visible, setVisible] = useState(false);

  useEffect(() => {
    if (!visible) return;
    first.current?.focus();
    const dismiss = (event: PointerEvent) => {
      if (!root.current?.contains(event.target as Node)) setVisible(false);
    };
    document.addEventListener("pointerdown", dismiss);
    return () => document.removeEventListener("pointerdown", dismiss);
  }, [visible]);

  function act(action: () => void) {
    setVisible(false);
    trigger.current?.focus();
    action();
  }

  const openFile = () =>
    void unwrap(commands.openMedia(path)).catch((error) =>
      toast.error(`打开原件失败：${String(error)}`),
    );

  return (
    <div
      className={children ? "media-actions media-with-preview" : "media-actions"}
      ref={root}
      onContextMenu={(event) => {
        event.preventDefault();
        event.stopPropagation();
        setVisible(true);
      }}
      onKeyDown={(event) => {
        if (event.key === "ContextMenu" || (event.shiftKey && event.key === "F10")) {
          event.preventDefault();
          event.stopPropagation();
          setVisible(true);
        }
        if (visible && event.key === "Escape") {
          event.preventDefault();
          event.stopPropagation();
          setVisible(false);
          trigger.current?.focus();
        }
      }}
    >
      {children}
      <button
        ref={trigger}
        type="button"
        className="btn xs"
        aria-label={`${label}文件操作`}
        title={`${label}文件操作`}
        aria-expanded={visible}
        onClick={(event) => {
          event.stopPropagation();
          setVisible((value) => !value);
        }}
        onDoubleClick={(event) => event.stopPropagation()}
      >
        <Ellipsis className="ic12" />
      </button>
      {visible && (
        <div
          className="media-actions-popup"
          role="group"
          aria-label={`${label}操作`}
          onClick={(event) => event.stopPropagation()}
        >
          {onPreview && (
            <button ref={first} type="button" onClick={() => act(onPreview)}>
              预览原图
            </button>
          )}
          <button ref={onPreview ? undefined : first} type="button" onClick={() => act(openFile)}>
            用系统应用打开
          </button>
          <button type="button" onClick={() => act(() => reveal(path))}>
            <FolderOpen className="ic12" />
            在文件管理器中显示
          </button>
          <button type="button" onClick={() => act(() => void copyText(path, "路径"))}>
            复制文件路径
          </button>
          <button type="button" onClick={() => act(() => void exportOriginals(paths ?? [path]))}>
            {paths && paths.length > 1 ? `导出所选原件 · ${paths.length} 个` : "导出原件…"}
          </button>
        </div>
      )}
    </div>
  );
}

export function OriginalPreview({
  path,
  title,
  onClose,
}: {
  path: string;
  title: string;
  onClose: () => void;
}) {
  const [actualSize, setActualSize] = useState(false);
  const [failed, setFailed] = useState(false);
  return (
    <Modal title={title} width="w700" onClose={onClose} headerExtra={<MediaActions path={path} />}>
      {failed ? (
        <div className="bigempty">原件无法预览，可尝试用系统应用打开。</div>
      ) : (
        <div className="original-preview">
          <button
            type="button"
            className="btn xs"
            aria-pressed={actualSize}
            onClick={() => setActualSize((value) => !value)}
          >
            {actualSize ? "适应窗口" : "原始尺寸"}
          </button>
          <img
            src={assetSrc(path) ?? undefined}
            alt={title}
            style={actualSize ? undefined : { maxWidth: "100%", maxHeight: "62vh" }}
            onError={() => setFailed(true)}
          />
        </div>
      )}
    </Modal>
  );
}
