import { type PublishSettings, commands, unwrap } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { usePublishStore } from "@/stores/publish";
import { FolderOpen } from "lucide-react";
import { useEffect, useState } from "react";
import { toast } from "sonner";

/** `HH:MM`（00:00–23:59）；与后端 `scheduler::parse_hhmm` 同一套规则。 */
function isHhmm(v: string | null | undefined): boolean {
  if (!v) return false;
  const m = /^(\d{1,2}):(\d{2})$/.exec(v.trim());
  if (!m) return false;
  const h = Number(m[1]);
  const min = Number(m[2]);
  return h >= 0 && h < 24 && min >= 0 && min < 60;
}

/** 设置页「发布与同步」区块（原型 publish.dc.html 设置 · 发布与同步）。 */
export function PublishSyncSection() {
  const [s, setS] = useState<PublishSettings | null>(null);
  const refreshBadges = usePublishStore((x) => x.refreshBadges);

  useEffect(() => {
    void unwrap(commands.getPublishSettings()).then(setS);
  }, []);

  const patch = async (p: Partial<Record<string, unknown>>) => {
    try {
      const next = await unwrap(
        commands.updatePublishSettings({
          rootLocal: null,
          rootExec: null,
          pathStyle: null,
          autogenTime: null,
          schedulePaused: null,
          ...p,
        }),
      );
      setS(next);
      void refreshBadges();
    } catch (e) {
      // 后端拒绝（如非法时间格式）→ 提示并回读，避免界面停在一个没存进去的值上。
      toast.error(String(e));
      setS(await unwrap(commands.getPublishSettings()));
    }
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
        <span className="pcap">图片素材库 · 任务包 · 自动组稿的运行设置</span>
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
        GenDesk 在根目录内管理三个分区：<span className="chip">图片素材库/</span>{" "}
        <span className="chip">收件箱/</span> <span className="chip">任务包/</span>
        （回执在任务包内）。任务 ID 与导出图片文件名只用 ASCII；数据库只存相对路径。
      </div>

      <div className="fs11 fw6 t3 mt14" style={{ letterSpacing: ".05em", marginBottom: 6 }}>
        执行机根路径 · 导出时唯一转换点
      </div>
      <div className="fx ac gap10">
        <input
          className="inp mono f1"
          value={s.rootExec}
          placeholder={s.pathStyle === "windows" ? "D:\\GenDesk" : "/srv/gendesk"}
          onChange={(e) => setS({ ...s, rootExec: e.target.value })}
          onBlur={(e) => void patch({ rootExec: e.target.value.trim() })}
        />
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
          <span className="fs12 t2 nowrap">每日生成时间</span>
          <input
            className="inp mono"
            style={{
              width: 84,
              ...(isHhmm(s.autogenTime) ? {} : { borderColor: "var(--er)" }),
            }}
            value={s.autogenTime ?? ""}
            onChange={(e) => setS({ ...s, autogenTime: e.target.value })}
            // 非法值不提交：后端会拒绝，先在本地拦下，省一次往返 + 一个红 toast。
            onBlur={(e) => {
              if (isHhmm(e.target.value)) void patch({ autogenTime: e.target.value });
            }}
          />
          <span className={cn("fs11", isHhmm(s.autogenTime) ? "t3" : "terr")}>
            {isHhmm(s.autogenTime) ? "HH:MM · 自动生成明日草稿" : "格式应为 HH:MM（00:00–23:59）"}
          </span>
        </div>
      </div>

      <div className="fs11 fw6 t3 mt14" style={{ letterSpacing: ".05em", marginBottom: 7 }}>
        平台档案
      </div>
      <div className="pub-platform-profiles">
        {[
          ["douyin", "抖音", "标题 ≤20 · 可挂车 · 首图为封面"],
          ["xhs", "小红书", "标题 ≤20 · 原创 · 不允许合拍/复制"],
          ["shipinhao", "视频号", "标题 ≤22 · 不显示位置"],
          ["kuaishou", "快手", "标题固定为空 · 正文 ≤500"],
        ].map(([code, label, hint]) => (
          <div className="pub-platform-profile" key={code}>
            <b>{label}</b>
            <small>{hint}</small>
          </div>
        ))}
      </div>
      <label className="pub-check mt10">
        <input
          type="checkbox"
          checked={s.schedulePaused ?? false}
          onChange={(event) => void patch({ schedulePaused: event.target.checked })}
        />
        暂停自动生成（回执收取与结算不暂停）
      </label>
    </section>
  );
}
