import { cn } from "@/lib/utils";
import { ROUTES, type RouteDef } from "@/routes";
import { navBadges, useEngineStore } from "@/stores/engine";
import { modKeyLabel, useUiStore } from "@/stores/ui";
import { Search } from "lucide-react";
import { useShallow } from "zustand/react/shallow";

/** 212px 侧栏导航（执行计划 0.4）。制作 / 资产两组 + 底部废纸篓/设置。 */
export function Sidebar() {
  const route = useUiStore((s) => s.route);
  const go = useUiStore((s) => s.go);
  const openPalette = useUiStore((s) => s.openPalette);
  const platform = useUiStore((s) => s.platform);
  const mod = modKeyLabel(platform);
  const badges = useEngineStore(useShallow(navBadges));

  const make = ROUTES.filter((r) => r.group === "make");
  const asset = ROUTES.filter((r) => r.group === "asset");
  const system = ROUTES.filter((r) => r.group === "system");

  const NavItem = ({ r }: { r: RouteDef }) => {
    const Icon = r.icon;
    const badge =
      r.key === "tasks" && badges.running > 0
        ? { cls: "nb-run", n: badges.running, spin: true }
        : r.key === "review" && badges.review > 0
          ? { cls: "nb-amb", n: badges.review, spin: false }
          : r.key === "trash" && badges.trash > 0
            ? { cls: "", n: badges.trash, spin: false }
            : null;
    return (
      <div
        className={cn("nv", route === r.key && "on")}
        onClick={() => go(r.key)}
        onKeyDown={(e) => e.key === "Enter" && go(r.key)}
        role="button"
        tabIndex={0}
      >
        <Icon className="ic" />
        <span className="f1">{r.label}</span>
        {badge && (
          <span className={cn("nbdg", badge.cls)}>
            {badge.spin && <i className="spn s9" />}
            {badge.n}
          </span>
        )}
        <span className="kbd">
          {mod}
          {r.shortcut}
        </span>
      </div>
    );
  };

  return (
    <div className="side">
      <button type="button" className="sbtn" onClick={openPalette}>
        <Search className="ic12" />
        <span className="f1" style={{ textAlign: "left" }}>
          搜索或跳转…
        </span>
        <span className="kbd">{mod} K</span>
      </button>

      <div className="nsec">制作</div>
      {make.map((r) => (
        <NavItem key={r.key} r={r} />
      ))}

      <div className="nsec">资产</div>
      {asset.map((r) => (
        <NavItem key={r.key} r={r} />
      ))}

      <div className="f1" />

      {/* 底部批次进度小卡占位（M2 引擎接入后由 batch://summary 驱动）。 */}

      {system.map((r) => (
        <NavItem key={r.key} r={r} />
      ))}

      <div className="sfoot">GenDesk v0.1.0 · 本地</div>
    </div>
  );
}
