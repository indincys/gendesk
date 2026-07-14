import { useEffect, useState } from "react";

/**
 * 防抖值：`delay` 毫秒内没有新变化才把值放出去。
 * 用于搜索框——每敲一个字母就发一次全量查询，列表大了会明显卡。
 */
export function useDebouncedValue<T>(value: T, delay = 300): T {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => {
    const t = setTimeout(() => setDebounced(value), delay);
    return () => clearTimeout(t);
  }, [value, delay]);
  return debounced;
}
