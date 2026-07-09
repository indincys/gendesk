import { ROUTE_BY_SHORTCUT } from "@/routes";
import { useUiStore } from "@/stores/ui";
import { useEffect } from "react";

/**
 * 全局键盘管理器（执行计划 0.5 / R9）。
 *
 * - ⌘/Ctrl+K：命令面板开/关
 * - ⌘/Ctrl+1–8：切页
 * - Esc：逐层关闭（命令面板 → 弹窗/抽屉 由各自组件处理）
 *
 * 铁律：在 INPUT / TEXTAREA / contentEditable 内不劫持普通按键（命令面板与
 * Esc 除外），避免打断用户输入。
 */
export function useGlobalKeyboard(): void {
  const togglePalette = useUiStore((s) => s.togglePalette);
  const closePalette = useUiStore((s) => s.closePalette);
  const go = useUiStore((s) => s.go);

  useEffect(() => {
    function onKeyDown(e: KeyboardEvent) {
      const mod = e.metaKey || e.ctrlKey;

      // ⌘K —— 全局有效，输入框内也响应。
      if (mod && (e.key === "k" || e.key === "K")) {
        e.preventDefault();
        togglePalette();
        return;
      }

      const { paletteOpen } = useUiStore.getState();
      // 命令面板打开时，Esc 关闭它；其余交给面板内部处理。
      if (paletteOpen) {
        if (e.key === "Escape") {
          e.preventDefault();
          closePalette();
        }
        return;
      }

      // 输入区域内不劫持后续快捷键。
      const target = e.target as HTMLElement | null;
      const tag = target?.tagName ?? "";
      const editable = target?.isContentEditable ?? false;
      if (tag === "INPUT" || tag === "TEXTAREA" || editable) return;

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
  }, [togglePalette, closePalette, go]);
}
