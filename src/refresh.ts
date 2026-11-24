import { useCallback, useEffect, useRef } from "react";

export function observeRefresh(refresh: () => void, interval?: number) {
  let timer: ReturnType<typeof setInterval> | undefined;
  const onFocus = () => {
    if (!document.hidden) refresh();
  };
  const schedule = () => {
    clearInterval(timer);
    if (!document.hidden && interval !== undefined) {
      timer = setInterval(onFocus, interval);
    }
  };
  const onVisibilityChange = () => {
    schedule();
    onFocus();
  };
  schedule();
  window.addEventListener("focus", onFocus);
  document.addEventListener("visibilitychange", onVisibilityChange);
  return () => {
    clearInterval(timer);
    window.removeEventListener("focus", onFocus);
    document.removeEventListener("visibilitychange", onVisibilityChange);
  };
}

export function useRefreshRequest(
  load: (signal: AbortSignal) => Promise<void>,
  interval?: number,
) {
  const latest = useRef(load);
  latest.current = load;
  const controller = useRef<AbortController | null>(null);
  const pending = useRef<Promise<void> | null>(null);
  const queued = useRef<Promise<void> | null>(null);

  const refresh = useCallback((): Promise<void> => {
    const signal = controller.current?.signal;
    if (!signal || signal.aborted) return Promise.resolve();
    if (queued.current) return queued.current;
    if (pending.current) {
      queued.current = pending.current.then(() => {
        queued.current = null;
        return refresh();
      });
      return queued.current;
    }
    pending.current = latest.current(signal).finally(() => {
      pending.current = null;
    });
    return pending.current;
  }, []);

  useEffect(() => {
    const current = new AbortController();
    controller.current = current;
    void refresh();
    return () => current.abort();
  }, [refresh]);

  useEffect(
    () =>
      observeRefresh(() => {
        if (!pending.current && !queued.current) void refresh();
      }, interval),
    [refresh, interval],
  );
  return refresh;
}
