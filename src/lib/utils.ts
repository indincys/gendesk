import { type ClassValue, clsx } from "clsx";
import { twMerge } from "tailwind-merge";

/** 合并 className —— shadcn/ui 约定的 cn 助手。 */
export function cn(...inputs: ClassValue[]): string {
  return twMerge(clsx(inputs));
}

/**
 * 提示词展示标签：有小标题时 `编号_小标题`，否则仅编号。
 * 用于提示词库 / 生成页 / 任务队列 / 废纸篓统一识别（需求 2）。
 */
export function promptLabel(code: string | null | undefined, title?: string | null): string {
  const c = code ?? "";
  const t = title?.trim();
  return t ? `${c}_${t}` : c;
}
