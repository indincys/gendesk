import { Stepper } from "@/components/ui/Stepper";
import { type PublishSettings, commands, unwrap } from "@/lib/ipc";
import { usePublishStore } from "@/stores/publish";
import { FolderOpen } from "lucide-react";
import { useEffect, useState } from "react";
import { toast } from "sonner";

/** 设置页「发布与同步」区块（原型 publish.dc.html 设置 · 发布与同步）。 */
export function PublishSyncSection() {
  const [s, setS] = useState<PublishSettings | null>(null);
  const refreshBadges = usePublishStore((x) => x.refreshBadges);

  useEffect(() => {
    void unwrap(commands.getPublishSettings()).then(setS);
  }, []);

  const patch = async (p: Partial<Record<string, unknown>>) => {
    const next = await unwrap(
      commands.updatePublishSettings({
        rootLocal: null,
        rootExec: null,
        pathStyle: null,
        dedupDays: null,
        receiptTimeoutHours: null,
        autogenTime: null,
        warnMaterial: null,
        warnTitle: null,
        warnBody: null,
        accountDailyLimitDefault: null,
        minGapMinutes: null,
        platformMatrix: null,
        tierRules: null,
        timeSlots: null,
        ...p,
      }),
    );
    setS(next);
    void refreshBadges();
  };

  if (!s) return null;

  const chooseRoot = async () => {
    const dir = await unwrap(commands.pickPublishRoot());
    if (dir) {
      await patch({ rootLocal: dir });
      toast.success("已配置根目录，四分区已就位");
    }
  };
  const sameAsLocal = async () => {
    const next = await unwrap(commands.useLocalAsExecRoot());
    setS(next);
  };

  return (
    <section className="sec">
      <div className="sechead">
        <span className="fw6 fs13">发布与同步</span>
        <span className="pcap">资产库 · 任务包 · 回执对账的路径与阈值</span>
      </div>

      <div className="fs11 fw6 t3" style={{ letterSpacing: ".05em", marginBottom: 6 }}>
        本机根目录
      </div>
      <div className="fx ac gap10">
        <div className="pathwell f1">{s.rootLocal || "（未配置 · 请选择根目录）"}</div>
        <button type="button" className="btn sm" onClick={() => void chooseRoot()}>
          <FolderOpen className="ic12" />
          更改目录
        </button>
      </div>
      <div className="fs11 t3 mt6" style={{ lineHeight: 1.7 }}>
        GenDesk 在根目录内管理三个分区：<span className="chip">资产库/</span>{" "}
        <span className="chip">收件箱/</span> <span className="chip">任务包/</span>
        （回执在任务包内）。目录与文件名只用 ASCII；数据库只存相对路径。
      </div>

      <div className="fs11 fw6 t3 mt14" style={{ letterSpacing: ".05em", marginBottom: 6 }}>
        执行机根路径 · 导出时唯一转换点
      </div>
      <div className="fx ac gap10">
        <div className="pathwell f1">{s.rootExec || "（未配置）"}</div>
        <button type="button" className="btn sm gho" onClick={() => void sameAsLocal()}>
          同本机
        </button>
        <div className="seg">
          <span
            className={s.pathStyle === "windows" ? "sgi on" : "sgi"}
            onClick={() => void patch({ pathStyle: "windows" })}
          >
            Windows \
          </span>
          <span
            className={s.pathStyle === "unix" ? "sgi on" : "sgi"}
            onClick={() => void patch({ pathStyle: "unix" })}
          >
            macOS /
          </span>
        </div>
      </div>

      <div className="fx gap14 mt14 wrap">
        <div className="fx ac gap8">
          <span className="fs12 t2 nowrap">查重窗口</span>
          <Stepper
            value={s.dedupDays ?? 30}
            min={1}
            max={365}
            onChange={(v) => void patch({ dedupDays: v })}
          />
          <span className="fs11 t3">天 · 同素材包同平台最短复用间隔</span>
        </div>
        <div className="fx ac gap8">
          <span className="fs12 t2 nowrap">回执超时</span>
          <Stepper
            value={s.receiptTimeoutHours ?? 4}
            min={1}
            max={72}
            onChange={(v) => void patch({ receiptTimeoutHours: v })}
          />
          <span className="fs11 t3">小时 · 超时未回写标记疑似已发</span>
        </div>
      </div>

      <div className="fx gap14 mt10 wrap">
        <div className="fx ac gap8">
          <span className="fs12 t2 nowrap">每日生成时间</span>
          <input
            className="inp mono"
            style={{ width: 84 }}
            value={s.autogenTime ?? ""}
            onChange={(e) => setS({ ...s, autogenTime: e.target.value })}
            onBlur={(e) => void patch({ autogenTime: e.target.value })}
          />
          <span className="fs11 t3">HH:MM · 自动生成明日草稿</span>
        </div>
      </div>

      <div className="fx gap14 mt10 wrap">
        <div className="fx ac gap8">
          <span className="fs12 t2 nowrap">余量预警</span>
          <span className="fs11 t3">素材 &lt;</span>
          <Stepper
            value={s.warnMaterial ?? 2}
            min={0}
            max={20}
            onChange={(v) => void patch({ warnMaterial: v })}
          />
          <span className="fs11 t3">标题 &lt;</span>
          <Stepper
            value={s.warnTitle ?? 3}
            min={0}
            max={20}
            onChange={(v) => void patch({ warnTitle: v })}
          />
          <span className="fs11 t3">正文 &lt;</span>
          <Stepper
            value={s.warnBody ?? 1}
            min={0}
            max={20}
            onChange={(v) => void patch({ warnBody: v })}
          />
        </div>
      </div>

      <div className="fx gap14 mt10 wrap">
        <div className="fx ac gap8">
          <span className="fs12 t2 nowrap">账号默认日上限</span>
          <Stepper
            value={s.accountDailyLimitDefault ?? 3}
            min={1}
            max={50}
            onChange={(v) => void patch({ accountDailyLimitDefault: v })}
          />
        </div>
        <div className="fx ac gap8">
          <span className="fs12 t2 nowrap">同平台多账号最小间隔</span>
          <Stepper
            value={s.minGapMinutes ?? 60}
            min={0}
            max={600}
            onChange={(v) => void patch({ minGapMinutes: v })}
          />
          <span className="fs11 t3">分钟</span>
        </div>
      </div>
    </section>
  );
}
