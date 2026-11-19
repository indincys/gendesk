import { ConfirmModal, Modal } from "@/components/ui/Modal";
import { copyText } from "@/features/_shared/MediaActions";
import { type ActivityEntry, commands, subscribeV2v, unwrap } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { AlertTriangle, Ban, Info, XCircle } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { toast } from "sonner";

/**
 * 视频流水线执行日志面板。
 *
 * 回答的是「刚才这台机器替我做了什么、有没有报错」。在此之前，这条链路上全部的操作
 * 信号（查询失败、下载改名失败、CLI 拒绝提交时打出来的那句原因）都只进了 tracing，
 * 而打包后的应用没有终端 —— 于是「已提交 19」旁边没有任何东西能解释它们在干嘛。
 *
 * 快照 + 事件增量：打开时取一次全量，之后靠 `v2v://activity` 追加。按 seq 去重，
 * 因为「取快照」与「收到事件」之间那一瞬产生的条目会同时出现在两边。
 */
const LEVELS = [
  { key: "all", label: "全部" },
  { key: "warn", label: "仅警告与错误" },
  { key: "error", label: "仅错误" },
] as const;

const PHASE_LABEL: Record<string, string> = {
  cli: "CLI",
  submit: "提交",
  poll: "轮询",
  media: "落盘",
  handoff: "交接",
  rewrite: "改写",
};

export function V2vLogPanel({
  onClose,
  onLocate,
}: { onClose: () => void; onLocate: (id: number) => boolean }) {
  const [query, setQuery] = useState("");
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(true);
  const [exporting, setExporting] = useState(false);
  const [confirmClear, setConfirmClear] = useState(false);
  const [rows, setRows] = useState<ActivityEntry[]>([]);
  const [level, setLevel] = useState<(typeof LEVELS)[number]["key"]>("all");
  const [detailOf, setDetailOf] = useState<number | null>(null);
  /** 贴底时自动跟随；人往上翻了就别把他拽回来（日志正在增长时这最烦人）。 */
  const [follow, setFollow] = useState(true);
  const scroller = useRef<HTMLDivElement>(null);

  const read = useCallback(async () => {
    setLoading(true);
    setError("");
    try {
      const snapshot = await unwrap(commands.v2vActivity());
      setRows((current) =>
        [...new Map([...snapshot, ...current].map((entry) => [entry.seq, entry])).values()]
          .sort((a, b) => a.seq - b.seq)
          .slice(-500),
      );
    } catch (error) {
      setError(String(error));
    } finally {
      setLoading(false);
    }
  }, []);
  useEffect(() => {
    void read();
  }, [read]);

  const exportShown = async () => {
    setExporting(true);
    try {
      const path = await unwrap(commands.exportV2vActivity(shown.map((entry) => entry.seq)));
      if (path)
        toast.success("已导出脱敏日志", {
          action: {
            label: "显示文件",
            onClick: () =>
              void unwrap(commands.openPathInFolder(path)).catch((error) =>
                toast.error(String(error)),
              ),
          },
        });
    } catch (error) {
      toast.error(String(error));
    } finally {
      setExporting(false);
    }
  };

  // 自己订阅：面板开着才需要逐条追加，关掉就不必占着一个监听器。
  // 按 seq 去重 —— 取快照与收到事件之间那一瞬产生的条目会同时出现在两边。
  useEffect(() => {
    let un: (() => void) | undefined;
    let disposed = false;
    void subscribeV2v({
      onActivity: (e) =>
        setRows((cur) =>
          cur.some((r) => r.seq === e.entry.seq) ? cur : [...cur, e.entry].slice(-500),
        ),
    })
      .then((f) => {
        if (disposed) f();
        else un = f;
      })
      .catch((error) => setError(String(error)));
    return () => {
      disposed = true;
      un?.();
    };
  }, []);

  const shown = useMemo(
    () =>
      rows.filter(
        (r) =>
          (level === "all" || (level === "error" ? r.level === "error" : r.level !== "info")) &&
          `${r.code} ${r.message} ${r.phase} ${PHASE_LABEL[r.phase] ?? ""} ${r.level}`
            .toLowerCase()
            .includes(query.trim().toLowerCase()),
      ),
    [rows, level, query],
  );

  useEffect(() => {
    if (follow) scroller.current?.scrollTo({ top: scroller.current.scrollHeight });
  }, [follow]);
  // biome-ignore lint/correctness/useExhaustiveDependencies: 新条目到达时才滚，依赖的是条数
  useEffect(() => {
    if (follow) scroller.current?.scrollTo({ top: scroller.current.scrollHeight });
  }, [shown.length, follow]);

  const errors = rows.filter((r) => r.level === "error").length;

  return (
    <>
      <Modal
        title="执行日志"
        width="w700"
        onClose={onClose}
        headerExtra={
          <>
            <span className="chip">{rows.length} 条</span>
            {errors > 0 && <span className="bdg b-red">{errors} 个错误</span>}
          </>
        }
        footer={
          <>
            <span className="fs11 t3">
              只保留最近 500 条；应用重启即清空（每条 clip 的当前状态另存在库里，不会丢）
            </span>
            <div className="f1" />
            <button
              type="button"
              className="btn sm"
              disabled={!shown.length || exporting}
              onClick={() => void exportShown()}
            >
              导出脱敏日志 · {shown.length} 条
            </button>
            <button
              type="button"
              className="btn sm gho"
              disabled={!rows.length}
              onClick={() => setConfirmClear(true)}
            >
              <Ban className="ic12" /> 清空 {rows.length} 条
            </button>
            <button type="button" className="btn sm" onClick={onClose}>
              关闭
            </button>
          </>
        }
      >
        <div className="fx ac gap8" style={{ padding: "2px 4px 8px" }}>
          <div className="seg">
            {LEVELS.map((l) => (
              <button
                type="button"
                key={l.key}
                className={cn("sgi", level === l.key && "on")}
                aria-pressed={level === l.key}
                onClick={() => setLevel(l.key)}
              >
                {l.label}
              </button>
            ))}
          </div>
          <input
            type="search"
            className="inp sm f1"
            aria-label="搜索视频日志"
            placeholder="编号、阶段或消息"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
          />
          <label className="fx ac gap6 fs11 t3" style={{ cursor: "pointer" }}>
            <input type="checkbox" checked={follow} onChange={(e) => setFollow(e.target.checked)} />
            跟随最新
          </label>
        </div>

        <div
          ref={scroller}
          onScroll={(e) => {
            const el = e.currentTarget;
            // 贴底判定留 24px 余量：滚动条到底常差一两个像素。
            setFollow(el.scrollHeight - el.scrollTop - el.clientHeight < 24);
          }}
          style={{ maxHeight: 420, overflow: "auto", padding: "0 2px" }}
        >
          {error && (
            <p className="fs12">
              {error}{" "}
              <button type="button" className="btn xs" onClick={() => void read()}>
                重读
              </button>
            </p>
          )}
          {loading && (
            <p className="fs12 t3" role="status">
              正在读取日志…
            </p>
          )}
          {!loading && !error && shown.length === 0 && (
            <p className="fs12 t3">{rows.length ? "当前筛选无匹配日志" : "暂无日志"}</p>
          )}
          {shown.map((r) => (
            <div key={r.seq} className={cn("lgrow", r.level !== "info" && r.level)}>
              <span className="lgt">{fmtClock(r.at)}</span>
              {r.level === "error" ? (
                <XCircle className="ic12" style={{ color: "var(--er)", flex: "none" }} />
              ) : r.level === "warn" ? (
                <AlertTriangle className="ic12" style={{ color: "var(--wr)", flex: "none" }} />
              ) : (
                <Info className="ic12 t3" style={{ flex: "none" }} />
              )}
              <span className="lgp">{PHASE_LABEL[r.phase] ?? r.phase}</span>
              {r.code &&
                (r.clipId === null ? (
                  <span className="pid">{r.code}</span>
                ) : (
                  <button
                    type="button"
                    className="btn xs"
                    onClick={() => {
                      if (r.clipId !== null && !onLocate(r.clipId))
                        toast.info("此条目已移除或暂不可读取");
                    }}
                  >
                    {r.code}
                  </button>
                ))}
              <span className="f1" style={{ minWidth: 0, lineHeight: 1.55 }}>
                {r.message}
                {r.detail && (
                  <>
                    {" "}
                    <button
                      type="button"
                      className="lglink"
                      aria-expanded={detailOf === r.seq}
                      onClick={() => setDetailOf(detailOf === r.seq ? null : r.seq)}
                    >
                      {detailOf === r.seq ? "收起" : "命令详情"}
                    </button>
                    {detailOf === r.seq && <div className="cmdwell mt6">{r.detail}</div>}
                  </>
                )}
              </span>
              <button
                type="button"
                className="btn xs"
                onClick={() =>
                  void copyText(
                    [
                      new Date(r.at * 1000).toLocaleString(),
                      r.level,
                      PHASE_LABEL[r.phase] ?? r.phase,
                      r.code,
                    ]
                      .filter(Boolean)
                      .join(" · "),
                    "脱敏日志行",
                  )
                }
              >
                复制
              </button>
            </div>
          ))}
        </div>
      </Modal>
      {confirmClear && (
        <ConfirmModal
          title="清空本次运行的执行日志"
          desc={`将清空当前会话中的 ${rows.length} 条日志，任务状态会保留。需要排查时可先导出脱敏日志。`}
          confirmLabel="清空日志"
          danger
          onClose={() => setConfirmClear(false)}
          onConfirm={async () => {
            try {
              await unwrap(commands.clearV2vActivity());
              setRows([]);
              setConfirmClear(false);
              toast.success("执行日志已清空");
            } catch (error) {
              toast.error(String(error));
            }
          }}
        />
      )}
    </>
  );
}

/** unix 秒 → 本地 HH:mm:ss。日志的时间粒度是秒级，日期由「重启即清空」隐含。 */
function fmtClock(unix: number): string {
  const d = new Date(unix * 1000);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}
