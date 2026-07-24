import { ErrorBoundary } from "@/components/ErrorBoundary";
import { AppShell } from "@/components/app-shell/AppShell";
import { Toaster } from "sonner";

export function App() {
  return (
    <ErrorBoundary>
      <AppShell />
      {/*
        sonner —— M3 将定制为原型深色胶囊样式（.tst）。
        右下角是各页面主操作按钮（开始生成 / 导出 / 确认 / 删除）的固定位置，黑色 toast
        压在上面，只能等它自己消失才点得到。改到右上，并把 top 让开 44px 标题栏
        （否则又压住「跳转」按钮与 Windows 窗控）；驻留 2.6s，长文案不至于一闪而过。
      */}
      <Toaster position="top-right" theme="dark" offset={{ top: 56, right: 16 }} duration={2600} />
    </ErrorBoundary>
  );
}
