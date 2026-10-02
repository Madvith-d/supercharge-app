import { readFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  CAPTION_BUTTON_TOGGLE_DEFER_MS,
  isFakeMaximized,
  minimizeWindowReliable,
  resetWindowChromeTestState,
  toggleMaximizeReliable,
  maximizeLooksNoop,
  osMaximizeWaitMs,
  scheduleCaptionButtonToggle,
  shouldAcceptTitlebarMaximize,
  shouldFakeMaximizeFallback,
  tauriDragRegion,
  TITLEBAR_MAXIMIZE_DEBOUNCE_MS,
} from "./windowChrome";

const host = vi.hoisted(() => ({
  platform: "win",
  captionAction: vi.fn(),
  minimize: vi.fn(),
  isMaximized: vi.fn(),
  toggleMaximize: vi.fn(),
  maximize: vi.fn(),
  unmaximize: vi.fn(),
  outerPosition: vi.fn(),
  outerSize: vi.fn(),
  scaleFactor: vi.fn(),
  setPosition: vi.fn(),
  setSize: vi.fn(),
  currentMonitor: vi.fn(),
}));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => host,
  currentMonitor: host.currentMonitor,
}));
vi.mock("@/lib/appPlatform", () => ({ detectAppPlatform: () => host.platform }));
vi.mock("@/lib/api/system", () => ({ windowCaptionAction: host.captionAction }));

describe("toggleMaximizeReliable", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.resetAllMocks();
    resetWindowChromeTestState();
    host.platform = "win";
    host.isMaximized.mockResolvedValue(false);
    host.toggleMaximize.mockResolvedValue(undefined);
    host.maximize.mockResolvedValue(undefined);
    host.unmaximize.mockResolvedValue(undefined);
    host.outerPosition.mockResolvedValue({ x: 100, y: 80 });
    host.outerSize.mockResolvedValue({ width: 1600, height: 1200 });
    host.scaleFactor.mockResolvedValue(2);
    host.setPosition.mockResolvedValue(undefined);
    host.setSize.mockResolvedValue(undefined);
    host.currentMonitor.mockResolvedValue({
      scaleFactor: 2,
      workArea: { position: { x: 0, y: 40 }, size: { width: 3840, height: 2080 } },
    });
    vi.spyOn(console, "warn").mockImplementation(() => {});
  });
  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it("delegates Windows toggling to the native command without a frontend state query", async () => {
    await toggleMaximizeReliable();
    expect(host.captionAction).toHaveBeenCalledExactlyOnceWith("toggleMaximize");
    expect(host.toggleMaximize).not.toHaveBeenCalled();
    expect(host.isMaximized).not.toHaveBeenCalled();
    expect(host.maximize).not.toHaveBeenCalled();
    expect(host.unmaximize).not.toHaveBeenCalled();
    expect(host.setSize).not.toHaveBeenCalled();
    expect(host.setPosition).not.toHaveBeenCalled();
  });

  it("propagates a rejected Windows toggle instead of returning an intended state", async () => {
    const error = new Error("toggle denied");
    host.captionAction.mockRejectedValueOnce(error);
    await expect(toggleMaximizeReliable()).rejects.toBe(error);
    expect(host.isMaximized).not.toHaveBeenCalled();
  });

  it("routes Windows minimize directly to the caption command", async () => {
    await minimizeWindowReliable();
    expect(host.captionAction).toHaveBeenCalledExactlyOnceWith("minimize");
    expect(host.minimize).not.toHaveBeenCalled();
    expect(host.isMaximized).not.toHaveBeenCalled();
  });

  it.each(["mac", "linux"])("preserves native minimize on %s", async (platform) => {
    host.platform = platform;
    await minimizeWindowReliable();
    expect(host.minimize).toHaveBeenCalledTimes(1);
    expect(host.captionAction).not.toHaveBeenCalled();
  });

  it("propagates a rejected Windows minimize", async () => {
    const error = new Error("minimize denied");
    host.captionAction.mockRejectedValueOnce(error);
    await expect(minimizeWindowReliable()).rejects.toBe(error);
  });

  it.each([false, true])("preserves macOS maximize/restore without work-area fill (%s)", async (was) => {
    host.platform = "mac";
    host.isMaximized.mockResolvedValue(was);
    await expect(toggleMaximizeReliable()).resolves.toBe(!was);
    expect(was ? host.unmaximize : host.maximize).toHaveBeenCalledTimes(1);
    expect(host.toggleMaximize).not.toHaveBeenCalled();
    expect(host.setSize).not.toHaveBeenCalled();
  });

  it("preserves the Linux work-area fallback and logical restore bounds", async () => {
    host.platform = "linux";
    const toggle = toggleMaximizeReliable();
    await vi.dynamicImportSettled();
    await vi.advanceTimersByTimeAsync(100);
    await expect(toggle).resolves.toBe(true);
    expect(isFakeMaximized()).toBe(true);
    expect(host.setPosition).toHaveBeenLastCalledWith(expect.objectContaining({ x: 0, y: 20 }));
    expect(host.setSize).toHaveBeenLastCalledWith(expect.objectContaining({ width: 1920, height: 1040 }));
    await expect(toggleMaximizeReliable()).resolves.toBe(false);
    expect(isFakeMaximized()).toBe(false);
    expect(host.setPosition).toHaveBeenLastCalledWith(expect.objectContaining({ x: 50, y: 40 }));
    expect(host.setSize).toHaveBeenLastCalledWith(expect.objectContaining({ width: 800, height: 600 }));
    expect(host.toggleMaximize).not.toHaveBeenCalled();
  });

  it("retains fake-maximize state and restore bounds when Linux restore rejects", async () => {
    host.platform = "linux";
    const toggle = toggleMaximizeReliable();
    await vi.dynamicImportSettled();
    await vi.advanceTimersByTimeAsync(100);
    await toggle;
    const error = new Error("resize denied");
    host.setSize.mockRejectedValueOnce(error);
    await expect(toggleMaximizeReliable()).rejects.toBe(error);
    expect(isFakeMaximized()).toBe(true);
    await expect(toggleMaximizeReliable()).resolves.toBe(false);
    expect(host.setSize).toHaveBeenLastCalledWith(expect.objectContaining({ width: 800, height: 600 }));
  });

  it("still fills the Linux work area when native maximize rejects", async () => {
    host.platform = "linux";
    const error = new Error("unsupported");
    host.maximize.mockRejectedValueOnce(error);
    const toggle = toggleMaximizeReliable();
    await vi.dynamicImportSettled();
    await vi.advanceTimersByTimeAsync(100);
    await expect(toggle).resolves.toBe(true);
    expect(console.warn).toHaveBeenCalledWith("[windowChrome] maximize failed; trying Linux fallback", error);
  });

  it("uses Linux native maximize when it works", async () => {
    host.platform = "linux";
    host.isMaximized.mockResolvedValueOnce(false).mockResolvedValue(true);
    await expect(toggleMaximizeReliable()).resolves.toBe(true);
    expect(isFakeMaximized()).toBe(false);
    expect(host.setSize).not.toHaveBeenCalled();
  });
});

describe("shouldAcceptTitlebarMaximize", () => {
  it("debounces the second click of a drag-region pair", () => {
    expect(shouldAcceptTitlebarMaximize(1000, 1000)).toBe(false);
    expect(shouldAcceptTitlebarMaximize(1000, 1000 + 399)).toBe(false);
    expect(
      shouldAcceptTitlebarMaximize(1000, 1000 + TITLEBAR_MAXIMIZE_DEBOUNCE_MS),
    ).toBe(true);
  });
});

describe("maximizeLooksNoop", () => {
  it("is true only when the flag did not flip", () => {
    expect(maximizeLooksNoop(false, false)).toBe(true);
    expect(maximizeLooksNoop(true, true)).toBe(true);
    expect(maximizeLooksNoop(false, true)).toBe(false);
    expect(maximizeLooksNoop(true, false)).toBe(false);
  });
});

describe("tauriDragRegion", () => {
  it("enables JS start_dragging on every desktop host (#1075)", () => {
    expect(tauriDragRegion("win")).toBe("deep");
    expect(tauriDragRegion("mac")).toBe("deep");
    expect(tauriDragRegion("linux")).toBe("deep");
    expect(tauriDragRegion("other")).toBe("deep");
  });

  it("keeps CSS compositor caption drag on drag-region attributes", () => {
    const sidebar = readFileSync(
      join(__dirname, "../styles/sidebar.part1.css"),
      "utf8",
    );
    const settings = readFileSync(
      join(__dirname, "../styles/settings.part1.css"),
      "utf8",
    );
    expect(sidebar).toMatch(
      /\[data-tauri-drag-region\][^{]*\{[^}]*-webkit-app-region:\s*drag/,
    );
    expect(sidebar).not.toMatch(
      /\[data-tauri-drag-region="false"\][^{]*\{[^}]*no-drag/,
    );
    expect(sidebar).not.toMatch(
      /html\.platform-win \[data-tauri-drag-region\][\s\S]{0,80}no-drag/,
    );
    expect(settings).not.toMatch(
      /html\.platform-win \.settings-page__chrome[\s\S]{0,120}no-drag/,
    );
  });

  it("keeps portaled GlassModal overlays off the window-drag region (#844)", () => {
    const chrome = readFileSync(
      join(__dirname, "../styles/chat.part4.css"),
      "utf8",
    );
    expect(chrome).toMatch(
      /\.overlay\s*\{[^}]*-webkit-app-region:\s*no-drag/s,
    );
    expect(chrome).toMatch(
      /\.modal\s*\{[^}]*-webkit-app-region:\s*no-drag/s,
    );
  });
});

describe("shouldFakeMaximizeFallback", () => {
  it("is Linux-only — Windows/mac must use OS maximize, not setSize fill", () => {
    expect(shouldFakeMaximizeFallback("linux")).toBe(true);
    expect(shouldFakeMaximizeFallback("win")).toBe(false);
    expect(shouldFakeMaximizeFallback("mac")).toBe(false);
    expect(shouldFakeMaximizeFallback("other")).toBe(false);
  });
});

describe("osMaximizeWaitMs", () => {
  it("only waits on the Linux work-area fill path", () => {
    expect(osMaximizeWaitMs(true)).toBe(40);
    expect(osMaximizeWaitMs(false)).toBe(0);
  });
});

describe("scheduleCaptionButtonToggle", () => {
  it("defers past mouse-up so Windows does not drag-to-restore", () => {
    expect(CAPTION_BUTTON_TOGGLE_DEFER_MS).toBeGreaterThan(0);
    vi.useFakeTimers();
    const fn = vi.fn();
    scheduleCaptionButtonToggle(fn, CAPTION_BUTTON_TOGGLE_DEFER_MS);
    expect(fn).not.toHaveBeenCalled();
    vi.advanceTimersByTime(CAPTION_BUTTON_TOGGLE_DEFER_MS - 1);
    expect(fn).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(fn).toHaveBeenCalledTimes(1);
    vi.useRealTimers();
  });
});
