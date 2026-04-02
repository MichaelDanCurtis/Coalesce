import { useState, useEffect, useCallback, useRef } from "react";

export function useApi<T>(fetcher: () => Promise<T>, intervalMs = 10000) {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const consecutiveErrors = useRef(0);

  const refresh = useCallback(async () => {
    try {
      const result = await fetcher();
      setData(result);
      setError(null);
      consecutiveErrors.current = 0;
    } catch (e) {
      consecutiveErrors.current++;
      setError(e instanceof Error ? e.message : "Unknown error");
    } finally {
      setLoading(false);
    }
  }, [fetcher]);

  useEffect(() => {
    refresh();
    // Use dynamic interval: back off when proxy is down (up to 30s)
    let timer: ReturnType<typeof setTimeout>;
    const schedule = () => {
      const backoff = Math.min(consecutiveErrors.current, 3);
      const delay = intervalMs * (1 + backoff);
      timer = setTimeout(() => {
        refresh().then(schedule);
      }, delay);
    };
    schedule();
    return () => clearTimeout(timer);
  }, [refresh, intervalMs]);

  return { data, error, loading, refresh };
}
