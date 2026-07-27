import type { ModelInfo } from "@/lib/ipc";
import { cn } from "@/lib/utils";

/** 一套生成参数（三件套要么都不给，要么给一套合法组合 —— 半套会被 CLI 拒）。 */
export interface Params {
  modelVersion: string;
  duration: number | null;
  videoResolution: string;
}

/**
 * 生成参数三件套的选择器。**受控组件，不自己存盘。**
 *
 * 它同时长在两处：提交确认卡里（改完当场重算这一批要花多少）与底栏的批量参数条。
 * 之所以不是「设置页里的一组下拉框」——参数是**每一批不一样**的东西：这一组要 vip
 * 1080p，下一组 4 秒 720p 就够。把它放进全局设置，等于每换一批都要去设置页改一次，
 * 改完还得记得改回来，而忘了改回来的代价是下一批按 5.5 倍的价钱跑掉。
 *
 * 换模型即清掉时长与分辨率：留着上一个模型的值必然撞新模型的约束，
 * 而那个报错会发生在**花钱之后**（CLI 拒绝在提交那一趟网络里）。
 */
export function V2vParamPicker({
  models,
  value,
  onChange,
  disabled,
  compact,
}: {
  models: ModelInfo[];
  value: Params;
  onChange: (p: Params) => void;
  disabled?: boolean;
  /** 紧凑排版（底栏用）。 */
  compact?: boolean;
}) {
  const picked = models.find((m) => m.modelVersion === value.modelVersion);
  const res = value.videoResolution || picked?.resolutions[0] || "";
  const dur = value.duration ?? picked?.minDuration ?? null;
  const perSec = picked?.resPrices.find((p) => p.resolution === res)?.creditPerSec ?? null;
  const unit = perSec != null && dur != null ? perSec * dur : null;

  return (
    <div className={cn("fx ac gap6 wrap", compact && "fs11")}>
      <select
        className="inp sm"
        style={{ minWidth: compact ? 150 : 190 }}
        value={value.modelVersion}
        disabled={disabled}
        onChange={(e) =>
          onChange({ modelVersion: e.target.value, duration: null, videoResolution: "" })
        }
      >
        <option value="">跟随全局默认</option>
        {models.map((m) => (
          <option key={m.modelVersion} value={m.modelVersion}>
            {m.modelVersion}
            {m.vip ? " · vip" : ""}
            {m.creditAtMin === null ? "" : ` · ${m.creditAtMin} 额度起`}
          </option>
        ))}
      </select>
      {picked && (
        <>
          <input
            className="inp sm"
            style={{ width: 78 }}
            type="number"
            min={picked.minDuration}
            max={picked.maxDuration}
            placeholder={`${picked.minDuration}–${picked.maxDuration}s`}
            value={value.duration ?? ""}
            disabled={disabled}
            onChange={(e) =>
              onChange({
                ...value,
                duration: e.target.value === "" ? null : Number(e.target.value),
              })
            }
          />
          <select
            className="inp sm"
            style={{ width: 92 }}
            value={res}
            disabled={disabled}
            onChange={(e) => onChange({ ...value, videoResolution: e.target.value })}
          >
            {picked.resolutions.map((r) => (
              <option key={r} value={r}>
                {r}
              </option>
            ))}
          </select>
          <span className={cn("fs11 nowrap", picked.vip ? "wr2" : "t3")}>
            {unit == null ? "单价未实测" : `${unit} 额度/条`}
            {picked.vip && " · vip 只买不排队"}
          </span>
        </>
      )}
      {!picked && value.modelVersion === "" && (
        <span className="fs11 t3 nowrap">用设置页里的默认值 —— 选一个模型可以只改这一批</span>
      )}
    </div>
  );
}
