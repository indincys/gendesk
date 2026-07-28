import { ErrorBoundary } from "@/components/ErrorBoundary";
import { AppShell } from "@/components/app-shell/AppShell";
import { Toaster } from "sonner";

export function App() {
  return (
    <ErrorBoundary>
      <AppShell />
      {/*
        sonner。位置：右下角是各页面主操作按钮（开始生成 / 导出 / 确认 / 删除）的固定
        位置，toast 压在上面只能等它自己消失才点得到。改到右上，并把 top 让开 44px
        标题栏（否则压住 Windows 窗控）；驻留 2.6s，长文案不至于一闪而过。

        样式：v0.24.0 起跟应用同一套（`.tst`，见 globals.css）。此前是 sonner 自带的
        深色主题 —— 三块黑胶囊叠在一整屏浅色界面的右上角，比它们要传达的消息本身
        （「已刷新，暂无新出片」）显眼得多，而那正好是这一栏最不需要抢注意力的时候。
      */}
      <Toaster
        position="top-right"
        theme="light"
        offset={{ top: 56, right: 16 }}
        duration={2600}
        toastOptions={{ className: "tst" }}
      />
    </ErrorBoundary>
  );
}
