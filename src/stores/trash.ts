import { create } from "zustand";
import { persist } from "zustand/middleware";

/** 网格「每行大约几张」的可选范围。越小图越大。 */
export const TRASH_SIZE_MIN = 3;
export const TRASH_SIZE_MAX = 9;

/**
 * 废纸篓的两个显示偏好。
 *
 * 铁律 1 说明：这里没有任何业务真相，纯粹是「我习惯怎么看这一页」。放 store 并持久化，
 * 是因为它的寿命必须比这一页的挂载长 —— 一次排查里人会在废纸篓与作品库之间来回好几趟，
 * 每回来一次就要重新把滑块拖到自己惯用的大小，那个滑块就等于没有。
 */
interface TrashUiState {
  mode: "grid" | "list";
  /** 每行目标张数（`TRASH_SIZE_MIN`–`TRASH_SIZE_MAX`）。 */
  size: number;
  setMode: (m: "grid" | "list") => void;
  setSize: (n: number) => void;
}

export const useTrashUiStore = create<TrashUiState>()(
  persist(
    (set) => ({
      mode: "grid",
      size: 5,
      setMode: (mode) => set({ mode }),
      setSize: (n) => set({ size: clampSize(n) }),
    }),
    {
      name: "gendesk-trash-ui",
      // 落盘的值来自上一版的取值范围。改过范围之后它会落在界外，而一个界外的
      // 目标张数会算出荒唐的行高 —— 夹取必须在**回填时**做，光夹滑块拦不住它。
      onRehydrateStorage: () => (s) => {
        if (s) s.size = clampSize(s.size);
      },
    },
  ),
);

function clampSize(n: number): number {
  // 先判有限再夹：`Math.max(3, NaN)` 回的是 NaN，而它会顺着「目标行高 → 每张宽度」
  // 一路污染下去，整页当场塌掉（同 `justified.ts` 里 `safeRatio` 那一课）。
  if (!Number.isFinite(n)) return 5;
  return Math.min(TRASH_SIZE_MAX, Math.max(TRASH_SIZE_MIN, Math.round(n)));
}
