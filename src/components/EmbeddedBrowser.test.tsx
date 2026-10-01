/** @vitest-environment jsdom */
import { StrictMode } from "react";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import "@testing-library/jest-dom/vitest";
import "@/test/jsdomStubs";
import { mockIPC, mockWindows, clearMocks } from "@tauri-apps/api/mocks";
import { emit } from "@tauri-apps/api/event";
import { acquireNativeWebviewCover, resetNativeWebviewCoverForTests } from "@/lib/nativeWebviewCover";
import { EmbeddedBrowser } from "./EmbeddedBrowser";

const mocks = vi.hoisted(() => ({
  create: vi.fn(), close: vi.fn(), bounds: vi.fn(), navigate: vi.fn(), reload: vi.fn(),
  show: vi.fn(), hide: vi.fn(), eval: vi.fn(), installDownloadHook: vi.fn(),
  handlers: new Map<string, (event: { payload: unknown }) => void>(),
}));
vi.mock("@/lib/api", () => ({
  isTauri: () => true,
  sideBrowserCreate: mocks.create,
  sideBrowserClose: mocks.close,
  sideBrowserSetBounds: mocks.bounds,
  sideBrowserNavigate: mocks.navigate,
  sideBrowserReload: mocks.reload,
  sideBrowserEval: mocks.eval,
  sideBrowserInstallDownloadHook: mocks.installDownloadHook,
}));

beforeEach(() => {
  vi.clearAllMocks();
  mocks.handlers.clear();
  mockWindows("main");
  mockIPC((command) => {
    if (command === "plugin:webview|get_all_webviews") {
      return ["bounds", "strict", "failure", "nav"].map((id) => ({ label: `resource-browser-${id}`, windowLabel: "main" }));
    }
    if (command === "plugin:webview|webview_show") return mocks.show();
    if (command === "plugin:webview|webview_hide") return mocks.hide();
    return undefined;
  }, { shouldMockEvents: true });
  for (const fn of [mocks.create, mocks.close, mocks.bounds, mocks.navigate, mocks.reload, mocks.show, mocks.hide, mocks.installDownloadHook]) fn.mockResolvedValue(undefined);
  mocks.eval.mockResolvedValue('"complete"');
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue({ x: 850, y: 90, left: 850, top: 90, right: 1250, bottom: 690, width: 400, height: 600, toJSON: () => ({}) });
  resetNativeWebviewCoverForTests();
});
afterEach(async () => {
  cleanup();
  await new Promise((resolve) => setTimeout(resolve, 170));
  resetNativeWebviewCoverForTests();
  vi.restoreAllMocks();
  clearMocks();
});

describe("EmbeddedBrowser native lifecycle", () => {
  it("uses the host bounds API so Linux does not stack a second full-width webview", async () => {
    render(<EmbeddedBrowser url="https://example.com" instanceId="bounds" />);
    await waitFor(() => expect(mocks.bounds).toHaveBeenCalledWith("resource-browser-bounds", { x: 850, y: 90, width: 400, height: 600 }));
    await waitFor(() => expect(mocks.show).toHaveBeenCalled());
  });

  it("restores the same native view after a modal in StrictMode", async () => {
    render(<StrictMode><EmbeddedBrowser url="https://example.com" instanceId="strict" /></StrictMode>);
    await waitFor(() => expect(mocks.show).toHaveBeenCalled());
    let release = () => {};
    act(() => { release = acquireNativeWebviewCover(); });
    await waitFor(() => expect(mocks.hide).toHaveBeenCalled());
    const before = mocks.show.mock.calls.length;
    act(() => release());
    await waitFor(() => expect(mocks.show.mock.calls.length).toBeGreaterThan(before));
    expect(mocks.close).not.toHaveBeenCalled();
  });

  it("shows load failures instead of leaving a blank native surface and allows retry", async () => {
    render(<EmbeddedBrowser url="https://example.com" instanceId="failure" />);
    await waitFor(() => expect(mocks.show).toHaveBeenCalled());
    await act(async () => emit("side-browser://page-load", { label: "resource-browser-failure", phase: "failed", url: "https://example.com", error: "Connection refused" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Connection refused");
    await waitFor(() => expect(mocks.hide).toHaveBeenCalled());
    fireEvent.click(screen.getByRole("alert").querySelector("button")!);
    await waitFor(() => expect(mocks.navigate).toHaveBeenCalledWith("resource-browser-failure", "https://example.com"));
  });

  it("reports native navigation without navigating back to the previous URL", async () => {
    const onNavigation = vi.fn();
    const view = render(<EmbeddedBrowser url="https://example.com" instanceId="nav" onNavigation={onNavigation} />);
    await waitFor(() => expect(mocks.eval).toHaveBeenCalled());
    await act(async () => emit("side-browser://page-load", { label: "resource-browser-nav", phase: "finished", url: "https://example.com/next" }));
    expect(onNavigation).toHaveBeenCalledWith("https://example.com/next");
    view.rerender(<EmbeddedBrowser url="https://example.com/next" instanceId="nav" onNavigation={onNavigation} />);
    expect(mocks.navigate).not.toHaveBeenCalled();
  });
});
