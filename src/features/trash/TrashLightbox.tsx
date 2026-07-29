import { dayLabel, hhmm } from "@/features/_shared/timeline";
import { entityLabel } from "@/features/trash/TrashRows";
import { assetSrc } from "@/lib/img";
import type { TrashItemView } from "@/lib/ipc";
import { promptLabel } from "@/lib/utils";
import { ChevronLeft, ChevronRight, Trash2, Undo2, X } from "lucide-react";

/**
 * 放大查看 —— 铺满一屏，左右换条，不离开这一屏就能判并处置。
 *
 * ## 为什么要有它
 *
 * 排除误删要看清细节：一张 200px 宽的格子看不出「这张脸糊没糊」，
 * 而那正是当初判它不通过的理由，也是现在要重新判一次的依据。
 *
 * ## 为什么按钮在这里而不是回到网格里点
 *
 * 判完一条紧接着就是处置它。回退一次再点一次，等于把每条的动作数翻倍 ——
 * 而人来这一页往往要一口气过几十条。
 *
 * 图取**原图**优先（`imagePath`）：缩略图放大到全屏就是一团马赛克，
 * 而这一屏存在的全部理由就是看清楚。原图没了才退回缩略图。
 */
export function TrashLightbox({
  item,
  index,
  total,
  now,
  onSeek,
  onRestore,
  onPurge,
  onClose,
}: {
  item: TrashItemView;
  /** 从 0 起的全局序号 —— 标题上写的是「第 n / 共 m」。 */
  index: number;
  total: number;
  now: number;
  onSeek: (dir: 1 | -1) => void;
  onRestore: () => void;
  onPurge: () => void;
  onClose: () => void;
}) {
  const src = assetSrc(item.imagePath) ?? assetSrc(item.thumbPath);
  const title = item.code ? promptLabel(item.code, item.title) : entityLabel(item);

  return (
    <div className="trlb" onClick={onClose}>
      <div className="trlbhd" onClick={(e) => e.stopPropagation()}>
        <span className="fw6 fs13">{title}</span>
        <span className="bdg b-gray">{item.sourceLabel}</span>
        <span className="fs11 t3">
          {dayLabel(item.deletedAt, now)} {hhmm(item.deletedAt)}
        </span>
        <div className="f1" />
        <span className="mono fs11 t3">
          {index + 1} / {total}
        </span>
        <button type="button" className="icb" title="Esc" onClick={onClose}>
          <X className="ic12" />
        </button>
      </div>

      <div className="trlbstage" onClick={onClose}>
        {src ? (
          <img
            src={src}
            alt="被丢弃的内容（铺满查看）"
            onClick={(e) => e.stopPropagation()}
            // 点图本身不关闭：放大之后人会想点着看细节，一点就退出等于这一屏没法用。
          />
        ) : (
          <div className="fs12 trlbgone">原图已随上一次清理删除，只剩这条记录。</div>
        )}
        {total > 1 && (
          <>
            <button
              type="button"
              className="trlbnav l"
              title="上一条（←）"
              onClick={(e) => {
                e.stopPropagation();
                onSeek(-1);
              }}
            >
              <ChevronLeft className="ic" />
            </button>
            <button
              type="button"
              className="trlbnav r"
              title="下一条（→）"
              onClick={(e) => {
                e.stopPropagation();
                onSeek(1);
              }}
            >
              <ChevronRight className="ic" />
            </button>
          </>
        )}
      </div>

      <div className="trlbft" onClick={(e) => e.stopPropagation()}>
        <span className="trlbtext">{item.promptText ?? "（无提示词记录）"}</span>
        <div className="f1" />
        <button type="button" className="btn sm gho dng" onClick={onPurge}>
          <Trash2 className="ic12" />
          彻底删除
        </button>
        <button
          type="button"
          className="btn pri sm"
          disabled={!item.restorable}
          title={item.restorable ? "R" : "这条是旧版本删除的，没有留下可还原的记录"}
          onClick={onRestore}
        >
          <Undo2 className="ic12" />
          还原回原位
        </button>
      </div>
    </div>
  );
}
