import { isTauri } from "@/lib/ipc";
import { convertFileSrc } from "@tauri-apps/api/core";

/**
 * 本地图片路径 → 可用于 <img src> 的 asset 协议 URL（技术文档 3.4）。
 * 浏览器预览（非 Tauri）下返回 undefined，由调用方回退占位样式。
 */
export function assetSrc(path?: string | null): string | undefined {
  if (!path || !isTauri()) return undefined;
  return convertFileSrc(path);
}
