import { cn } from "@/lib/utils";
import { X } from "lucide-react";
import { type ReactNode, useEffect } from "react";

/** 弹窗（原型 .ovl/.mdl）。Esc / 点击遮罩关闭。 */
export function Modal({
  title,
  width = "w420",
  onClose,
  children,
  footer,
  headerExtra,
}: {
  title: ReactNode;
  width?: "w360" | "w420" | "w640" | "w700";
  onClose: () => void;
  children: ReactNode;
  footer?: ReactNode;
  headerExtra?: ReactNode;
}) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onClose]);

  return (
    <div className="ovl" onClick={onClose}>
      <div className={cn("mdl", width)} onClick={(e) => e.stopPropagation()}>
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
  return (
    <div className="ovl" onClick={onClose}>
      <div className="mdl w360" onClick={(e) => e.stopPropagation()}>
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
          <button type="button" className="btn sm" onClick={onClose}>
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
