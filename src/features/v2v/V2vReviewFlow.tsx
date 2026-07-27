import { V2vVideo } from "@/features/v2v/V2vVideo";
import type { Row } from "@/features/v2v/model";
import { assetSrc } from "@/lib/img";
import { cn } from "@/lib/utils";
import { useEffect, useRef } from "react";

/**
 * 全屏看片流。
 *
 * 看板回答「这批做到哪了」，这里回答「这一条行不行」—— 两个问题的最优布局不一样：
 * 判片要大画面、要**首帧对照**（判的是「动起来之后产品还对不对」）、要一次按键走一条。
 * 故它是一层覆盖而不是一个页面：ESC 回到看板，光标位置与筛选原样保留。
 *
 * 暗场沿用图片验收的精审模式（`--rd-*`）：判色差与形变时，浅色界面的反光会骗人。
 */
export function V2vReviewFlow({
  list,
  index,
  passedCount,
  killedCount,
  undoLabel,
  busy,
  onSeek,
  onPass,
  onReject,
  onRerun,
  onRewrite,
  onUndo,
  onExit,
}: {
  /** 待验收序列（受看板筛选影响 —— 筛了「幽灵单」就只看那几条）。 */
  list: Row[];
  index: number;
  passedCount: number;
  killedCount: number;
  undoLabel: string | null;
  busy: boolean;
  onSeek: (clipId: number) => void;
  onPass: () => void;
  onReject: () => void;
  onRerun: () => void;
  onRewrite: () => void;
  onUndo: () => void;
  onExit: () => void;
}) {
  const cur = list[index] ?? null;
  const stripRef = useRef<HTMLDivElement>(null);

  // 胶片条跟着光标滚：一次 46 条时，当前那格常常已经滚出视野，
  // 而这条带子存在的意义就是「还剩多少 / 刚判了什么」。
  // biome-ignore lint/correctness/useExhaustiveDependencies: 依赖的是「光标换了」这个信号，effect 体读的是 DOM
  useEffect(() => {
    const el = stripRef.current?.querySelector<HTMLElement>("[data-cur='1']");
    el?.scrollIntoView({ block: "nearest", inline: "center" });
  }, [index]);

  if (!cur) {
    return (
      <div className="vrev">
        <div className="vrevhd">
          <span className="fw6 fs13">验收看片流</span>
          <div className="f1" />
          <button type="button" className="btn sm gho vrevbtn" onClick={onExit}>
            退出 ESC
          </button>
        </div>
        <div className="f1 fx ac jc" style={{ justifyContent: "center" }}>
          <span className="fs13" style={{ color: "var(--rd-t2)" }}>
            当前筛选下没有待验收的条目
          </span>
        </div>
      </div>
    );
  }

  const c = cur.clip;
  const video = assetSrc(c.videoPath);
  const frame = assetSrc(c.thumbPath);
  const pct = list.length === 0 ? 0 : Math.round(((index + 1) / list.length) * 100);
  // 只画光标附近 ±11 条：46 条全铺出来会横向溢出到看不见，而胶片条要回答的是
  // 「还剩多少 / 刚判了什么」，两端各留一个计数就够。
  const from = Math.max(0, index - 11);
  const to = Math.min(list.length, index + 12);
  const strip = list.slice(from, to);

  return (
    <div className="vrev">
      <div className="vrevhd">
        <span className="fw6 fs13">验收看片流</span>
        <span className="fs11 mono" style={{ color: "var(--rd-t2)" }}>
          第 {index + 1} / {list.length} 条
        </span>
        <div className="vrevbar">
          <div style={{ width: `${pct}%` }} />
        </div>
        <span className="fs11" style={{ color: "var(--rd-t3)" }}>
          本轮已过 {passedCount} · 已毙 {killedCount} · 剩 {Math.max(0, list.length - index - 1)}
        </span>
        <div className="f1" />
        <span className="vrevchip">
          {c.batchId == null ? "无批次" : `#${c.batchId}`} · {c.groupName || "未分组"}
        </span>
        <span className="vrevchip">
          {cur.modelShort} ·{" "}
          {c.creditCount != null ? `${c.creditCount} 额度` : `${cur.estimate ?? "?"} 额度（估）`}
        </span>
        <button type="button" className="btn sm gho vrevbtn" onClick={onExit}>
          退出 ESC
        </button>
      </div>

      <div className="vrevbody">
        <div className="vrevmain">
          <div className="vrevstage">
            {video ? (
              // key 绑 clip id：不换 key 的话切下一条时 <video> 会复用同一个元素，
              // 播放头停在上一条的位置，看着像「这条没动」。
              <V2vVideo src={video} fps={c.fps} dark videoKey={c.id} />
            ) : (
              <span className="vrevnote">这一条没有成片文件</span>
            )}
            <span className="vrevcode">{c.promptCode}</span>
          </div>
          <div className="fs10 mono" style={{ color: "var(--rd-t3)" }}>
            {c.width != null && `${c.width}×${c.height}`}
            {c.durationSec != null && ` · ${c.durationSec.toFixed(1)}s`}
            {c.fps != null && ` · ${Math.round(c.fps)}fps`}
            {c.benefitType != null && ` · 计费型号 ${c.benefitType}`}
          </div>
        </div>

        <div className="vrevside">
          <div className="vrevframe">
            {frame ? (
              <img src={frame} alt="首帧原图（对照）" />
            ) : (
              <span className="vrevnote">首帧原图不可用</span>
            )}
            <span className="vrevcode">首帧原图（对照）</span>
          </div>
          <div className="vrevcard">
            <div className="fs10 fw6" style={{ color: "var(--rd-t3)", letterSpacing: ".05em" }}>
              视频提示词
            </div>
            <div className="vrevtext">{c.videoPrompt ?? "（无）"}</div>
            <div className="fx wrap gap5 mt8">
              <span className="vrevchip sm">{c.batchId == null ? "无批次" : `#${c.batchId}`}</span>
              <span className="vrevchip sm">{cur.modelFull ?? "CLI 默认"}</span>
              <span className="vrevchip sm">第 {Math.max(1, c.attempt)} 次尝试</span>
              {cur.waitSecs > 0 && (
                <span className="vrevchip sm">
                  提交→出片 {Math.floor(cur.waitSecs / 3600)}h
                  {String(Math.floor((cur.waitSecs % 3600) / 60)).padStart(2, "0")}m
                </span>
              )}
            </div>
          </div>
        </div>
      </div>

      <div className="vrevstrip" ref={stripRef}>
        {from > 0 && <span className="n">←{from}</span>}
        <div className="vstrip fx gap5" style={{ flex: 1, minWidth: 0, alignItems: "flex-end" }}>
          {strip.map((r) => {
            const t = assetSrc(r.clip.posterPath ?? r.clip.thumbPath);
            const isCur = r.clip.id === c.id;
            return (
              <div
                key={r.clip.id}
                data-cur={isCur ? "1" : "0"}
                className={cn("vrevcell", isCur && "on")}
                onClick={() => onSeek(r.clip.id)}
                onKeyDown={(e) => e.key === "Enter" && onSeek(r.clip.id)}
                role="button"
                tabIndex={-1}
                title={r.clip.promptCode}
              >
                {t && <img src={t} alt="" />}
              </div>
            );
          })}
        </div>
        {to < list.length && <span className="n">+{list.length - to}</span>}
      </div>

      <div className="vrevfoot">
        <button type="button" className="btn sm vrevok" disabled={busy} onClick={onPass}>
          通过 <span className="kh">空格</span>
        </button>
        <button type="button" className="btn sm vrevno" disabled={busy} onClick={onReject}>
          不通过 <span className="kh">X</span>
        </button>
        <button type="button" className="btn sm vrevbtn" disabled={busy} onClick={onRerun}>
          不通过并重跑 <span className="kh">R</span>
        </button>
        <button type="button" className="btn sm vrevbtn" disabled={busy} onClick={onRewrite}>
          退回改写 <span className="kh">E</span>
        </button>
        {undoLabel && (
          <span className="vrevundo">
            {undoLabel}
            <button type="button" onClick={onUndo}>
              撤销 U
            </button>
          </span>
        )}
        <div className="f1" />
        <span className="fs10 mono" style={{ color: "var(--rd-t3)" }}>
          J/K 上下条 · U 撤销 · ESC 退出
        </span>
      </div>
    </div>
  );
}
