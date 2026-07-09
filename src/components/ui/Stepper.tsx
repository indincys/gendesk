import { cn } from "@/lib/utils";

/** 数字步进器（原型 .stp）。 */
export function Stepper({
  value,
  min,
  max,
  onChange,
}: {
  value: number;
  min: number;
  max: number;
  onChange: (v: number) => void;
}) {
  return (
    <span className="stp">
      <button
        type="button"
        className="stpb"
        onClick={() => onChange(Math.max(min, value - 1))}
        disabled={value <= min}
      >
        −
      </button>
      <span className="stpv">{value}</span>
      <button
        type="button"
        className="stpb"
        onClick={() => onChange(Math.min(max, value + 1))}
        disabled={value >= max}
      >
        +
      </button>
    </span>
  );
}

/** 开关（原型 .sw）。 */
export function Toggle({ on, onClick }: { on: boolean; onClick: () => void }) {
  return (
    <span
      className={cn("sw", on && "on")}
      onClick={onClick}
      onKeyDown={(e) => (e.key === "Enter" || e.key === " ") && onClick()}
      role="switch"
      aria-checked={on}
      tabIndex={0}
    >
      <i />
    </span>
  );
}
