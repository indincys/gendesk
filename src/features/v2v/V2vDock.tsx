import { Tooltip } from "@/components/ui/Tooltip";
import { type Row, removalRisk } from "@/features/v2v/model";
import { cn } from "@/lib/utils";
import { FolderOpen, RefreshCw, Send, Trash2 } from "lucide-react";

export interface DockHandlers {
  onSubmit: (ids: number[]) => void;
  onReview: (ids: number[], pass: boolean) => void;
  onRecover: (ids: number[]) => void;
  onRequeueRewrite: (ids: number[]) => void;
  onResume: (ids: number[]) => void;
  onUnqueue: (ids: number[]) => void;
  onEnterReview: (ids: number[]) => void;
  onIngest: () => void;
  onOpenHandoff: () => void;
  onPollNow: () => void;
  onSwitchChannel: (ids: number[]) => void;
  onEditParams: (ids: number[]) => void;
  onRemove: (ids: number[]) => void;
  onUndo: () => void;
}

export function dockScope(row: Row | null, rows: Row[], selected: ReadonlySet<number>): Row[] {
  if (selected.size > 0) return rows.filter((item) => selected.has(item.clip.id));
  return row == null ? [] : [row];
}

export function reviewScope(rows: Row[], selected: ReadonlySet<number> | null): Row[] {
  return rows.filter(
    (row) => row.stage === "rev" && (selected == null || selected.has(row.clip.id)),
  );
}

/**
 * 统一动作坞：勾选存在时只处理全部勾选项，否则只处理当前条目。
 * 每个按钮都从同一作用域筛出自己的合格 ID，标签显示真实数量。
 */
export function V2vDock({
  row,
  rows,
  sel,
  running,
  busy,
  undoLabel,
  h,
}: {
  row: Row | null;
  rows: Row[];
  sel: Set<number>;
  running: number;
  busy: boolean;
  undoLabel: string | null;
  h: DockHandlers;
}) {
  const scope = dockScope(row, rows, sel);
  const idsOf = (test: (r: Row) => boolean) => scope.filter(test).map((r) => r.clip.id);
  const submit = idsOf((r) => r.stage === "ready" && r.clip.submitQueuedAt == null);
  const queued = idsOf((r) => r.stage === "ready" && r.clip.submitQueuedAt != null);
  const review = idsOf((r) => r.stage === "rev");
  const recover = idsOf(
    (r) =>
      (r.stage === "fail" &&
        !(r.clip.errorType === "timeout" && (r.clip.submitId ?? "").trim() !== "")) ||
      (r.stage === "run" && r.clip.phantomSuspect),
  );
  const resume = idsOf(
    (r) =>
      r.stage === "fail" && r.clip.errorType === "timeout" && (r.clip.submitId ?? "").trim() !== "",
  );
  const rewrite = idsOf((r) => ["ready", "rev", "rej", "fail"].includes(r.stage));
  const switchable = idsOf(
    (r) => r.stage === "ready" || r.stage === "rewrite" || r.stage === "run",
  );
  const editable = idsOf((r) => r.stage === "ready" || r.stage === "rewrite");
  const hasRewrite = scope.some((r) => r.action === "rewrite");
  const hasWait = scope.some((r) => r.action === "wait");
  const removal = scope.some((r) => removalRisk(r) !== "free");
  const count = (label: string, ids: number[]) => `${label} · ${ids.length}`;

  return (
    <div className="vdock">
      {submit.length > 0 && (
        <button
          type="button"
          className="btn sm pri"
          disabled={busy}
          onClick={() => h.onSubmit(submit)}
        >
          <Send className="ic12" />
          {count("放行", submit)}
        </button>
      )}
      {queued.length > 0 && (
        <Tooltip content="尚未提交到即梦，不会产生额度消耗">
          <button
            type="button"
            className="btn sm gho"
            disabled={busy}
            onClick={() => h.onUnqueue(queued)}
          >
            {count("撤回放行", queued)}
          </button>
        </Tooltip>
      )}
      {review.length > 0 && (
        <>
          <button
            type="button"
            className="btn sm okb"
            disabled={busy}
            onClick={() => h.onReview(review, true)}
          >
            {count("通过", review)}
          </button>
          <button
            type="button"
            className="btn sm dngo"
            disabled={busy}
            onClick={() => h.onReview(review, false)}
          >
            {count("不通过", review)}
          </button>
          <button
            type="button"
            className="btn sm gho"
            disabled={busy}
            onClick={() => h.onEnterReview(review)}
          >
            {count("看片", review)}
          </button>
        </>
      )}
      {resume.length > 0 && (
        <Tooltip content="继续查询原提交单，不再次提交或计费">
          <button
            type="button"
            className="btn sm pri"
            disabled={busy}
            onClick={() => h.onResume(resume)}
          >
            {count("继续等待", resume)}
          </button>
        </Tooltip>
      )}
      {recover.length > 0 && (
        <Tooltip content="仅恢复没有可用输出的失败或中断任务">
          <button
            type="button"
            className={cn(
              "btn sm",
              scope.some((r) => recover.includes(r.clip.id) && r.clip.billed) ? "dngo" : "gho",
            )}
            disabled={busy}
            onClick={() => h.onRecover(recover)}
          >
            {count("恢复", recover)}
          </button>
        </Tooltip>
      )}
      {rewrite.length > 0 && (
        <Tooltip content="清除当前视频提示词并进入改写流程">
          <button
            type="button"
            className="btn sm gho"
            disabled={busy}
            onClick={() => h.onRequeueRewrite(rewrite)}
          >
            {count("退回改写", rewrite)}
          </button>
        </Tooltip>
      )}
      {hasRewrite && (
        <>
          <button type="button" className="btn sm pri" disabled={busy} onClick={h.onIngest}>
            <RefreshCw className="ic12" />
            收录改写
          </button>
          <button type="button" className="btn sm gho" onClick={h.onOpenHandoff}>
            <FolderOpen className="ic12" />
            交接目录
          </button>
        </>
      )}
      {hasWait && (
        <button type="button" className="btn sm gho" disabled={busy} onClick={h.onPollNow}>
          <RefreshCw className="ic12" />
          刷新远端 · {running}
        </button>
      )}
      {switchable.length > 0 && (
        <button
          type="button"
          className="btn sm gho"
          disabled={busy}
          onClick={() => h.onSwitchChannel(switchable)}
        >
          {count("改通道", switchable)}
        </button>
      )}
      {editable.length > 0 && (
        <button
          type="button"
          className="btn sm gho"
          disabled={busy}
          onClick={() => h.onEditParams(editable)}
        >
          {count("改参数", editable)}
        </button>
      )}
      {scope.length > 0 && (
        <button
          type="button"
          className={cn("btn sm", removal ? "dngo" : "gho")}
          disabled={busy}
          onClick={() => h.onRemove(scope.map((r) => r.clip.id))}
        >
          <Trash2 className="ic12" />
          {count(
            "删除",
            scope.map((r) => r.clip.id),
          )}
        </button>
      )}
      {undoLabel && (
        <span className="vundo">
          <span className="ohide">{undoLabel}</span>
          <button type="button" onClick={h.onUndo}>
            撤销 U
          </button>
        </span>
      )}
    </div>
  );
}
