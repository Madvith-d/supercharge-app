import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import {
  maximizeLooksNoop,
  osMaximizeWaitMs,
  shouldAcceptTitlebarMaximize,
  shouldFakeMaximizeFallback,
  tauriDragRegion,
  TITLEBAR_MAXIMIZE_DEBOUNCE_MS,
} from "./windowChrome";

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

describe("Windows caption controls", () => {
  it("dispatches from click without a timer and avoids maximize checks on move", () => {
    const controls = readFileSync(
      join(__dirname, "../components/WindowControls.tsx"),
      "utf8",
    );
    const nativeHost = readFileSync(
      join(__dirname, "../../src-tauri/src/win_shell.rs"),
      "utf8",
    );
    expect(controls).toContain('void winChrome("toggleMaximize")');
    expect(controls).toContain("minimizeWindowReliable()");
    expect(controls).toContain(".onResized(");
    expect(controls).not.toContain(".onMoved(");
    expect(controls).not.toContain("setMaximized((value) => !value)");
    expect(controls).not.toContain("scheduleCaptionButtonToggle");
    expect(nativeHost).toContain("ShowWindowAsync(hwnd, command)");
    expect(nativeHost).toContain("static MAIN_HWND: AtomicIsize");
  });
});
