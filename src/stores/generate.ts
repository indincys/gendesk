import { create } from "zustand";
import { persist } from "zustand/middleware";

/** React 风格更新量：可传新值或 (prev) => next。 */
type Updater<T> = T | ((prev: T) => T);
function apply<T>(u: Updater<T>, prev: T): T {
  return typeof u === "function" ? (u as (p: T) => T)(prev) : u;
}

/**
 * 生成页配置（E07）：选择的分组/参考图/挂靠 + 生成参数 + 抽卡次数。
 *
 * 铁律 1 说明：业务真相在 Rust；此处仅为「本次要生成什么」的 UI 选择态（尚未落库为批次），
 * 属允许前端持久化的 UI 态。持久化到 localStorage，切页/重启沿用；点「开始生成」后由
 * Rust createBatch 落库真相。挂靠记忆（E32）另在 Rust 侧按参考图落库，二者互补。
 */
interface GenerateState {
  selGroupIds: number[];
  selRefIds: number[];
  mapping: Record<number, number>;
  size: string | null;
  quality: string | null;
  draws: number;
  // 任务1 输出处理：去水印档位（V1 仅 "none"）+ 清除 AI 元数据 + 去除 C2PA。
  watermark: string;
  clearAiMetadata: boolean;
  removeC2pa: boolean;

  setSelGroupIds: (u: Updater<number[]>) => void;
  setSelRefIds: (u: Updater<number[]>) => void;
  setMapping: (u: Updater<Record<number, number>>) => void;
  setSize: (v: string | null) => void;
  setQuality: (v: string | null) => void;
  setDraws: (v: number) => void;
  setWatermark: (v: string) => void;
  setClearAiMetadata: (v: boolean) => void;
  setRemoveC2pa: (v: boolean) => void;
  /** E07 再来一批：用批次快照还原选择（分组集合从挂靠去重推导）。 */
  restoreFromBatch: (
    refs: { refImageId: number; promptGroupId: number }[],
    params: {
      size?: string | null;
      quality?: string | null;
      watermark?: string | null;
      clearAiMetadata?: boolean | null;
      removeC2pa?: boolean | null;
    },
  ) => void;
}

export const useGenerateStore = create<GenerateState>()(
  persist(
    (set) => ({
      selGroupIds: [],
      selRefIds: [],
      mapping: {},
      size: null,
      quality: null,
      draws: 1,
      watermark: "none",
      clearAiMetadata: true,
      removeC2pa: true,

      setSelGroupIds: (u) => set((s) => ({ selGroupIds: apply(u, s.selGroupIds) })),
      setSelRefIds: (u) => set((s) => ({ selRefIds: apply(u, s.selRefIds) })),
      setMapping: (u) => set((s) => ({ mapping: apply(u, s.mapping) })),
      setSize: (v) => set({ size: v }),
      setQuality: (v) => set({ quality: v }),
      setDraws: (v) => set({ draws: v }),
      setWatermark: (v) => set({ watermark: v }),
      setClearAiMetadata: (v) => set({ clearAiMetadata: v }),
      setRemoveC2pa: (v) => set({ removeC2pa: v }),
      restoreFromBatch: (refs, params) =>
        set({
          selRefIds: [...new Set(refs.map((r) => r.refImageId))],
          selGroupIds: [...new Set(refs.map((r) => r.promptGroupId))],
          mapping: Object.fromEntries(refs.map((r) => [r.refImageId, r.promptGroupId])),
          size: params.size ?? null,
          quality: params.quality ?? null,
          watermark: params.watermark ?? "none",
          // 缺省（旧批次快照无此字段）视为开启，与生成页默认一致。
          clearAiMetadata: params.clearAiMetadata ?? true,
          removeC2pa: params.removeC2pa ?? true,
          draws: 1,
        }),
    }),
    { name: "gendesk-generate" },
  ),
);
