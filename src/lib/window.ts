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
  /**
   * 「关闭」现在的语义是**收起窗口**，不是退出应用。
   *
   * `close()` 走的是窗口的 `CloseRequested`，而 Rust 侧一律拦下来改成 hide
   * （`shell::install_close_to_hide`）——即梦轮询器与 CLI 控制通道得继续跑，
   * 非 VIP 队列一排就是十几个小时，那段时间恰恰不需要界面开着。
   * 真退出在应用菜单/托盘的「退出 GenDesk」里。
   */
  async close(): Promise<void> {
    if (!isTauri()) return;
    await getCurrentWindow().close();
  },
};
