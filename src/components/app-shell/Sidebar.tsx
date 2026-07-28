import { V2vNavCards } from "@/features/v2v/V2vNavCards";
import { useAppVersion } from "@/lib/useAppVersion";
import { cn } from "@/lib/utils";
import { type NavGroup, ROUTES, type RouteDef } from "@/routes";
import { navBadges, useEngineStore } from "@/stores/engine";
import { usePublishStore } from "@/stores/publish";
import { modKeyLabel, useUiStore } from "@/stores/ui";
import { useV2vStore } from "@/stores/v2v";
import { useShallow } from "zustand/react/shallow";

/**
 * 246px 侧栏导航（v0.24.0 按 Claude Design 原型重做）。
 *
 * 三处与旧版的差别，各自有理由：
 *
 * 1. **没有行内图标**。十一条路由里有八条的图标是同义反复（「废纸篓」旁边一个垃圾桶），
 *    而剩下三条（Clapperboard / Film / Layers）反倒得靠标签才认得出。去掉之后
 *    分组色轨承担辨识，一眼扫的是三块颜色而不是十一个灰色小图形。
 * 2. **搜索框搬到顶栏**。⌘K 是全局的，长在侧栏顶端会读成「搜这一列」。
 * 3. **视频流水线选中时，下面展开两张筛选卡**（`V2vNavCards`）。主轴与通道从页里
 *    搬到这儿，工作台那一屏才腾得出地方给大预览。
 */

/** 分组头 —— 色条 + 组名，组内还有一条同色左轨。 */
const GROUPS: { key: NavGroup; label: string }[] = [
  { key: "make", label: "制作" },
  { key: "asset", label: "资产" },
  { key: "publish", label: "发布" },
];

export function Sidebar() {
  const route = useUiStore((s) => s.route);
  const go = useUiStore((s) => s.go);
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

  const NavItem = ({ r }: { r: RouteDef }) => {
    const assetsN = pubBadges.unclaimed + pubBadges.warn;
    const planN = pubBadges.pendingSheets + pubBadges.pendingReconcile;
    const badge =
      r.key === "tasks" && badges.running > 0
        ? { cls: "nb-run", n: `${badges.running}`, spin: true }
        : r.key === "review" && badges.review > 0
          ? { cls: "nb-amb", n: `${badges.review}`, spin: false }
          : r.key === "assets" && assetsN > 0
            ? { cls: "nb-amb", n: `${assetsN}`, spin: false }
            : r.key === "plan" && planN > 0
              ? { cls: "nb-amb", n: `${planN}`, spin: false }
              : r.key === "v2v" && v2vN > 0
                ? { cls: "nb-amb", n: `${v2vN}`, spin: false }
                : // 成片这一格写成一句话而不是一个数：「4」答不出「4 个什么」，
                  // 而它说的恰恰是一件需要人动手补的事（拷贝失败不回滚验收）。
                  r.key === "clips" && clipsN > 0
                  ? { cls: "nb-amb", n: `${clipsN} 条未交付`, spin: false }
                  : r.key === "trash" && badges.trash > 0
                    ? { cls: "", n: `${badges.trash}`, spin: false }
                    : null;
    return (
      <div
        className={cn("nv", route === r.key && "on")}
        onClick={() => go(r.key)}
        onKeyDown={(e) => e.key === "Enter" && go(r.key)}
        role="button"
        tabIndex={0}
      >
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
      {GROUPS.map((g) => (
        <div key={g.key}>
          <div className={cn("nsec", g.key)}>
            <span className="bar" />
            {g.label}
          </div>
          <div className={cn("ngrp", g.key)}>
            {ROUTES.filter((r) => r.group === g.key).map((r) => (
              <div key={r.key}>
                <NavItem r={r} />
                {/* 卡挂在「视频流水线」这一行下面而不是列表末尾：它们筛的是那一页，
                    离得远就成了两件看不出关系的东西。 */}
                {r.key === "v2v" && route === "v2v" && <V2vNavCards />}
              </div>
            ))}
          </div>
        </div>
      ))}

      <div className="f1" />

      <div className="nsys">
        {ROUTES.filter((r) => r.group === "system").map((r) => (
          <NavItem key={r.key} r={r} />
        ))}
      </div>

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
