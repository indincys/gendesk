import { Modal } from "@/components/ui/Modal";
import { copyText } from "@/features/_shared/MediaActions";
import { type JobView, commands, subscribeIntake, unwrap } from "@/lib/ipc";
import { useUiStore } from "@/stores/ui";
import { Bell } from "lucide-react";
import { useCallback, useEffect, useState } from "react";

const STATUSES: Record<string, string> = {
  running: "收录中",
  done: "已收录",
  error: "收录失败",
  hold: "待 Agent 确认",
};

function jobArgument(id: string) {
  return `'${id.replaceAll("'", "'\"'\"'")}'`;
}

export function IntakeNotifications() {
  const [open, setOpen] = useState(false);
  const [jobs, setJobs] = useState<JobView[]>([]);
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);
  const load = useCallback(async () => {
    setLoading(true);
    try {
      setJobs(await unwrap(commands.recentIntakeJobs()));
      setError("");
    } catch (error) {
      setError(String(error));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (!open) return;
    void load();
    const subscription = subscribeIntake(() => void load());
    return () => {
      void subscription.then((cleanup) => cleanup());
    };
  }, [load, open]);

  return (
    <>
      <button
        type="button"
        className="icb"
        title="工单通知"
        aria-label="工单通知"
        onClick={() => setOpen(true)}
      >
        <Bell className="ic12" />
      </button>
      {open && (
        <Modal title="工单通知" width="w700" onClose={() => setOpen(false)}>
          {error && (
            <p className="fs12" role="alert">
              {error}{" "}
              <button type="button" className="btn xs" onClick={() => void load()}>
                重试
              </button>
            </p>
          )}
          {loading && (
            <p className="fs12" role="status">
              正在读取工单…
            </p>
          )}
          {!loading && !error && jobs.length === 0 && <p className="fs12 t3">暂无工单通知。</p>}
          <div style={{ maxHeight: "54vh", overflow: "auto" }}>
            {jobs.map((job) => (
              <div key={job.id} className="mt10">
                <div className="fx ac gap6 fs12 wrap">
                  <strong>{job.jobId}</strong>
                  <span>
                    {STATUSES[job.status] ?? job.status} ·{" "}
                    {new Date(job.createdAt * 1000).toLocaleString()}
                  </span>
                </div>
                {job.message && <p className="fs11 t2">{job.message}</p>}
                <div className="fx ac gap6 fs11 wrap">
                  {job.batchIds.map((batch) => (
                    <button
                      type="button"
                      className="btn xs"
                      key={batch}
                      onClick={() => {
                        setOpen(false);
                        useUiStore.getState().openBatch(job.message ? "review" : "tasks", batch);
                      }}
                    >
                      打开批次 #{batch}
                    </button>
                  ))}
                  <button
                    type="button"
                    className="btn xs"
                    onClick={() => void copyText(job.jobId, "工单 ID")}
                  >
                    复制工单 ID
                  </button>
                  <button
                    type="button"
                    className="btn xs"
                    onClick={() =>
                      void copyText(`gendesk status ${jobArgument(job.jobId)}`, "状态命令")
                    }
                  >
                    复制状态命令
                  </button>
                  {(job.status === "hold" || job.status === "error") && (
                    <button
                      type="button"
                      className="btn xs"
                      onClick={() => {
                        const action = job.status === "hold" ? "confirm" : "retry";
                        void copyText(`gendesk ${action} ${jobArgument(job.jobId)}`, "Agent 命令");
                      }}
                    >
                      {job.status === "hold" ? "复制 Agent 放行命令" : "复制重试命令"}
                    </button>
                  )}
                </div>
              </div>
            ))}
          </div>
        </Modal>
      )}
    </>
  );
}
