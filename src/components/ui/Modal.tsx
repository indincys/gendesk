import { cn } from "@/lib/utils";
import { X } from "lucide-react";
import { type KeyboardEvent, type ReactNode, useLayoutEffect, useRef } from "react";

/** 弹窗（原型 .ovl/.mdl）。Esc / 点击遮罩关闭。 */
export function Modal({
  id,
  title,
  width = "w420",
  onClose,
  children,
  footer,
  headerExtra,
}: {
  id?: string;
  title: ReactNode;
  width?: "w360" | "w420" | "w640" | "w700";
  onClose: () => void;
  children: ReactNode;
  footer?: ReactNode;
  headerExtra?: ReactNode;
}) {
  const box = useDialogFocus();

  return (
    <div id={id} className="ovl" onClick={onClose}>
      <div
        ref={box}
        className={cn("mdl", width)}
        onClick={(e) => e.stopPropagation()}
        data-gendesk-dialog
        tabIndex={-1}
        role="dialog"
        aria-modal="true"
        aria-label={typeof title === "string" ? title : undefined}
        onKeyDown={(event) => handleDialogKeys(event, onClose)}
      >
        <div className="mhead">
          <span className="fw6 fs13">{title}</span>
          {headerExtra}
          <div className="f1" />
          <button type="button" className="icb" onClick={onClose} aria-label="关闭">
            <X className="ic12" />
          </button>
        </div>
        <div className="mbody">{children}</div>
        {footer && <div className="mfoot">{footer}</div>}
      </div>
    </div>
  );
}

/** 确认弹窗（原型 .w360）。 */
export function ConfirmModal({
  title,
  desc,
  confirmLabel = "确定",
  danger,
  onConfirm,
  onClose,
}: {
  title: string;
  desc: string;
  confirmLabel?: string;
  danger?: boolean;
  onConfirm: () => void;
  onClose: () => void;
}) {
  const box = useDialogFocus();
  return (
    <div className="ovl" onClick={onClose}>
      <div
        ref={box}
        className="mdl w360"
        onClick={(e) => e.stopPropagation()}
        data-gendesk-dialog
        tabIndex={-1}
        role="alertdialog"
        aria-modal="true"
        aria-label={title}
        onKeyDown={(event) => handleDialogKeys(event, onClose)}
      >
        <div className="mbody" style={{ padding: "18px 18px 4px" }}>
          <div className="fw6 fs13">{title}</div>
          <div className="fs12 t2 mt6" style={{ lineHeight: 1.7 }}>
            {desc}
          </div>
        </div>
        <div
          className="mfoot"
          style={{ borderTop: "none", background: "var(--panel)", borderRadius: "0 0 13px 13px" }}
        >
          <div className="f1" />
          <button type="button" className="btn sm" data-initial-focus onClick={onClose}>
            取消
          </button>
          <button
            type="button"
            className={cn("btn sm", danger && "dng")}
            onClick={() => {
              onConfirm();
              onClose();
            }}
          >
            {confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
}

function focusable(element: HTMLElement): HTMLElement[] {
  return [
    ...element.querySelectorAll<HTMLElement>(
      'button:not([disabled]), a[href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex="0"]',
    ),
  ].filter((node) => node.getClientRects().length > 0);
}

function useDialogFocus() {
  const ref = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const element = ref.current;
    if (!element) return;
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const frame = requestAnimationFrame(() => {
      if (!element.contains(document.activeElement)) {
        (
          element.querySelector<HTMLElement>("[data-initial-focus]") ??
          focusable(element)[0] ??
          element
        ).focus();
      }
    });
    return () => {
      cancelAnimationFrame(frame);
      if (previous?.isConnected && !element.contains(previous)) previous.focus();
    };
  }, []);
  return ref;
}

function handleDialogKeys(event: KeyboardEvent<HTMLDivElement>, onClose: () => void) {
  if (event.key === "Escape") {
    event.preventDefault();
    event.stopPropagation();
    onClose();
  } else if (event.key === "Tab") {
    const elements = focusable(event.currentTarget);
    const first = elements[0] ?? event.currentTarget;
    const last = elements.at(-1) ?? first;
    if (
      event.shiftKey &&
      (document.activeElement === first || document.activeElement === event.currentTarget)
    ) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  }
}
