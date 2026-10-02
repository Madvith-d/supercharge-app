import { useCallback, useEffect, useRef, useState } from "react";
import {
  isFakeMaximized,
  minimizeWindowReliable,
  scheduleCaptionButtonToggle,
  toggleMaximizeReliable,
} from "@/lib/windowChrome";

type CaptionAction = "minimize" | "toggleMaximize" | "close";
const CAPTION_STATE_SYNC_MS = 32;

export function useWindowCaptionControls(visible: boolean) {
  const [maximized, setMaximized] = useState(false);
  const dispatch = useRef<((action: CaptionAction) => void) | null>(null);
  const act = useCallback((action: CaptionAction) => dispatch.current?.(action), []);

  useEffect(() => {
    if (!visible) return;
    let disposed = false;
    let revision = 0;
    let syncing = false;
    let dirty = false;
    let running: CaptionAction | null = null;
    let pending: "minimize" | "close" | null = null;
    let togglePending = false;
    let toggleReady = false;
    let toggleTimer: ReturnType<typeof setTimeout> | undefined;
    let syncTimer: ReturnType<typeof setTimeout> | undefined;
    const unlisteners: (() => void)[] = [];
    const host = import("@tauri-apps/api/window").then(({ getCurrentWindow }) =>
      getCurrentWindow(),
    );

    function scheduleSync() {
      if (disposed || syncing || running || !dirty || syncTimer !== undefined) return;
      syncTimer = setTimeout(() => {
        syncTimer = undefined;
        void sync();
      }, CAPTION_STATE_SYNC_MS);
    }

    function requestSync() {
      if (disposed) return;
      revision += 1;
      dirty = true;
      scheduleSync();
    }

    async function sync() {
      if (disposed || syncing || running) return;
      syncing = true;
      dirty = false;
      const version = revision;
      try {
        const w = await host;
        if (disposed || version !== revision) return;
        const os = await w.isMaximized();
        if (!disposed && version === revision) {
          setMaximized(os || isFakeMaximized());
        }
      } catch (error) {
        console.warn("[WindowControls] caption state sync failed", error);
      } finally {
        syncing = false;
        scheduleSync();
      }
    }

    function cancelToggle() {
      clearTimeout(toggleTimer);
      toggleTimer = undefined;
      togglePending = false;
      toggleReady = false;
    }

    async function drain() {
      if (disposed || running) return;
      const action = pending ?? (togglePending && toggleReady ? "toggleMaximize" : null);
      if (!action) return;
      pending = null;
      if (action === "toggleMaximize") cancelToggle();
      running = action;
      revision += 1;
      dirty = true;
      try {
        const w = await host;
        if (disposed) return;
        if (action === "toggleMaximize") await toggleMaximizeReliable();
        else if (action === "minimize") await minimizeWindowReliable();
        else await w.close();
      } catch (error) {
        console.warn(`[WindowControls] ${action} failed`, error);
      } finally {
        running = null;
        if (!disposed) {
          requestSync();
          void drain();
        }
      }
    }

    dispatch.current = (action) => {
      if (disposed) return;
      if (action === "toggleMaximize") {
        if (pending || running === "minimize" || running === "close") return;
        // Unsent toggle pairs cancel; native command IPCs are serialized.
        togglePending = !togglePending;
        clearTimeout(toggleTimer);
        toggleReady = false;
        toggleTimer = undefined;
        if (togglePending) {
          toggleTimer = scheduleCaptionButtonToggle(() => {
            toggleTimer = undefined;
            toggleReady = true;
            void drain();
          });
        }
      } else {
        cancelToggle();
        if (running === "close" || pending === "close") return;
        if (running !== action) pending = action;
        void drain();
      }
    };

    function listen(register: () => Promise<() => void>, event: string) {
      void register().then((unlisten) => {
        if (disposed) unlisten();
        else unlisteners.push(unlisten);
      }).catch((error) => {
        console.warn(`[WindowControls] ${event} listener failed`, error);
      });
    }

    void host.then((w) => {
      if (disposed) return;
      listen(() => w.onResized(requestSync), "resize");
      listen(() => w.onMoved(requestSync), "move");
      void sync();
    }).catch((error) => {
      console.warn("[WindowControls] window initialization failed", error);
    });

    return () => {
      disposed = true;
      dispatch.current = null;
      cancelToggle();
      clearTimeout(syncTimer);
      pending = null;
      for (const unlisten of unlisteners) unlisten();
    };
  }, [visible]);

  return { maximized, act };
}
