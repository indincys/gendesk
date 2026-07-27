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
  /**
   * 把窗口叫到前面来。
   *
   * 用在「后台发生了一件需要人当场表态的事」上——超阈值工单就是唯一一例：
   * 投单那一刻人在 Claude Code 里，GenDesk 可能只是个后台图标，而那份工单
   * 在等的恰恰是人核对完挂靠再放行。失败静默：抢焦点失败不该变成一个报错。
   */
  async focus(): Promise<void> {
    if (!isTauri()) return;
    await getCurrentWindow()
      .setFocus()
      .catch(() => {});
  },
};
