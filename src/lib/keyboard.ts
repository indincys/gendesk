import { ROUTE_BY_SHORTCUT } from "@/routes";
import { useUiStore } from "@/stores/ui";
import { useEffect } from "react";

/**
 * 全局键盘管理器（执行计划 0.5 / R9）。
 *
 * - ⌘/Ctrl+1–8：切页
 * - ⌘/ 或 ?：快捷键速查面板
 * - Esc：关闭速查面板（弹窗/抽屉由各自组件处理）
 *
 * v0.24.0 去掉了 ⌘K 命令面板：它能做的两件事各自有更近的入口 —— 跳转在侧栏
 * （十一条路由全列着，还带徽章），那几条「操作」在它们自己的页面上
 * （暂停队列在任务页、生成任务单与导入回执在发布计划页、重扫收件箱在资产页）。
 * 一个要先想起来才用得上的第二入口，实测从没人用过。
 *
 * 铁律：在 INPUT / TEXTAREA / contentEditable 内不劫持普通按键（Esc 除外），
 * 避免打断用户输入。
 */
export function useGlobalKeyboard(): void {
  const toggleHelp = useUiStore((s) => s.toggleHelp);
  const closeHelp = useUiStore((s) => s.closeHelp);
  const go = useUiStore((s) => s.go);

  useEffect(() => {
    function onKeyDown(e: KeyboardEvent) {
      const mod = e.metaKey || e.ctrlKey;

      // ⌘/ —— 快捷键速查面板（E39），全局有效。
      if (mod && e.key === "/") {
        e.preventDefault();
        toggleHelp();
        return;
      }

      // 帮助面板打开时，任意 Esc/? / ⌘/ 关闭。
      if (useUiStore.getState().helpOpen) {
        if (e.key === "Escape" || e.key === "?") {
          e.preventDefault();
          closeHelp();
        }
        return;
      }

      // 输入区域内不劫持后续快捷键。
      const target = e.target as HTMLElement | null;
      const tag = target?.tagName ?? "";
      const editable = target?.isContentEditable ?? false;
      if (tag === "INPUT" || tag === "TEXTAREA" || editable) return;

      // ? —— 快捷键速查面板（E39）；输入区已在上方放行。
      if (e.key === "?") {
        e.preventDefault();
        toggleHelp();
        return;
      }

      // ⌘1–8 切页。
      if (mod && e.key >= "1" && e.key <= "8") {
        const route = ROUTE_BY_SHORTCUT[Number(e.key)];
        if (route) {
          e.preventDefault();
          go(route);
        }
        return;
      }
    }

    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [toggleHelp, closeHelp, go]);
}
