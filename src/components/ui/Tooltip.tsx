import { CircleHelp } from "lucide-react";
import {
  type FocusEvent,
  type ReactNode,
  useCallback,
  useEffect,
  useId,
  useRef,
  useState,
} from "react";
import { createPortal } from "react-dom";

export function Tooltip({ content, children }: { content: ReactNode; children: ReactNode }) {
  const id = useId();
  const wrapRef = useRef<HTMLSpanElement>(null);
  const [open, setOpen] = useState(false);
  const [position, setPosition] = useState({ left: 0, top: 0, below: false });
  const place = useCallback(() => {
    const rect = wrapRef.current?.getBoundingClientRect();
    if (!rect) return;
    const below = rect.top < 96;
    setPosition({
      left: Math.max(170, Math.min(window.innerWidth - 170, rect.left + rect.width / 2)),
      top: below ? rect.bottom + 8 : rect.top - 8,
      below,
    });
  }, []);

  useEffect(() => {
    if (!open) return;
    place();
    window.addEventListener("resize", place);
    window.addEventListener("scroll", place, true);
    return () => {
      window.removeEventListener("resize", place);
      window.removeEventListener("scroll", place, true);
    };
  }, [open, place]);

  const closeAfterBlur = (event: FocusEvent<HTMLSpanElement>) => {
    if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setOpen(false);
  };

  return (
    <span
      className="tipwrap"
      ref={wrapRef}
      onMouseEnter={() => {
        place();
        setOpen(true);
      }}
      onMouseLeave={() => setOpen(false)}
      onFocusCapture={() => {
        place();
        setOpen(true);
      }}
      onBlurCapture={closeAfterBlur}
    >
      {children}
      {open &&
        createPortal(
          <span
            id={id}
            className="tipbubble"
            role="tooltip"
            style={{
              left: position.left,
              top: position.top,
              transform: position.below ? "translate(-50%, 0)" : "translate(-50%, -100%)",
            }}
          >
            {content}
          </span>,
          document.body,
        )}
    </span>
  );
}

export function DescriptionHint({
  label,
  children,
}: {
  label: string;
  children: ReactNode;
}) {
  return (
    <Tooltip content={children}>
      <button type="button" className="hintbtn" aria-label={label}>
        <CircleHelp className="ic12" />
      </button>
    </Tooltip>
  );
}
