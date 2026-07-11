import { assetSrc } from "@/lib/img";
import { cn } from "@/lib/utils";

/**
 * 自然比例缩略图（任务3）：按原图比例铺满列宽，供验收/作品库/参考图库瀑布流网格使用，
 * 取代统一正方形裁切。无缩略时退化为方形占位。`className` 传入原图框类
 * （如 `rcimg rcnat` / `wcimg wcnat`）以沿用选中/焦点等视觉钩子。
 */
export function NatThumb({ path, className }: { path?: string | null; className?: string }) {
  const src = assetSrc(path);
  return (
    <div className={cn("ph", className)}>
      {src ? (
        <img className="thumbimg" src={src} alt="" loading="lazy" />
      ) : (
        <div className="thumbfill" />
      )}
    </div>
  );
}
