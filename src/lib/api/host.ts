/** Host detection + shared Tauri/mirror invoke helpers. */

import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen as tauriListen } from "@tauri-apps/api/event";
import {
  isMirrorClient,
  mirrorEnsureTransport,
  mirrorInvoke,
  mirrorListen,
} from "../mirrorTransport";

export { isMirrorClient } from "../mirrorTransport";

export function isTauri(): boolean {
  return (
    typeof window !== "undefined" &&
    ("__TAURI_INTERNALS__" in window || "__TAURI__" in window)
  );
}

/** Desktop WebView path (not phone mirror). Prefer over bare `isTauri()` when AC6 matters. */
export function isDesktopHost(): boolean {
  return isTauri() && !isMirrorClient();
}

/**
 * True when a host backend is reachable — desktop Tauri IPC **or** mirror WS.
 *
 * Use this to gate work that only needs "a backend exists", e.g. creating a
 * session before the first send. Only valid for commands present in
 * `CMD_TO_METHOD` (mirrorTransport.ts); anything desktop-only must keep using
 * `isTauri()` / `isDesktopHost()` so mirror clients cannot reach it (AC6).
 */
export function hasHost(): boolean {
  return isTauri() || isMirrorClient();
}

/** Internal invoke — used by domain modules; not part of the public barrel surface. */
export async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (isMirrorClient()) {
    return mirrorInvoke<T>(cmd, args);
  }
  if (!isTauri()) throw new Error(`Tauri required: ${cmd}`);
  // Dispatch synchronously before the first await. Dynamic-importing here
  // yielded back to React on every click, letting a large workbench render run
  // before native dialogs and other Host commands were even enqueued.
  return tauriInvoke<T>(cmd, args);
}

export async function listen<T>(
  event: string,
  handler: (payload: T) => void,
): Promise<() => void> {
  if (isMirrorClient()) {
    await mirrorEnsureTransport();
    return mirrorListen<T>(event, handler);
  }
  if (!isTauri()) return () => {};
  const un = await tauriListen<T>(event, (e) => handler(e.payload));
  return un;
}
