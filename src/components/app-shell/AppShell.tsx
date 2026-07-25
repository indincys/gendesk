import { CommandPalette } from "@/components/CommandPalette";
import { DailyBrief } from "@/components/DailyBrief";
import { HelpPanel } from "@/components/HelpPanel";
import { Onboarding } from "@/components/Onboarding";
import { Sidebar } from "@/components/app-shell/Sidebar";
import { TitleBar } from "@/components/app-shell/TitleBar";
import { commands, unwrap } from "@/lib/ipc";
import { useGlobalKeyboard } from "@/lib/keyboard";
import { ROUTE_BY_KEY } from "@/routes";
import { useEngineStore } from "@/stores/engine";
import { usePublishStore } from "@/stores/publish";
import { useSettingsStore } from "@/stores/settings";
import { useUiStore } from "@/stores/ui";
import { useV2vStore } from "@/stores/v2v";
import { useEffect } from "react";

/** 应用外壳（执行计划 0.4）：标题栏 + 侧栏 + 主面板容器。 */
export function AppShell() {
  useGlobalKeyboard();
  const route = useUiStore((s) => s.route);
  const platform = useUiStore((s) => s.platform);
  const initEngine = useEngineStore((s) => s.init);
  const initPublish = usePublishStore((s) => s.init);
  const refreshBadges = useEngineStore((s) => s.refreshBadgeCounts);
  const refreshPubBadges = usePublishStore((s) => s.refreshBadges);
  const initV2v = useV2vStore((s) => s.init);
  const refreshV2v = useV2vStore((s) => s.refresh);
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

  // 订阅发布模块事件（徽章 / 收件箱收录）。
  useEffect(() => {
    let cleanup: (() => void) | undefined;
    void initPublish().then((fn) => {
      cleanup = fn;
    });
    return () => cleanup?.();
  }, [initPublish]);

  // 订阅视频流水线事件（侧栏徽章要在任何页面上都是对的）。
  useEffect(() => {
    let cleanup: (() => void) | undefined;
    void initV2v().then((fn) => {
      cleanup = fn;
    });
    return () => cleanup?.();
  }, [initV2v]);

  // 切页刷新废纸篓 + 发布 + 视频徽章（增删后即时反映，非定时轮询）。
  useEffect(() => {
    void refreshBadges();
    void refreshPubBadges();
    void refreshV2v();
  }, [route, refreshBadges, refreshPubBadges, refreshV2v]);

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
      <DailyBrief />
    </div>
  );
}
