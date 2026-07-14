import { Modal } from "@/components/ui/Modal";
import { type BriefView, commands, unwrap } from "@/lib/ipc";
import { useUiStore } from "@/stores/ui";
import { useEffect, useState } from "react";

const SHOWN_KEY = "briefShown";

/**
 * 开屏晨报（F6）：昨天怎么样、今天要做什么、有什么卡住了。每天只弹一次。
 *
 * 只在发布模块已配置（根目录已设）时出现——没配的用户不该被一个空报告打扰。
 */
export function DailyBrief() {
  const [brief, setBrief] = useState<BriefView | null>(null);
  const go = useUiStore((s) => s.go);

  useEffect(() => {
    void (async () => {
      const s = await unwrap(commands.getPublishSettings()).catch(() => null);
      if (!s?.rootLocal) return; // 未配置发布模块 → 不打扰

      const b = await unwrap(commands.dailyBrief()).catch(() => null);
      if (!b) return;
      // 本日已弹过就不再弹（localStorage 只是「弹没弹过」的备忘，不是业务真相）。
      if (localStorage.getItem(SHOWN_KEY) === b.today) return;
      localStorage.setItem(SHOWN_KEY, b.today);
      setBrief(b);
    })();
  }, []);

  if (!brief) return null;
  const b = brief;
  const close = () => setBrief(null);
  const jump = (page: "plan" | "assets") => {
    close();
    go(page);
  };

  const nothingToDo =
    b.todaySuspect === 0 && b.todayShortage === 0 && b.unclaimed === 0 && b.runwayWarn === 0;

  return (
    <Modal
      title={`今日晨报 · ${b.today}`}
      onClose={close}
      footer={
        <>
          <span className="fs11 t3">每天首次打开时显示一次</span>
          <div className="f1" />
          <button type="button" className="btn sm" onClick={close}>
            知道了
          </button>
          <button type="button" className="btn sm pri" onClick={() => jump("plan")}>
            去发布计划
          </button>
        </>
      }
    >
      <div className="fs11 fw6 t3" style={{ letterSpacing: ".05em" }}>
        昨日
      </div>
      <div className="fx ac gap14 mt6">
        <span className="fs12">
          已发布 <b className="fw6">{b.yesterdayPublished}</b>
        </span>
        <span className="fs12">
          失败{" "}
          <b className="fw6" style={{ color: b.yesterdayFailed > 0 ? "var(--er)" : undefined }}>
            {b.yesterdayFailed}
          </b>
        </span>
        {b.yesterdaySuccessRate != null && (
          <span className="fs12 t3">成功率 {b.yesterdaySuccessRate}%</span>
        )}
      </div>

      <div className="fs11 fw6 t3 mt14" style={{ letterSpacing: ".05em" }}>
        今日
      </div>
      <div className="fx ac gap14 mt6 wrap">
        <span className="fs12">
          计划 <b className="fw6">{b.todayPlanned}</b> 条
        </span>
        {b.todaySuspect > 0 && (
          <button
            type="button"
            className="btn xs"
            style={{ color: "var(--wr)" }}
            onClick={() => jump("plan")}
          >
            {b.todaySuspect} 个疑似已发待核对
          </button>
        )}
        {b.todayShortage > 0 && (
          <button type="button" className="btn xs" onClick={() => jump("assets")}>
            {b.todayShortage} 个 SKU 缺料
          </button>
        )}
      </div>

      <div className="fs11 fw6 t3 mt14" style={{ letterSpacing: ".05em" }}>
        待处理
      </div>
      <div className="fx ac gap14 mt6 wrap">
        {b.unclaimed > 0 && (
          <button type="button" className="btn xs" onClick={() => jump("assets")}>
            收件箱 {b.unclaimed} 条待认领
          </button>
        )}
        {b.runwayWarn > 0 && (
          <button
            type="button"
            className="btn xs"
            style={{ color: "var(--wr)" }}
            onClick={() => jump("assets")}
          >
            {b.runwayWarn} 个 SKU 素材 7 天内见底
          </button>
        )}
        {nothingToDo && <span className="fs12 t3">没有需要处理的事项</span>}
      </div>
    </Modal>
  );
}
