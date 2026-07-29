import { dayLabel, hhmm } from "@/features/_shared/timeline";
import { entityLabel } from "@/features/trash/TrashRows";
import { assetSrc } from "@/lib/img";
import type { TrashItemView } from "@/lib/ipc";
import { promptLabel } from "@/lib/utils";
import { Maximize2, Trash2, Undo2 } from "lucide-react";

/**
 * 详情栏 —— 「光标这一条是什么、从哪来、还回得去吗」。
 *
 * **常驻**而不是弹窗（同视频工作台的判断）：逐条排查误删时，开一次窗看一条、关掉、
 * 再开下一条，一屏能过的条数会掉一个数量级。这一栏跟着光标走，方向键扫过去就在变。
 *
 * 「还原回原位」的按钮语气是**主要动作**：来这一页的人，十次里有九次是来找回东西的，
 * 而彻底删除在这里没有撤销可言。
 */
export function TrashSide({
  item,
  now,
  onZoom,
  onRestore,
  onPurge,
}: {
  item: TrashItemView | null;
  /** 「删除于」用它换算今天/昨天 —— 与时间线上的分段说的是同一件事。 */
  now: number;
  onZoom: () => void;
  onRestore: () => void;
  onPurge: () => void;
}) {
  if (!item) {
    return (
      <div className="trside">
        <div className="trsidehd">
          <span className="fs13 fw6">详情</span>
        </div>
        <div className="trsideempty fs12 t3">选中一项后，它的来龙去脉显示在这里。</div>
      </div>
    );
  }

  const src = assetSrc(item.imagePath) ?? assetSrc(item.thumbPath);
  const title = item.code ? promptLabel(item.code, item.title) : entityLabel(item);

  return (
    <div className="trside">
      <div className="trsidehd">
        <span className="fs13 fw6 nowrap ohide">{title}</span>
        <div className="f1" />
        <span className="bdg b-gray noshrink">{item.sourceLabel}</span>
      </div>

      <div className="trsidebody">
        {src ? (
          <button type="button" className="trbig" title="铺满查看（⏎）" onClick={onZoom}>
            <img src={src} alt="被丢弃的内容" />
            <span className="zi">
              <Maximize2 className="ic12" />
            </span>
          </button>
        ) : (
          <div className="trbiggone fs12 t3">原图已随上一次清理删除，只剩这条记录与提示词。</div>
        )}

        <dl className="trmeta">
          <Meta k="删除于" v={`${dayLabel(item.deletedAt, now)} ${hhmm(item.deletedAt)}`} />
          <Meta k="类型" v={entityLabel(item)} />
          <Meta k="批次" v={item.batchId == null ? "—" : `#${item.batchId}`} />
          <Meta k="可还原" v={item.restorable ? "是 · 回原位" : "否 · 旧版本删的，没留记录"} />
        </dl>

        <div className="fs11 fw6 t3 trlab">提示词原文</div>
        <div className="ptext mt6">{item.promptText ?? "（无提示词记录）"}</div>
      </div>

      <div className="trsidefoot">
        <button
          type="button"
          className="btn sm gho dng"
          title="物理删除文件并回收编号，不可恢复"
          onClick={onPurge}
        >
          <Trash2 className="ic12" />
          彻底删除
        </button>
        <div className="f1" />
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

function Meta({ k, v }: { k: string; v: string }) {
  return (
    <>
      <dt>{k}</dt>
      <dd>{v}</dd>
    </>
  );
}
