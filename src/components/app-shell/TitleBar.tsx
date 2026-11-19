import { IntakeNotifications } from "@/components/app-shell/IntakeNotifications";
import { UpdateInstallButton } from "@/components/app-shell/UpdateInstallButton";
import { V2vTitleChrome } from "@/features/v2v/V2vTitleChrome";
import { windowControls } from "@/lib/window";
import { ROUTE_BY_KEY } from "@/routes";
import { useEngineStore } from "@/stores/engine";
import { useUiStore } from "@/stores/ui";
import { CircleHelp } from "lucide-react";

/**
 * 44px 自绘标题栏（执行计划 0.4）。macOS 用原生交通灯（Overlay），Windows 自绘窗控。
 *
 * v0.24.0 起它多了两件事：
 *
 * - **副标题跟着页面走**（「视频流水线 · 本地」）。原来固定写「图片生产 · 本地」，
 *   那是产品的自我介绍，看第二遍就没有信息了；写当前页名反而让这一条永远有用。
 * - **留一段给当前页的读数**（`V2vTitleChrome`：通道状态灯 / 刷新 / 余额）。
 *   它们回答的是「远端此刻是什么状况」，而页头装不下 —— 那一屏要留给看片。
 */
export function TitleBar() {
  const openHelp = useUiStore((s) => s.openHelp);
  const platform = useUiStore((s) => s.platform);
  const route = useUiStore((s) => s.route);
  const returnToQueue = useUiStore((s) => s.returnToQueue);
  const goBackToQueue = useUiStore((s) => s.goBackToQueue);
  const updateReady = useEngineStore((s) => s.updateReady);
  const updateVersion = useEngineStore((s) => s.updateVersion);
  const isMac = platform === "mac";
  const isWin = platform === "win";

  return (
    <div className="tbar" data-tauri-drag-region>
      {/* macOS：为原生交通灯预留空间（不自绘圆点）。 */}
      {isMac && <div style={{ width: 68 }} className="noshrink" />}

      <div className="fx ac gap7 noshrink">
        <span className="logo">
          <svg
            className="ic12"
            viewBox="0 0 16 16"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.6"
            strokeLinecap="round"
            strokeLinejoin="round"
          >
            <path d="M8 2 9.7 6.3 14 8 9.7 9.7 8 14 6.3 9.7 2 8 6.3 6.3Z" />
          </svg>
        </span>
        <span className="fw6 fs13 nowrap">GenDesk</span>
        <span className="t3 fs11 nowrap">{ROUTE_BY_KEY[route].label} · 本地</span>
      </div>

      {route === "v2v" && <V2vTitleChrome />}

      <div className="f1" />

      {updateReady && (
        <UpdateInstallButton className="pill">
          <span className="pdot" />
          {updateVersion ? `v${updateVersion} ` : ""}已就绪 · 重启安装
        </UpdateInstallButton>
      )}

      {returnToQueue && (route === "review" || route === "library") && (
        <button type="button" className="btn xs" onClick={goBackToQueue}>
          返回任务队列
        </button>
      )}
      <IntakeNotifications />
      <button
        type="button"
        className="icb"
        onClick={openHelp}
        aria-label="快捷键与帮助"
        title="快捷键与帮助"
      >
        <CircleHelp className="ic12" />
      </button>

      {isWin && (
        <div className="fx ac noshrink">
          <button
            type="button"
            className="wb"
            onClick={() => void windowControls.minimize()}
            aria-label="最小化"
          >
            <svg className="ic12" viewBox="0 0 16 16" stroke="currentColor" strokeWidth="1.2">
              <path d="M4 8h8" />
            </svg>
          </button>
          <button
            type="button"
            className="wb"
            onClick={() => void windowControls.toggleMaximize()}
            aria-label="最大化"
          >
            <svg
              className="ic12"
              viewBox="0 0 16 16"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.2"
            >
              <rect x="4.5" y="4.5" width="7" height="7" rx="1" />
            </svg>
          </button>
          {/* 收起而不是退出：点它之后进程照常跑（轮询器 / 两个 watcher / 常驻补单），
              图标退进托盘。标签必须说清这件事，否则人会以为自己刚把跑批停了。 */}
          <button
            type="button"
            className="wb wbx"
            onClick={() => void windowControls.close()}
            aria-label="收起窗口（后台继续运行）"
            title="收起窗口 —— 后台继续跑，从菜单栏图标叫回来"
          >
            <svg className="ic12" viewBox="0 0 16 16" stroke="currentColor" strokeWidth="1.2">
              <path d="m4.5 4.5 7 7M11.5 4.5l-7 7" />
            </svg>
          </button>
        </div>
      )}
    </div>
  );
}
