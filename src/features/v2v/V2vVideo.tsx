import { cn } from "@/lib/utils";
import { Pause, Play, StepBack, StepForward } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";

/**
 * 验收用的播放器。**自绘控制条，不用浏览器原生 controls。**
 *
 * ## 为什么不用原生 controls
 *
 * 两件事，各自都足以否掉它：
 *
 * 1. **它压着画面**。WebKit 的原生控制条自带一层暗色渐变铺在画面下缘，一获焦就常驻
 *    不隐。而这一页判的恰恰是**色差与形变** —— 下三分之一被压暗，产品的暗部细节
 *    直接看不出来了，验收结论会跟着变。
 * 2. **它抢走空格键**。原生控件获焦后，空格是「播放/暂停」；而这一页的空格是
 *    「通过」。两个处理器都会响应，于是按一下既判了片又暂停了播放 —— 更糟的是
 *    暂停发生得悄无声息，人以为这条视频「不动了」。
 *
 * 故控制条自绘、放在**画面之外**，而 `<video>` 自己 `tabIndex={-1}` 永不获焦，
 * 控制按钮 `onMouseDown` 阻止默认以免点击后留住焦点。
 *
 * ## 逐帧
 *
 * 判形变要停在某一帧上看。`fps` 拿不到时按 24 算 —— 步进的精确值不重要，
 * 「能停下来一帧一帧挪」才重要。
 */
export function V2vVideo({
  src,
  fps,
  portrait,
  dark,
  className,
  videoKey,
}: {
  src: string;
  /** 逐帧步进用；拿不到按 24。 */
  fps?: number | null;
  /** 竖幅画框（9:16）。详情栏小窗默认开 —— 出的片子基本都是竖版。 */
  portrait?: boolean;
  /** 暗场（看片流）配色。 */
  dark?: boolean;
  className?: string;
  /**
   * 换条时必须换 key，否则 `<video>` 元素被复用、播放头停在上一条的位置，
   * 看着像「这条没动」。
   */
  videoKey?: string | number;
}) {
  const ref = useRef<HTMLVideoElement>(null);
  const [playing, setPlaying] = useState(true);
  const [cur, setCur] = useState(0);
  const [dur, setDur] = useState(0);
  const [rate, setRate] = useState(1);

  const step = 1 / (fps && fps > 0 ? fps : 24);

  // 换条时把状态复位：不复位的话新片子会顶着上一条的进度条数字渲染一帧。
  // biome-ignore lint/correctness/useExhaustiveDependencies: 依赖的是「换条了」这个信号
  useEffect(() => {
    setCur(0);
    setDur(0);
    setPlaying(true);
    setRate(1);
  }, [videoKey, src]);

  const toggle = useCallback(() => {
    const v = ref.current;
    if (!v) return;
    if (v.paused) void v.play().catch(() => {});
    else v.pause();
  }, []);

  const nudge = useCallback((d: number) => {
    const v = ref.current;
    if (!v) return;
    v.pause();
    v.currentTime = Math.max(0, Math.min(v.duration || 0, v.currentTime + d));
  }, []);

  const seek = useCallback((e: React.MouseEvent<HTMLDivElement>) => {
    const v = ref.current;
    if (!v || !v.duration) return;
    const box = e.currentTarget.getBoundingClientRect();
    v.currentTime = Math.max(0, Math.min(1, (e.clientX - box.left) / box.width)) * v.duration;
  }, []);

  const pct = dur > 0 ? (cur / dur) * 100 : 0;

  return (
    <div className={cn("vplay", dark && "dk", className)}>
      <div className={cn("vplayframe", portrait && "pt")}>
        <video
          key={videoKey}
          ref={ref}
          src={src}
          loop
          autoPlay
          muted
          playsInline
          tabIndex={-1}
          onClick={toggle}
          onPlay={() => setPlaying(true)}
          onPause={() => setPlaying(false)}
          onTimeUpdate={(e) => setCur(e.currentTarget.currentTime)}
          onLoadedMetadata={(e) => setDur(e.currentTarget.duration || 0)}
        />
      </div>
      <div className="vplaybar">
        <button
          type="button"
          className="ic"
          onMouseDown={noFocus}
          onClick={toggle}
          title="播放/暂停"
        >
          {playing ? <Pause className="ic12" /> : <Play className="ic12" />}
        </button>
        <button
          type="button"
          className="ic"
          onMouseDown={noFocus}
          onClick={() => nudge(-step)}
          title="上一帧"
        >
          <StepBack className="ic12" />
        </button>
        <button
          type="button"
          className="ic"
          onMouseDown={noFocus}
          onClick={() => nudge(step)}
          title="下一帧"
        >
          <StepForward className="ic12" />
        </button>
        <div
          className="tk"
          onClick={seek}
          onKeyDown={(e) => {
            if (e.key === "ArrowLeft") nudge(-step);
            if (e.key === "ArrowRight") nudge(step);
          }}
          role="slider"
          aria-label="播放进度"
          aria-valuemin={0}
          aria-valuemax={Math.round(dur * 10)}
          aria-valuenow={Math.round(cur * 10)}
          tabIndex={-1}
        >
          <div style={{ width: `${pct}%` }} />
        </div>
        <span className="tm">
          {fmtT(cur)}/{fmtT(dur)}
        </span>
        <button
          type="button"
          className="sp"
          onMouseDown={noFocus}
          onClick={() => {
            const next = rate === 1 ? 0.5 : rate === 0.5 ? 0.25 : 1;
            setRate(next);
            if (ref.current) ref.current.playbackRate = next;
          }}
          title="放慢看形变"
        >
          {rate}×
        </button>
      </div>
    </div>
  );
}

/** 点了按钮不留焦点 —— 留住焦点，空格就会去按它而不是判「通过」。 */
function noFocus(e: React.MouseEvent) {
  e.preventDefault();
}

function fmtT(s: number): string {
  if (!Number.isFinite(s) || s < 0) return "0.0s";
  return `${s.toFixed(1)}s`;
}
