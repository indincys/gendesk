import { useAppVersion } from "@/lib/useAppVersion";
import { cn } from "@/lib/utils";
import { ROUTES, type RouteDef } from "@/routes";
import { navBadges, useEngineStore } from "@/stores/engine";
import { usePublishStore } from "@/stores/publish";
import { modKeyLabel, useUiStore } from "@/stores/ui";
import { useV2vStore } from "@/stores/v2v";
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
  const pubBadges = usePublishStore((s) => s.badges);
  // 视频流水线徽章数「阻在人身上的」（待改写 + 待提交 + 待验收 + 失败），规则在 Rust 侧算。
  const v2vN = useV2vStore((s) => s.counts.actionable);
  // 成片徽章数「验收通过了却没交付到输出目录」的 —— 成片这条链上唯一一处会无声断掉的地方。
  const clipsN = useV2vStore((s) => s.counts.undelivered);
  const version = useAppVersion();
  const updateReady = useEngineStore((s) => s.updateReady);
  const updateVersion = useEngineStore((s) => s.updateVersion);

  const make = ROUTES.filter((r) => r.group === "make");
  const asset = ROUTES.filter((r) => r.group === "asset");
  const publish = ROUTES.filter((r) => r.group === "publish");
  const system = ROUTES.filter((r) => r.group === "system");

  const NavItem = ({ r }: { r: RouteDef }) => {
    const Icon = r.icon;
    const assetsN = pubBadges.unclaimed + pubBadges.warn;
    const planN = pubBadges.pendingSheets + pubBadges.pendingReconcile;
    const badge =
      r.key === "tasks" && badges.running > 0
        ? { cls: "nb-run", n: badges.running, spin: true }
        : r.key === "review" && badges.review > 0
          ? { cls: "nb-amb", n: badges.review, spin: false }
          : r.key === "assets" && assetsN > 0
            ? { cls: "nb-amb", n: assetsN, spin: false }
            : r.key === "plan" && planN > 0
              ? { cls: "nb-amb", n: planN, spin: false }
              : r.key === "v2v" && v2vN > 0
                ? { cls: "nb-amb", n: v2vN, spin: false }
                : r.key === "clips" && clipsN > 0
                  ? { cls: "nb-amb", n: clipsN, spin: false }
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
        {r.shortcut !== null && (
          <span className="kbd">
            {mod}
            {r.shortcut}
          </span>
        )}
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

      <div className="nsec">发布</div>
      {publish.map((r) => (
        <NavItem key={r.key} r={r} />
      ))}

      <div className="f1" />

      {/* 底部批次进度小卡占位（M2 引擎接入后由 batch://summary 驱动）。 */}

      {system.map((r) => (
        <NavItem key={r.key} r={r} />
      ))}

      <div className="sfoot">
        GenDesk{version ? ` v${version}` : ""} · 本地
        {updateReady && (
          <span
            className="verup"
            title={updateVersion ? `新版本 v${updateVersion} 待安装` : undefined}
          >
            有新版{updateVersion ? ` v${updateVersion}` : ""}
          </span>
        )}
      </div>
    </div>
  );
}
