import { useEffect, useRef } from "react";
import * as api from "@/lib/api";

const POLL_MS = 10_000;
const isVisible = () => document.visibilityState !== "hidden";

/** Desktop inventory only; catalog and journal updates use host events. */
export function useCliHistorySync(enabled = true): void {
  const inFlightRef = useRef<Promise<boolean> | null>(null);

  useEffect(() => {
    if (!enabled || !api.isDesktopHost()) return;
    let disposed = false;
    let waiting = false;
    let timer: number | undefined;

    const clearTimer = () => {
      window.clearTimeout(timer);
      timer = undefined;
    };
    const sync = async () => {
      if (disposed || !isVisible() || waiting) return;
      clearTimer();
      waiting = true;
      // Retain the flight across StrictMode effect cleanup/setup.
      const flight = inFlightRef.current ?? api.cliHistorySync().catch(() => false);
      inFlightRef.current = flight;
      try {
        await flight;
      } finally {
        if (inFlightRef.current === flight) inFlightRef.current = null;
        waiting = false;
        if (!disposed && isVisible()) {
          timer = window.setTimeout(() => void sync(), POLL_MS);
        }
      }
    };
    const onVisibility = () => {
      clearTimer();
      if (isVisible()) void sync();
    };
    const onFocus = () => void sync();
    window.addEventListener("focus", onFocus);
    document.addEventListener("visibilitychange", onVisibility);
    void sync();

    return () => {
      disposed = true;
      clearTimer();
      window.removeEventListener("focus", onFocus);
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, [enabled]);
}
