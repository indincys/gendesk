import { CommandPalette } from "@/components/CommandPalette";
import { Sidebar } from "@/components/app-shell/Sidebar";
import { TitleBar } from "@/components/app-shell/TitleBar";
import { useGlobalKeyboard } from "@/lib/keyboard";
import { ROUTE_BY_KEY } from "@/routes";
import { useUiStore } from "@/stores/ui";

/** 应用外壳（执行计划 0.4）：标题栏 + 侧栏 + 主面板容器。 */
export function AppShell() {
  useGlobalKeyboard();
  const route = useUiStore((s) => s.route);
  const platform = useUiStore((s) => s.platform);
  const ActivePage = ROUTE_BY_KEY[route].component;

  return (
    <div className={`app ${platform === "win" ? "win" : "mac"}`}>
      <TitleBar />
      <div className="shell">
        <Sidebar />
        <div className="main">
          <ActivePage />
        </div>
      </div>
      <CommandPalette />
    </div>
  );
}
