import { isTauri } from "@/lib/ipc";
import { convertFileSrc } from "@tauri-apps/api/core";
import type React from "react";

/**
 * 本地图片路径 → 可用于 <img src> 的 asset 协议 URL（技术文档 3.4）。
 * 浏览器预览（非 Tauri）下返回 undefined，由调用方回退占位样式。
 */
export function assetSrc(path?: string | null): string | undefined {
  if (!path || !isTauri()) return undefined;
  return convertFileSrc(path);
}

/**
 * 本地图片路径 → 铺满容器的背景样式（缩略图格子用）。
 *
 * 四个页面各抄过一份一模一样的实现。它们看着无害，直到有一天要改 `cover` → `contain`
 * 或加一层占位色 —— 那时四处里改对三处，剩下那一页会安静地不一样。
 * 拿不到路径就返回空对象，由 CSS 上的占位样式接手。
 */
export function bg(path?: string | null): React.CSSProperties {
  const src = assetSrc(path);
  return src
    ? { backgroundImage: `url(${src})`, backgroundSize: "cover", backgroundPosition: "center" }
    : {};
}
