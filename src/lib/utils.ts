import { type ClassValue, clsx } from "clsx";
import { twMerge } from "tailwind-merge";

/** 合并 className —— shadcn/ui 约定的 cn 助手。 */
export function cn(...inputs: ClassValue[]): string {
  return twMerge(clsx(inputs));
}
