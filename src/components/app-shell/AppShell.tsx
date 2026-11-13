import { HelpPanel } from "@/components/HelpPanel";
import { Onboarding } from "@/components/Onboarding";
import { Sidebar } from "@/components/app-shell/Sidebar";
import { TitleBar } from "@/components/app-shell/TitleBar";
import { commands, subscribeIntake, subscribeLivePhoto, unwrap } from "@/lib/ipc";
import { useGlobalKeyboard } from "@/lib/keyboard";
import { ROUTE_BY_KEY } from "@/routes";
import { useEngineStore } from "@/stores/engine";
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
  const refreshBadges = useEngineStore((s) => s.refreshBadgeCounts);
  const initV2v = useV2vStore((s) => s.init);
  const refreshV2v = useV2vStore((s) => s.refreshCounts);
  const loadSettings = useSettingsStore((s) => s.load);
  const ActivePage = ROUTE_BY_KEY[route].component;

  // 加载设置（E13 引导态 / 动效偏好等全局所需）。
  useEffect(() => {
    void loadSettings();
  }, [loadSettings]);

  useEffect(() => {
    const subscription = subscribeLivePhoto((error) => {
      if (error) toast.error("实况图转换失败，可在设置中重试", { id: "live-photo-error" });
    });
    return () => void subscription.then((unlisten) => unlisten());
  }, []);

  // 订阅引擎事件（事件驱动徽章/任务镜像，不轮询）；启动检查更新一次。
  useEffect(() => {
    let disposed = false;
    let cleanup: (() => void) | undefined;
    void initEngine().then((fn) => {
      // StrictMode 会“挂载 → 立即清理 → 再挂载”来找副作用。init 是异步的，第一次
      // 清理可能早于 listen 完成；若直接赋值，第一套监听永远没人释放，高频事件翻倍。
      if (disposed) fn();
      else cleanup = fn;
    });
    // 更新检查由 Rust 定时跑；这里只补读启动早期已经发出的状态。
    void unwrap(commands.getUpdateState())
      .then(useEngineStore.getState().applyUpdateState)
      .catch(() => {});
    return () => {
      disposed = true;
      cleanup?.();
    };
  }, [initEngine]);

  // 订阅视频流水线事件（侧栏徽章要在任何页面上都是对的）。
  useEffect(() => {
    let cleanup: (() => void) | undefined;
    void initV2v().then((fn) => {
      cleanup = fn;
    });
    return () => cleanup?.();
  }, [initV2v]);

  // 订阅生图工单收件（Agent 经 `gendesk submit` 投单 → 建批开跑）。
  //
  // 挂在外壳而不是某个页面：投单由 Agent 触发，人当时未必停在哪一页，而一个凭空出现的
  // 批次（以及一份静默失败的工单）都需要当场给一句解释。
  useEffect(() => {
    let cleanup: (() => void) | undefined;
    void subscribeIntake((e) => {
      for (const j of e.jobs) {
        if (j.status === "done") {
          // 一份工单可能拆出多个批次（各组比例不同时）。
          const b = j.batchIds.map((x) => `#${x}`).join(" ");
          // 外部成图导入单完成时带说明（没有「开跑」，图已在待验收）；生图单完成时 message 为空。
          toast(
            j.message
              ? `收到工单「${j.jobId}」· 批次 ${b} · ${j.message}`
              : `收到工单「${j.jobId}」· 批次 ${b} · ${j.taskCount} 张已开跑`,
          );
        } else if (j.status === "hold") {
          // 超阈值：琥珀而不是红——它不是错误，是在等人表态。放行只在 Agent 里经用户同意后
          // `gendesk confirm`，应用这边只提示，不设第二个确认入口。
          toast.warning(`工单「${j.jobId}」${j.message}，等你在 Agent 里同意后确认开跑`);
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

  // 切页刷新废纸篓 + 视频徽章（增删后即时反映，非定时轮询）。
  // biome-ignore lint/correctness/useExhaustiveDependencies: route 变化本身就是刷新信号
  useEffect(() => {
    void refreshBadges();
    void refreshV2v();
  }, [route, refreshBadges, refreshV2v]);

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
      <HelpPanel />
    </div>
  );
}
