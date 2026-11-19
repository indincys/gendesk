import { cn } from "@/lib/utils";
import { Maximize, Pause, Play, StepBack, StepForward, Volume2, VolumeX } from "lucide-react";
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
  const [muted, setMuted] = useState(true);
  const [failed, setFailed] = useState(false);

  const step = 1 / (fps && fps > 0 ? fps : 24);

  // 换条时把状态复位：不复位的话新片子会顶着上一条的进度条数字渲染一帧。
  // biome-ignore lint/correctness/useExhaustiveDependencies: 依赖的是「换条了」这个信号
  useEffect(() => {
    setCur(0);
    setDur(0);
    setPlaying(true);
    setRate(1);
    setMuted(true);
    setFailed(false);
  }, [videoKey, src]);

  const toggle = useCallback(() => {
    const v = ref.current;
    if (!v) return;
    if (v.paused) void v.play().catch(() => setFailed(true));
    else v.pause();
  }, []);

  const nudge = useCallback((d: number) => {
    const v = ref.current;
    if (!v) return;
    v.pause();
    v.currentTime = Math.max(0, Math.min(v.duration || 0, v.currentTime + d));
  }, []);

  function seek(value: number) {
    const video = ref.current;
    if (video && Number.isFinite(video.duration)) video.currentTime = value;
  }

  return (
    <div className={cn("vplay", dark && "dk", className)}>
      <div className={cn("vplayframe", portrait && "pt")}>
        <video
          key={videoKey}
          ref={ref}
          src={src}
          loop
          autoPlay
          muted={muted}
          playsInline
          tabIndex={-1}
          onClick={toggle}
          onPlay={() => setPlaying(true)}
          onPause={() => setPlaying(false)}
          onTimeUpdate={(e) => setCur(e.currentTarget.currentTime)}
          onLoadedMetadata={(e) => {
            setDur(Number.isFinite(e.currentTarget.duration) ? e.currentTarget.duration : 0);
            setFailed(false);
          }}
          onError={() => setFailed(true)}
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
        <input
          className="tk"
          type="range"
          min={0}
          max={dur || 1}
          step={step}
          value={Math.min(cur, dur || 1)}
          disabled={!dur}
          aria-label="播放进度"
          aria-valuetext={fmtT(cur)}
          onChange={(event) => seek(Number(event.target.value))}
        />
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
        <button
          type="button"
          className="ic"
          title={muted ? "打开声音" : "静音"}
          aria-label={muted ? "打开声音" : "静音"}
          aria-pressed={!muted}
          onClick={() => setMuted((value) => !value)}
        >
          {muted ? <VolumeX className="ic12" /> : <Volume2 className="ic12" />}
        </button>
        <button
          type="button"
          className="ic"
          title="全屏播放"
          aria-label="全屏播放"
          onClick={() => {
            const video = ref.current as
              | (HTMLVideoElement & { webkitEnterFullscreen?: () => void })
              | null;
            if (video?.requestFullscreen)
              void video.requestFullscreen().catch(() => video.webkitEnterFullscreen?.());
            else video?.webkitEnterFullscreen?.();
          }}
        >
          <Maximize className="ic12" />
        </button>
      </div>
      {failed && (
        <div className="fx ac gap8 fs11" role="alert">
          视频播放失败
          <button
            type="button"
            className="btn xs"
            onClick={() => {
              setFailed(false);
              ref.current?.load();
            }}
          >
            重新加载
          </button>
        </div>
      )}
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
