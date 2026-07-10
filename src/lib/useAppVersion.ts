import { commands, unwrap } from "@/lib/ipc";
import { useEffect, useState } from "react";

/**
 * 应用当前版本（取自 Rust `app_version` → tauri.conf 的 package 版本，与 updater 比对同源）。
 * 避免在前端硬编码版本号导致与发布 tag 漂移。加载失败返回空串（页脚兜底）。
 */
export function useAppVersion(): string {
  const [version, setVersion] = useState("");
  useEffect(() => {
    void unwrap(commands.appVersion())
      .then(setVersion)
      .catch(() => {});
  }, []);
  return version;
}
