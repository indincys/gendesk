import { reportFrontendError } from "@/lib/ipc";
import { Component, type ErrorInfo, type ReactNode } from "react";

interface Props {
  children: ReactNode;
}
interface State {
  error: Error | null;
}

/** 全局错误边界（执行计划 0.7）：捕获渲染错误并转发到 Rust 统一日志流。 */
export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo): void {
    void reportFrontendError({
      message: error.message,
      stack: error.stack ?? info.componentStack ?? undefined,
      source: "react-error-boundary",
    });
  }

  render(): ReactNode {
    if (this.state.error) {
      return (
        <div className="bigempty" style={{ height: "100vh" }}>
          <div className="fs13 fw6 t2">页面出现异常</div>
          <div className="fs12 t3">错误已记录到本地日志。可尝试重新加载。</div>
          <button type="button" className="btn mt10" onClick={() => window.location.reload()}>
            重新加载
          </button>
        </div>
      );
    }
    return this.props.children;
  }
}

/** 注册全局未捕获错误 / Promise rejection 转发。 */
export function installGlobalErrorForwarding(): void {
  window.addEventListener("error", (e) => {
    void reportFrontendError({
      message: e.message,
      stack: e.error?.stack,
      source: "window.onerror",
    });
  });
  window.addEventListener("unhandledrejection", (e) => {
    const reason = e.reason;
    void reportFrontendError({
      message: reason instanceof Error ? reason.message : String(reason),
      stack: reason instanceof Error ? reason.stack : undefined,
      source: "unhandledrejection",
    });
  });
}
