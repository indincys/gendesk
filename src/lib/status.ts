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
