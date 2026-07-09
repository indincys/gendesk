import { ErrorBoundary } from "@/components/ErrorBoundary";
import { AppShell } from "@/components/app-shell/AppShell";
import { Toaster } from "sonner";

export function App() {
  return (
    <ErrorBoundary>
      <AppShell />
      {/* sonner —— M3 将定制为原型深色胶囊样式（.tst） */}
      <Toaster position="bottom-right" theme="dark" offset={16} />
    </ErrorBoundary>
  );
}
