import { type RefImageView, commands, subscribeRefImportProgress, unwrap } from "@/lib/ipc";
import { useCallback, useRef, useState } from "react";

/** 导入进行态。null = 空闲。 */
export interface RefImportState {
  done: number;
  total: number;
  /** 当前处理的文件名（收尾阶段为空串）。 */
  name: string;
  /** 已失败张数（逐张容错，不中断整批）。 */
  failed: number;
}

/**
 * 参考图导入（生成页上传 / 图库批量上传共用）。
 *
 * 存在的理由是一条真实事故：导入一次十几张，后端要逐张拷贝 + 解码 + 缩略图 + 压缩副本，
 * 十几秒里界面一声不吭；用户以为没点上，又点、又点，同一批图进库五六遍。
 * 所以这里做两件事——**订阅逐张进度**，以及 **用 ref 同步上锁**（`useState` 的
 * busy 要等下一次渲染才生效，挡不住连点两下这种同一帧内的重入）。
 */
export function useRefImport() {
  const [state, setState] = useState<RefImportState | null>(null);
  const busy = useRef(false);

  const run = useCallback(
    async (
      paths: string[],
      groupId: number | null,
      ephemeral: boolean,
    ): Promise<RefImageView[]> => {
      if (paths.length === 0 || busy.current) return [];
      busy.current = true;
      setState({ done: 0, total: paths.length, name: "", failed: 0 });
      // 必须先挂监听再发命令：反过来会漏掉前几张的事件（小批量时等于全漏）。
      const unsub = await subscribeRefImportProgress((p) =>
        setState({ done: p.done, total: p.total, name: p.name, failed: p.failed }),
      );
      try {
        return await unwrap(commands.importRefImages(paths, groupId, ephemeral));
      } finally {
        unsub();
        setState(null);
        busy.current = false;
      }
    },
    [],
  );

  return { state, busy, run };
}

/**
 * 导入进度覆盖层。**不可关闭**：后端没有中断能力，给个关闭按钮只会让进度从眼前消失，
 * 而任务照跑——那正是「不知道成没成」的来源。
 */
export function RefImportOverlay({ state, title }: { state: RefImportState; title: string }) {
  const pct = state.total > 0 ? Math.round((state.done / state.total) * 100) : 0;
  return (
    <div className="ovl" style={{ cursor: "progress" }}>
      <div className="mdl w420" onClick={(e) => e.stopPropagation()}>
        <div className="mhead">
          <span className="fw6 fs13">{title}</span>
          <div className="f1" />
          <span className="fs11 t3 mono">
            {state.done} / {state.total}
          </span>
        </div>
        <div className="mbody">
          <div className="fx ac gap10">
            <div className="pbarw">
              <i style={{ width: `${pct}%` }} />
            </div>
            <span className="fs11 t3 mono nowrap">{pct}%</span>
          </div>
          <div className="fs11 t3 mt10 nowrap ohide" title={state.name}>
            {state.name ? `正在处理：${state.name}` : "收尾中…"}
          </div>
          {state.failed > 0 && (
            <div className="fs11 mt6" style={{ color: "var(--wr)" }}>
              已跳过 {state.failed} 张无法读取的文件
            </div>
          )}
          <div className="fs11 t3 mt10" style={{ lineHeight: 1.7 }}>
            每张都要拷贝入库、生成缩略图与上传副本，大图会慢一些。请勿重复点击上传。
          </div>
        </div>
      </div>
    </div>
  );
}
