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

/**
 * 拼音首字母排序比较器（提示词组排序）。
 * 用 zh 排序规则的 `Intl.Collator`：中文按拼音升序（阿→bo→...），数字自然序，
 * 英文/符号回退到本地字母序。WebView2（Chromium）与 WKWebView 均自带完整 ICU，
 * 结果稳定。纯展示排序，不涉及业务真相，放前端合规。
 */
const zhCollator = new Intl.Collator("zh-Hans-CN", { numeric: true, sensitivity: "base" });
export function pinyinCompare(a: string, b: string): number {
  return zhCollator.compare(a, b);
}

/** 按拼音首字母升序排序提示词分组（返回新数组，不改原数组）。 */
export function sortGroupsByPinyin<T extends { name: string }>(groups: T[]): T[] {
  return [...groups].sort((x, y) => pinyinCompare(x.name, y.name));
}
