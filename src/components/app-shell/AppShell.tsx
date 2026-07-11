import { CommandPalette } from "@/components/CommandPalette";
import { HelpPanel } from "@/components/HelpPanel";
import { Onboarding } from "@/components/Onboarding";
import { Sidebar } from "@/components/app-shell/Sidebar";
import { TitleBar } from "@/components/app-shell/TitleBar";
import { commands, unwrap } from "@/lib/ipc";
import { useGlobalKeyboard } from "@/lib/keyboard";
import { ROUTE_BY_KEY } from "@/routes";
import { useEngineStore } from "@/stores/engine";
import { useSettingsStore } from "@/stores/settings";
import { useUiStore } from "@/stores/ui";
import { useEffect } from "react";

/** 应用外壳（执行计划 0.4）：标题栏 + 侧栏 + 主面板容器。 */
export function AppShell() {
  useGlobalKeyboard();
  const route = useUiStore((s) => s.route);
  const platform = useUiStore((s) => s.platform);
  const initEngine = useEngineStore((s) => s.init);
  const refreshBadges = useEngineStore((s) => s.refreshBadgeCounts);
  const loadSettings = useSettingsStore((s) => s.load);
  const ActivePage = ROUTE_BY_KEY[route].component;

  // 加载设置（E13 引导态 / 动效偏好等全局所需）。
  useEffect(() => {
    void loadSettings();
  }, [loadSettings]);

  // 订阅引擎事件（事件驱动徽章/任务镜像，不轮询）；启动检查更新一次。
  useEffect(() => {
    let cleanup: (() => void) | undefined;
    void initEngine().then((fn) => {
      cleanup = fn;
    });
    void unwrap(commands.checkUpdateNow()).catch(() => {}); // 启动后台检查
    return () => cleanup?.();
  }, [initEngine]);

  // 切页刷新废纸篓徽章（清理/删除后即时反映，非定时轮询）。
  useEffect(() => {
    void refreshBadges();
  }, [route, refreshBadges]);

  return (
    <div className={`app ${platform === "win" ? "win" : "mac"}`}>
      <TitleBar />
      <div className="shell">
        <Sidebar />
        <div className="main">
          <ActivePage />
          <Onboarding />
        </div>
      </div>
      <CommandPalette />
      <HelpPanel />
    </div>
  );
}
