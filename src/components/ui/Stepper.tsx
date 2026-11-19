import { cn } from "@/lib/utils";
import { useEffect, useState } from "react";

/** 数字步进器（原型 .stp）。 */
export function Stepper({
  label,
  value,
  min,
  max,
  onChange,
}: {
  label: string;
  value: number;
  min: number;
  max: number;
  onChange: (v: number) => void;
}) {
  const [text, setText] = useState(String(value));
  useEffect(() => setText(String(value)), [value]);
  const commit = () => {
    const parsed = text.trim() ? Number(text) : Number.NaN;
    const next = Number.isFinite(parsed) ? Math.min(max, Math.max(min, Math.round(parsed))) : value;
    setText(String(next));
    if (next !== value) onChange(next);
  };
  return (
    <span className="stp">
      <button
        type="button"
        className="stpb"
        aria-label={`减少${label}`}
        onClick={() => onChange(Math.max(min, value - 1))}
        disabled={value <= min}
      >
        −
      </button>
      <input
        type="number"
        className="stpv"
        value={text}
        min={min}
        max={max}
        aria-label={label}
        title={`${label}，${min}–${max}`}
        onChange={(event) => setText(event.target.value)}
        onBlur={commit}
        onKeyDown={(event) => {
          if (event.key === "Enter") {
            event.preventDefault();
            event.currentTarget.blur();
          }
        }}
      />
      <button
        type="button"
        className="stpb"
        aria-label={`增加${label}`}
        onClick={() => onChange(Math.min(max, value + 1))}
        disabled={value >= max}
      >
        +
      </button>
    </span>
  );
}

/** 开关（原型 .sw）。 */
export function Toggle({
  label,
  on,
  onClick,
  disabled,
}: { label: string; on: boolean; onClick: () => void; disabled?: boolean }) {
  return (
    <button
      type="button"
      className={cn("sw", on && "on")}
      onClick={onClick}
      disabled={disabled}
      role="switch"
      aria-label={label}
      aria-checked={on}
    >
      <i />
    </button>
  );
}
