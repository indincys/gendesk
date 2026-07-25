import { ROUTES } from "@/routes";
import { modKeyLabel, useUiStore } from "@/stores/ui";

/** 快捷键速查面板（E39）：? 或 ⌘/ 呼出，列出全局与验收页快捷键。 */
export function HelpPanel() {
  const open = useUiStore((s) => s.helpOpen);
  const close = useUiStore((s) => s.closeHelp);
  const platform = useUiStore((s) => s.platform);
  if (!open) return null;
  const mod = modKeyLabel(platform);

  const globalKeys: [string, string][] = [
    [`${mod}K`, "命令面板"],
    [`${mod}/`, "本速查面板"],
    ["?", "本速查面板"],
    // 无数字快捷键的页面不列进速查表——列一个空的修饰键比不列更让人困惑。
    ...ROUTES.filter((r) => r.shortcut !== null).map(
      (r) => [`${mod}${r.shortcut}`, r.label] as [string, string],
    ),
  ];
  const reviewKeys: [string, string][] = [
    ["↑ ↓ ← →", "移动焦点"],
    ["空格", "选中 / 大图内切换参考图"],
    ["⏎", "通过焦点项 / 所选"],
    ["⌫", "不通过"],
    ["R", "重试并微调提示词"],
    ["S", "标记待定"],
    ["Z / 双击", "进入大图"],
    [`${mod}A`, "全选当前筛选"],
    ["⇧ 点选", "范围多选"],
    ["Esc", "退出大图 / 关闭面板"],
  ];

  return (
    <div className="ovl" onClick={close}>
      <div className="mdl w640" onClick={(e) => e.stopPropagation()}>
        <div className="mhead">
          <span className="fw6 fs13">快捷键速查</span>
          <div className="f1" />
          <button type="button" className="icb" onClick={close}>
            ×
          </button>
        </div>
        <div className="mlist" style={{ display: "flex", gap: 24, padding: 16 }}>
          <KeyList title="全局" items={globalKeys} />
          <KeyList title="验收页" items={reviewKeys} />
        </div>
      </div>
    </div>
  );
}

function KeyList({ title, items }: { title: string; items: [string, string][] }) {
  return (
    <div className="f1" style={{ minWidth: 0 }}>
      <div className="fs11 fw6 t3" style={{ letterSpacing: ".05em", marginBottom: 8 }}>
        {title}
      </div>
      <div className="col" style={{ gap: 7 }}>
        {items.map(([k, label]) => (
          <div key={`${k}-${label}`} className="fx ac gap8">
            <span className="kbd" style={{ minWidth: 44, textAlign: "center" }}>
              {k}
            </span>
            <span className="fs12 t2">{label}</span>
          </div>
        ))}
      </div>
    </div>
  );
}
