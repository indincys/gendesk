import { CommandPalette } from "@/components/CommandPalette";
import { DailyBrief } from "@/components/DailyBrief";
import { HelpPanel } from "@/components/HelpPanel";
import { Onboarding } from "@/components/Onboarding";
import { Sidebar } from "@/components/app-shell/Sidebar";
import { TitleBar } from "@/components/app-shell/TitleBar";
import { commands, subscribeIntake, unwrap } from "@/lib/ipc";
import { useGlobalKeyboard } from "@/lib/keyboard";
import { ROUTE_BY_KEY } from "@/routes";
import { useEngineStore } from "@/stores/engine";
import { usePublishStore } from "@/stores/publish";
import { useSettingsStore } from "@/stores/settings";
import { useUiStore } from "@/stores/ui";
import { useV2vStore } from "@/stores/v2v";
import { useEffect } from "react";
import { toast } from "sonner";

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

  // 订阅生图工单收件（Claude Code / Codex 投单 → 自动建批）。
  //
  // 挂在外壳而不是设置页：收录是自动发生的，人当时未必停在设置页，而一个凭空出现的
  // 批次（以及一份静默失败的工单）都需要当场给一句解释。
  useEffect(() => {
    let cleanup: (() => void) | undefined;
    void subscribeIntake((e) => {
      for (const j of e.jobs) {
        if (j.status === "done") {
          // 一份工单可能拆出多个批次（各组比例/抽卡不同时）。
          const b = j.batchIds.map((x) => `#${x}`).join(" ");
          toast(`收到工单「${j.jobId}」· 批次 ${b} · ${j.taskCount} 张已开跑`);
        } else if (j.status === "hold") {
          // 超阈值：琥珀而不是红——它不是错误，是在等人表态。
          toast.warning(`工单「${j.jobId}」${j.message}，去设置页确认`);
        } else {
          // 失败必须是红的且带原因：静默失败的工单等于「投了单什么也没发生」。
          toast.error(`工单「${j.jobId}」收录失败：${j.message}`);
        }
      }
      // 批次已经建好在跑，徽章/任务镜像要立刻跟上。
      void refreshBadges();
    }).then((fn) => {
      cleanup = fn;
    });
    return () => cleanup?.();
  }, [refreshBadges]);

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
