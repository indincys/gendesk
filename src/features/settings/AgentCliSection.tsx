import { type AgentCliInfo, commands, unwrap } from "@/lib/ipc";
import { Copy, FolderOpen } from "lucide-react";
import { useEffect, useState } from "react";
import { toast } from "sonner";

const COMMANDS: [string, string][] = [
  ["gendesk doctor", "检查连接与应用能力"],
  ["gendesk submit --manifest 清单.json", "投单并开跑"],
  ["gendesk status --job <id>", "回读收录与生成进度"],
  ["gendesk v2v pending / rewrite", "取待改写视频提示词并写回"],
];

/** 设置页「Agent CLI」：所有外部任务都经 `gendesk` 命令走本机控制通道，不靠目录监听。 */
export function AgentCliSection({ id }: { id: string }) {
  const [info, setInfo] = useState<AgentCliInfo | null>(null);

  useEffect(() => {
    void unwrap(commands.getAgentCliInfo())
      .then(setInfo)
      .catch(() => setInfo(null));
  }, []);

  return (
    <section className="sec" id={id}>
      <div className="sechead">
        <span className="fw6 fs13">Agent CLI</span>
        <span className="pcap">Agent 用 gendesk 命令投单、查进度、写回改写</span>
        <div className="f1" />
        {info &&
          (info.listening ? (
            <span className="bdg b-green">控制通道在线 · 协议 {info.protocol}</span>
          ) : (
            <span className="bdg b-red">控制通道未启动</span>
          ))}
      </div>
      {info && (
        <div className="fx ac gap10">
          <span className="fs11 t3 nowrap">交接根</span>
          <div className="pathwell f1 ohide nowrap" title={info.root}>
            {info.root}
          </div>
          <button
            type="button"
            className="btn sm gho"
            onClick={() =>
              void unwrap(commands.openAgentRoot()).catch((e) => toast.error(String(e)))
            }
          >
            <FolderOpen className="ic12" />
            打开
          </button>
        </div>
      )}
      <div className="clilist mt10">
        {COMMANDS.map(([cmd, desc]) => (
          <div className="cliline" key={cmd}>
            <code className="mono fs11">{cmd}</code>
            <span className="fs11 t3 f1">{desc}</span>
            <button
              type="button"
              className="icb"
              title="复制命令"
              onClick={() =>
                void navigator.clipboard
                  .writeText(cmd)
                  .then(() => toast.success("已复制"))
                  .catch(() => {})
              }
            >
              <Copy className="ic12" />
            </button>
          </div>
        ))}
      </div>
      <div className="fs11 t3 mt10">
        {info ? `单次超过 ${info.taskThreshold} 张会停在待确认，回 Agent 确认后才开跑。` : ""}
        应用没开时 CLI 会在后台启动它；连接信息写在交接根的「连接.json」。
      </div>
    </section>
  );
}
