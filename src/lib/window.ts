import { isTauri } from "@/lib/ipc";
import { getCurrentWindow } from "@tauri-apps/api/window";

/** 自绘窗控动作（Windows 无边框窗口用；macOS 用原生交通灯）。 */
export const windowControls = {
  async minimize(): Promise<void> {
    if (!isTauri()) return;
    await getCurrentWindow().minimize();
  },
  async toggleMaximize(): Promise<void> {
    if (!isTauri()) return;
    await getCurrentWindow().toggleMaximize();
  },
  async close(): Promise<void> {
    if (!isTauri()) return;
    await getCurrentWindow().close();
  },
};
