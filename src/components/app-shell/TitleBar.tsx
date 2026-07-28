import { commands, unwrap } from "@/lib/ipc";
import { windowControls } from "@/lib/window";
import { useEngineStore } from "@/stores/engine";
import { modKeyLabel, useUiStore } from "@/stores/ui";
import { Search } from "lucide-react";

/** 44px 自绘标题栏（执行计划 0.4）。macOS 用原生交通灯（Overlay），Windows 自绘窗控。 */
export function TitleBar() {
  const platform = useUiStore((s) => s.platform);
  const openPalette = useUiStore((s) => s.openPalette);
  const updateReady = useEngineStore((s) => s.updateReady);
  const updateVersion = useEngineStore((s) => s.updateVersion);
  const mod = modKeyLabel(platform);
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
        <span className="t3 fs11 nowrap">图片生产 · 本地</span>
      </div>

      <div className="f1" />

      {updateReady && (
        <button
          type="button"
          className="pill"
          onClick={() => void unwrap(commands.installUpdate()).catch(() => {})}
        >
          <span className="pdot" />
          {updateVersion ? `v${updateVersion} ` : ""}已就绪 · 重启安装
        </button>
      )}

      <button type="button" className="tbtn" onClick={openPalette}>
        <Search className="ic12" />
        跳转
        <span className="kbd">{mod} K</span>
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
