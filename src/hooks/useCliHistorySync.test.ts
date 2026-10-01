/** @vitest-environment jsdom */
import { createElement, StrictMode } from "react";
import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as api from "@/lib/api";
import { useCliHistorySync } from "./useCliHistorySync";

function visibility(value: "visible" | "hidden") {
  vi.spyOn(document, "visibilityState", "get").mockReturnValue(value);
  document.dispatchEvent(new Event("visibilitychange"));
}

async function flush() {
  await act(async () => { await Promise.resolve(); });
}

describe("useCliHistorySync", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.spyOn(document, "visibilityState", "get").mockReturnValue("visible");
    vi.spyOn(api, "isDesktopHost").mockReturnValue(true);
    vi.spyOn(api, "cliHistorySync").mockResolvedValue(false);
    vi.spyOn(api, "sessionsList");
    vi.spyOn(api, "trayRefresh");
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  it("syncs at startup and polls without catalog or tray work on unchanged history", async () => {
    renderHook(() => useCliHistorySync());
    expect(api.cliHistorySync).toHaveBeenCalledTimes(1);
    await flush();
    await act(async () => { await vi.advanceTimersByTimeAsync(30_000); });
    expect(api.cliHistorySync).toHaveBeenCalledTimes(4);
    expect(api.sessionsList).not.toHaveBeenCalled();
    expect(api.trayRefresh).not.toHaveBeenCalled();
  });

  it("pauses hidden polling and syncs on visibility and focus", async () => {
    renderHook(() => useCliHistorySync());
    await flush();
    act(() => visibility("hidden"));
    expect(vi.getTimerCount()).toBe(0);
    await act(async () => {
      window.dispatchEvent(new Event("focus"));
      await vi.advanceTimersByTimeAsync(60_000);
    });
    expect(api.cliHistorySync).toHaveBeenCalledTimes(1);
    await act(async () => visibility("visible"));
    expect(api.cliHistorySync).toHaveBeenCalledTimes(2);
    await act(async () => window.dispatchEvent(new Event("focus")));
    expect(api.cliHistorySync).toHaveBeenCalledTimes(3);
  });

  it("defers hidden startup until visible and respects the listener readiness gate", async () => {
    visibility("hidden");
    const { rerender } = renderHook(({ enabled }) => useCliHistorySync(enabled), {
      initialProps: { enabled: false },
    });
    rerender({ enabled: true });
    expect(api.cliHistorySync).not.toHaveBeenCalled();
    await act(async () => visibility("visible"));
    expect(api.cliHistorySync).toHaveBeenCalledTimes(1);
  });

  it("retries errors on the next bounded poll and on focus", async () => {
    vi.mocked(api.cliHistorySync)
      .mockRejectedValueOnce(new Error("temporary"))
      .mockRejectedValueOnce(new Error("temporary"));
    renderHook(() => useCliHistorySync());
    await flush();
    await act(async () => { await vi.advanceTimersByTimeAsync(10_000); });
    expect(api.cliHistorySync).toHaveBeenCalledTimes(2);
    await act(async () => window.dispatchEvent(new Event("focus")));
    expect(api.cliHistorySync).toHaveBeenCalledTimes(3);
    expect(vi.getTimerCount()).toBe(1);
  });

  it("coalesces duplicate triggers and StrictMode startup while a request is pending", async () => {
    let resolve!: (changed: boolean) => void;
    vi.mocked(api.cliHistorySync).mockReturnValueOnce(new Promise((done) => { resolve = done; }));
    renderHook(() => useCliHistorySync(), {
      wrapper: ({ children }) => createElement(StrictMode, null, children),
    });
    await act(async () => {
      window.dispatchEvent(new Event("focus"));
      visibility("hidden");
      visibility("visible");
      await vi.advanceTimersByTimeAsync(60_000);
    });
    expect(api.cliHistorySync).toHaveBeenCalledTimes(1);
    expect(vi.getTimerCount()).toBe(0);
    await act(async () => resolve(true));
    expect(vi.getTimerCount()).toBe(1);
    await act(async () => { await vi.advanceTimersByTimeAsync(10_000); });
    expect(api.cliHistorySync).toHaveBeenCalledTimes(2);
  });

  it("removes listeners and timers and ignores late completion after disposal", async () => {
    let resolve!: (changed: boolean) => void;
    vi.mocked(api.cliHistorySync).mockReturnValueOnce(new Promise((done) => { resolve = done; }));
    const { unmount } = renderHook(() => useCliHistorySync());
    unmount();
    await act(async () => {
      resolve(true);
      window.dispatchEvent(new Event("focus"));
      visibility("visible");
      await vi.advanceTimersByTimeAsync(60_000);
    });
    expect(api.cliHistorySync).toHaveBeenCalledTimes(1);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("clears a scheduled poll when disabled", async () => {
    const { rerender } = renderHook(({ enabled }) => useCliHistorySync(enabled), {
      initialProps: { enabled: true },
    });
    await flush();
    expect(vi.getTimerCount()).toBe(1);
    rerender({ enabled: false });
    expect(vi.getTimerCount()).toBe(0);
    window.dispatchEvent(new Event("focus"));
    expect(api.cliHistorySync).toHaveBeenCalledTimes(1);
  });

  it("does not run inventory in a browser preview or mirror client", () => {
    vi.mocked(api.isDesktopHost).mockReturnValue(false);
    renderHook(() => useCliHistorySync());
    expect(api.cliHistorySync).not.toHaveBeenCalled();
    expect(vi.getTimerCount()).toBe(0);
  });
});
