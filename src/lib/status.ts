/** 8 态 → 5 视觉组映射（globals.css 徽章类）。CLAUDE.md 视觉规范。 */

export interface StatusVisual {
  label: string;
  /** 徽章类：b-gray / b-blue / b-amber / b-red / b-green */
  badgeClass: string;
  /** 生成中/重试中显示 spinner */
  spinner: boolean;
}

export function statusVisual(status: string): StatusVisual {
  switch (status) {
    case "q":
      return { label: "待生成", badgeClass: "b-gray", spinner: false };
    case "run":
      return { label: "生成中", badgeClass: "b-blue", spinner: true };
    case "retry":
      return { label: "重试中", badgeClass: "b-blue", spinner: true };
    case "rev":
      return { label: "待验收", badgeClass: "b-amber", spinner: false };
    case "pass":
      return { label: "已通过", badgeClass: "b-green", spinner: false };
    case "rej":
      return { label: "未通过", badgeClass: "b-gray", spinner: false };
    case "fail":
      return { label: "失败", badgeClass: "b-red", spinner: false };
    default:
      return { label: status, badgeClass: "b-gray", spinner: false };
  }
}

/** 发布模块：冷热分层 → 徽章。hot=琥珀 warm=蓝 cold=灰 gen=灰。 */
export function tierVisual(tier: string, isGeneral = false): { label: string; badgeClass: string } {
  if (isGeneral) return { label: "通用", badgeClass: "b-gray" };
  switch (tier) {
    case "hot":
      return { label: "热款", badgeClass: "b-amber" };
    case "warm":
      return { label: "温款", badgeClass: "b-blue" };
    case "cold":
      return { label: "冷款", badgeClass: "b-gray" };
    default:
      return { label: tier, badgeClass: "b-gray" };
  }
}

/** 发布模块：素材包派生生命周期 → 徽章。new=灰 active=绿 exhausted=琥珀 retired=灰。 */
export function packLifeVisual(derived: string): { label: string; badgeClass: string } {
  switch (derived) {
    case "new":
      return { label: "新入库", badgeClass: "b-gray" };
    case "active":
      return { label: "可用", badgeClass: "b-green" };
    case "exhausted":
      return { label: "冷却中", badgeClass: "b-amber" };
    case "retired":
      return { label: "退役", badgeClass: "b-gray" };
    default:
      return { label: derived, badgeClass: "b-gray" };
  }
}

/** 发布模块：单任务 5 视觉组。待执行=灰 已发布=绿 失败=红 疑似已发=琥珀 已取消=灰。 */
export function pubTaskVisual(status: string): { label: string; badgeClass: string } {
  switch (status) {
    case "pending":
      return { label: "待执行", badgeClass: "b-gray" };
    case "published":
      return { label: "已发布", badgeClass: "b-green" };
    case "failed":
      return { label: "失败", badgeClass: "b-red" };
    case "suspect":
      return { label: "疑似已发", badgeClass: "b-amber" };
    case "canceled":
      return { label: "已取消", badgeClass: "b-gray" };
    default:
      return { label: status, badgeClass: "b-gray" };
  }
}

/**
 * 平台 code → 中文。后端 `publish/platform.rs` 是权威单点；这里只用于**纯展示**
 * 且拿不到后端 zh 字段的场合（如 shortage_json 里的平台清单）。
 */
const PLATFORM_ZH: Record<string, string> = {
  douyin: "抖音",
  xhs: "小红书",
  kuaishou: "快手",
  shipinhao: "视频号",
  bilibili: "B站",
  general: "通用",
};

export function platformZh(code: string): string {
  return PLATFORM_ZH[code] ?? code;
}

/**
 * 发布模块：缺料/提示原因码 → 中文（后端 shortage_json 的 `reason` 单点映射）。
 * `timeout_backfill` 不是缺料，是「这个 SKU 今天为什么出现」的说明。
 */
export function shortageLabel(reason: string, platforms?: string[]): string {
  const plats = platforms?.length ? `（${platforms.map(platformZh).join("/")}）` : "";
  switch (reason) {
    case "no_pack":
      return "无可用素材包";
    case "no_title":
      return "无可用标题";
    case "no_body":
      return "无可用正文（图集需正文）";
    case "no_account":
      return `无可用账号${plats}`;
    case "dedup_partial":
      return `查重窗口内已发过${plats}，本次跳过这些平台`;
    case "timeout_backfill":
      return `昨日超时失败，今日已补排${plats}`;
    default:
      return reason;
  }
}

/**
 * 资产跑道（F3）→ 文案 + 颜色。null = 60 天内不会断（或不排期）。
 * ≤3 天红、≤7 天琥珀——静态阈值只说明「现在少」，倒计时才说明「什么时候断」。
 */
export function runwayVisual(days: number | null): { label: string; cls: string } {
  if (days == null) return { label: "充足", cls: "t3" };
  if (days <= 0) return { label: "已断料", cls: "terr" };
  if (days <= 3) return { label: `${days} 天见底`, cls: "terr" };
  if (days <= 7) return { label: `${days} 天见底`, cls: "twarn" };
  return { label: `${days} 天`, cls: "t3" };
}

/** 缺料项是否为真·缺料（false = 只是提示，不该进「缺料清单」横幅）。 */
export function isShortage(reason: string): boolean {
  return reason !== "timeout_backfill";
}

/** 发布模块：任务单状态。草稿=灰 已确认/已导出/回收中=蓝 已关闭=绿。 */
export function sheetVisual(status: string): { label: string; badgeClass: string } {
  switch (status) {
    case "draft":
      return { label: "草稿", badgeClass: "b-gray" };
    case "confirmed":
      return { label: "已确认", badgeClass: "b-blue" };
    case "exported":
      return { label: "已导出", badgeClass: "b-blue" };
    case "reconciling":
      return { label: "回收中", badgeClass: "b-blue" };
    case "closed":
      return { label: "已关闭", badgeClass: "b-green" };
    default:
      return { label: status, badgeClass: "b-gray" };
  }
}

/** 发布模块：失败六类 → 中文标签。 */
export function failKindLabel(kind?: string | null): string {
  switch (kind) {
    case "login":
      return "登录失效";
    case "risk":
      return "风控拦截";
    case "content":
      return "素材不合规";
    case "page":
      return "页面变更";
    case "timeout":
      return "网络超时";
    default:
      return "其他";
  }
}

/** 错误类型 → 中文标签。 */
export function errorLabel(errorType?: string | null): string {
  switch (errorType) {
    case "Timeout":
      return "读取超时";
    case "RateLimited":
      return "接口限流";
    case "ContentPolicy":
      return "提示词违规";
    case "Auth":
      return "Key 已失效";
    case "Interrupted":
      return "因中断请重试";
    default:
      return "生成失败";
  }
}
