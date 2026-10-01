/** @vitest-environment jsdom */
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { WindowControls, titlebarMaximizeHandlers } from "./WindowControls";
import { CAPTION_BUTTON_TOGGLE_DEFER_MS, resetWindowChromeTestState } from "@/lib/windowChrome";

const host = vi.hoisted(() => ({
  osMaximized: false,
  platform: "win",
  isMaximized: vi.fn(),
  toggleMaximize: vi.fn(),
  maximize: vi.fn(),
  unmaximize: vi.fn(),
  minimize: vi.fn(),
  close: vi.fn(),
  onResized: vi.fn(),
  onMoved: vi.fn(),
  unlistenResize: vi.fn(),
  unlistenMove: vi.fn(),
}));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => host }));
vi.mock("@/lib/appPlatform", () => ({ detectAppPlatform: () => host.platform }));
vi.mock("@/components/ui/tooltip", () => ({ Tip: ({ children }: { children: ReactNode }) => children }));

const labels = { minimize: "Minimize", maximize: "Maximize", restore: "Restore", close: "Close" };
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<T>((done, fail) => { resolve = done; reject = fail; });
  return { promise, resolve, reject };
}
async function mount() {
  const view = render(<WindowControls visible labels={labels} />);
  await act(async () => {});
  return view;
}
async function advance(ms = CAPTION_BUTTON_TOGGLE_DEFER_MS) {
  await act(async () => { await vi.advanceTimersByTimeAsync(ms); });
}
function click(label: string) {
  fireEvent.click(screen.getByRole("button", { name: label }));
}
function flood() {
  act(() => {
    for (let i = 0; i < 100; i += 1) {
      host.onResized.mock.calls[0][0]();
      host.onMoved.mock.calls[0][0]();
    }
  });
}

function delayedNativeQueue() {
  const queue: { name: string; run: () => void }[] = [];
  const ipc = new Map<string, () => void>();
  const observations: { name: string; maximized: boolean }[] = [];
  let toggles = 0;
  let captions = 0;
  let minimized = false;

  function query(name: string) {
    const result = deferred<boolean>();
    queue.push({ name, run: () => {
      observations.push({ name, maximized: host.osMaximized });
      result.resolve(host.osMaximized);
    } });
    return result.promise;
  }

  host.isMaximized.mockImplementation(() => query(`caption:${++captions}:query`));
  host.toggleMaximize.mockImplementation(() => {
    const name = `toggle:${++toggles}`;
    const completion = deferred<void>();
    // Native query, queued mutation, and IPC completion are distinct stages.
    void query(`${name}:query`).then((was) => {
      queue.push({ name: `${name}:apply`, run: () => { host.osMaximized = !was; } });
      ipc.set(name, () => completion.resolve());
    });
    return completion.promise;
  });
  host.minimize.mockImplementation(() => {
    const completion = deferred<void>();
    // This host retains its maximized restore state while minimized.
    queue.push({ name: "minimize:apply", run: () => { minimized = true; } });
    ipc.set("minimize", () => completion.resolve());
    return completion.promise;
  });

  return {
    observations,
    pending: () => queue.map(({ name }) => name),
    state: () => ({ maximized: host.osMaximized, minimized }),
    async step(expected: string) {
      await act(async () => {
        const next = queue.shift();
        expect(next?.name).toBe(expected);
        next!.run();
      });
    },
    async completeIpc(name: string) {
      await act(async () => {
        const complete = ipc.get(name);
        expect(complete).toBeDefined();
        ipc.delete(name);
        complete!();
      });
    },
    resize() {
      act(() => host.onResized.mock.calls[0][0]());
    },
  };
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.resetAllMocks();
  resetWindowChromeTestState();
  host.platform = "win";
  host.osMaximized = false;
  host.isMaximized.mockImplementation(async () => host.osMaximized);
  host.toggleMaximize.mockImplementation(async () => { host.osMaximized = !host.osMaximized; });
  host.maximize.mockResolvedValue(undefined);
  host.unmaximize.mockResolvedValue(undefined);
  host.minimize.mockResolvedValue(undefined);
  host.close.mockResolvedValue(undefined);
  host.onResized.mockResolvedValue(host.unlistenResize);
  host.onMoved.mockResolvedValue(host.unlistenMove);
  vi.spyOn(console, "warn").mockImplementation(() => {});
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe("caption actions", () => {
  it("defers Windows toggles and updates the icon only from observed state", async () => {
    await mount();
    host.isMaximized.mockClear();
    click("Maximize");
    await advance(CAPTION_BUTTON_TOGGLE_DEFER_MS - 1);
    expect(host.toggleMaximize).not.toHaveBeenCalled();
    await advance(1);
    expect(host.toggleMaximize).toHaveBeenCalledTimes(1);
    expect(host.isMaximized).not.toHaveBeenCalled();
    expect(host.maximize).not.toHaveBeenCalled();
    expect(host.unmaximize).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Maximize" })).toBeTruthy();
    await advance();
    expect(screen.getByRole("button", { name: "Restore" })).toBeTruthy();
  });

  it("coalesces rapid unsent toggles by parity", async () => {
    await mount();
    click("Maximize");
    click("Maximize");
    await advance();
    expect(host.toggleMaximize).not.toHaveBeenCalled();
    click("Maximize");
    click("Maximize");
    click("Maximize");
    await advance();
    expect(host.toggleMaximize).toHaveBeenCalledTimes(1);
  });

  it("serializes a second toggle behind an in-flight native command", async () => {
    const first = deferred<void>();
    host.toggleMaximize.mockImplementationOnce(() => first.promise);
    await mount();
    click("Maximize");
    await advance();
    click("Maximize");
    await advance();
    expect(host.toggleMaximize).toHaveBeenCalledTimes(1);
    await act(async () => { host.osMaximized = true; first.resolve(); });
    expect(host.toggleMaximize).toHaveBeenCalledTimes(2);
    await advance();
    expect(screen.getByRole("button", { name: "Maximize" })).toBeTruthy();
  });

  it.each(["Minimize", "Close"])("cancels deferred maximize on %s", async (label) => {
    await mount();
    click("Maximize");
    click(label);
    await advance(100);
    expect(host.toggleMaximize).not.toHaveBeenCalled();
    expect(label === "Minimize" ? host.minimize : host.close).toHaveBeenCalledTimes(1);
  });

  it("prioritizes close over queued minimize and cancels queued toggles", async () => {
    const first = deferred<void>();
    host.toggleMaximize.mockReturnValueOnce(first.promise);
    await mount();
    click("Maximize");
    await advance();
    click("Maximize");
    click("Minimize");
    click("Close");
    click("Close");
    await advance();
    expect(host.close).not.toHaveBeenCalled();
    await act(async () => first.resolve());
    expect(host.toggleMaximize).toHaveBeenCalledTimes(1);
    expect(host.minimize).not.toHaveBeenCalled();
    expect(host.close).toHaveBeenCalledTimes(1);
  });

  it("runs minimize only after an already-sent toggle completes", async () => {
    const first = deferred<void>();
    host.toggleMaximize.mockReturnValueOnce(first.promise);
    await mount();
    click("Maximize");
    await advance();
    click("Minimize");
    expect(host.minimize).not.toHaveBeenCalled();
    await act(async () => first.resolve());
    expect(host.minimize).toHaveBeenCalledTimes(1);
  });

  it.each([false, true])("keeps truthful state when toggle rejects (maximized=%s)", async (state) => {
    host.osMaximized = state;
    const error = new Error("toggle denied");
    host.toggleMaximize.mockRejectedValueOnce(error);
    await mount();
    const label = state ? "Restore" : "Maximize";
    click(label);
    await advance(100);
    expect(screen.getByRole("button", { name: label })).toBeTruthy();
    expect(console.warn).toHaveBeenCalledWith("[WindowControls] toggleMaximize failed", error);
    click(label);
    await advance(100);
    expect(screen.getByRole("button", { name: state ? "Maximize" : "Restore" })).toBeTruthy();
  });

  it.each(["Minimize", "Close"])("logs a rejected %s command and remains usable", async (label) => {
    const command = label === "Minimize" ? host.minimize : host.close;
    const error = new Error("command denied");
    command.mockRejectedValueOnce(error);
    await mount();
    click(label);
    await advance();
    expect(console.warn).toHaveBeenCalledWith(`[WindowControls] ${label.toLowerCase()} failed`, error);
    click("Maximize");
    await advance();
    expect(host.toggleMaximize).toHaveBeenCalledTimes(1);
  });
});

describe("delayed native caption operations", () => {
  it("orders the second toggle query behind the first mutation even when IPC finishes before application", async () => {
    const native = delayedNativeQueue();
    await mount();
    await native.step("caption:1:query");
    click("Maximize");
    await advance();
    click("Maximize");
    await advance();
    expect(host.toggleMaximize).toHaveBeenCalledTimes(1);
    expect(native.pending()).toEqual(["toggle:1:query"]);

    await native.step("toggle:1:query");
    expect(host.toggleMaximize).toHaveBeenCalledTimes(1);
    expect(native.state()).toEqual({ maximized: false, minimized: false });
    await native.completeIpc("toggle:1");
    expect(host.toggleMaximize).toHaveBeenCalledTimes(2);
    expect(native.pending()).toEqual(["toggle:1:apply", "toggle:2:query"]);
    expect(native.state()).toEqual({ maximized: false, minimized: false });
    expect(screen.getByRole("button", { name: "Maximize" })).toBeTruthy();

    await native.step("toggle:1:apply");
    expect(native.state()).toEqual({ maximized: true, minimized: false });
    native.resize();
    await native.step("toggle:2:query");
    expect(native.observations).toContainEqual({ name: "toggle:2:query", maximized: true });
    await native.completeIpc("toggle:2");
    await advance();
    expect(native.pending()).toEqual(["toggle:2:apply", "caption:2:query"]);
    expect(native.state()).toEqual({ maximized: true, minimized: false });

    await native.step("toggle:2:apply");
    native.resize();
    await native.step("caption:2:query");
    await advance();
    await native.step("caption:3:query");
    expect(native.state()).toEqual({ maximized: false, minimized: false });
    expect(native.observations).toEqual([
      { name: "caption:1:query", maximized: false },
      { name: "toggle:1:query", maximized: false },
      { name: "toggle:2:query", maximized: true },
      { name: "caption:2:query", maximized: false },
      { name: "caption:3:query", maximized: false },
    ]);
    expect(screen.getByRole("button", { name: "Maximize" })).toBeTruthy();
    expect(native.pending()).toEqual([]);
  });

  it("applies minimize after a delayed toggle mutation and reconciles the caption after both IPCs finish", async () => {
    const native = delayedNativeQueue();
    await mount();
    await native.step("caption:1:query");
    click("Maximize");
    await advance();
    click("Minimize");
    await advance();
    expect(host.minimize).not.toHaveBeenCalled();
    await native.step("toggle:1:query");
    expect(host.minimize).not.toHaveBeenCalled();

    await native.completeIpc("toggle:1");
    expect(host.minimize).toHaveBeenCalledTimes(1);
    expect(native.pending()).toEqual(["toggle:1:apply", "minimize:apply"]);
    await native.completeIpc("minimize");
    await advance();
    expect(native.state()).toEqual({ maximized: false, minimized: false });
    expect(screen.getByRole("button", { name: "Maximize" })).toBeTruthy();
    expect(native.pending()).toEqual(["toggle:1:apply", "minimize:apply", "caption:2:query"]);

    await native.step("toggle:1:apply");
    expect(native.state()).toEqual({ maximized: true, minimized: false });
    await native.step("minimize:apply");
    expect(native.state()).toEqual({ maximized: true, minimized: true });
    native.resize();
    await native.step("caption:2:query");
    expect(screen.getByRole("button", { name: "Maximize" })).toBeTruthy();
    await advance();
    await native.step("caption:3:query");
    expect(native.state()).toEqual({ maximized: true, minimized: true });
    expect(native.observations).toEqual([
      { name: "caption:1:query", maximized: false },
      { name: "toggle:1:query", maximized: false },
      { name: "caption:2:query", maximized: true },
      { name: "caption:3:query", maximized: true },
    ]);
    expect(screen.getByRole("button", { name: "Restore" })).toBeTruthy();
    expect(native.pending()).toEqual([]);
  });
});

describe("caption synchronization and cleanup", () => {
  it("coalesces move/resize floods into one read plus one trailing read", async () => {
    await mount();
    host.isMaximized.mockClear();
    const read = deferred<boolean>();
    host.isMaximized.mockReturnValueOnce(read.promise);
    flood();
    expect(host.isMaximized).not.toHaveBeenCalled();
    await advance();
    expect(host.isMaximized).toHaveBeenCalledTimes(1);
    flood();
    await advance(100);
    expect(host.isMaximized).toHaveBeenCalledTimes(1);
    await act(async () => read.resolve(true));
    expect(screen.getByRole("button", { name: "Maximize" })).toBeTruthy();
    await advance();
    expect(host.isMaximized).toHaveBeenCalledTimes(2);
    expect(screen.getByRole("button", { name: "Maximize" })).toBeTruthy();
  });

  it("ignores a state read started before a caption command", async () => {
    const read = deferred<boolean>();
    host.isMaximized.mockReturnValueOnce(read.promise);
    await mount();
    click("Maximize");
    await advance();
    await act(async () => read.resolve(false));
    await advance();
    expect(screen.getByRole("button", { name: "Restore" })).toBeTruthy();
  });

  it("retains the icon on a failed state read and retries on the next event", async () => {
    host.osMaximized = true;
    await mount();
    const error = new Error("read denied");
    host.isMaximized.mockRejectedValueOnce(error);
    flood();
    await advance();
    expect(screen.getByRole("button", { name: "Restore" })).toBeTruthy();
    expect(console.warn).toHaveBeenCalledWith("[WindowControls] caption state sync failed", error);
    host.osMaximized = false;
    flood();
    await advance();
    expect(screen.getByRole("button", { name: "Maximize" })).toBeTruthy();
  });

  it("cancels timers and listeners on unmount", async () => {
    const view = await mount();
    click("Maximize");
    flood();
    const reads = host.isMaximized.mock.calls.length;
    view.unmount();
    await advance(100);
    expect(host.toggleMaximize).not.toHaveBeenCalled();
    expect(host.isMaximized).toHaveBeenCalledTimes(reads);
    expect(host.unlistenResize).toHaveBeenCalledTimes(1);
    expect(host.unlistenMove).toHaveBeenCalledTimes(1);
  });

  it("drops queued work and post-command reads after unmount", async () => {
    const first = deferred<void>();
    host.toggleMaximize.mockReturnValueOnce(first.promise);
    const view = await mount();
    click("Maximize");
    await advance();
    click("Minimize");
    view.unmount();
    await act(async () => first.resolve());
    await advance(100);
    expect(host.minimize).not.toHaveBeenCalled();
    expect(host.isMaximized).toHaveBeenCalledTimes(1);
  });

  it("cleans up late listener registrations independently", async () => {
    const resize = deferred<() => void>();
    const move = deferred<() => void>();
    host.onResized.mockReturnValueOnce(resize.promise);
    host.onMoved.mockReturnValueOnce(move.promise);
    const view = await mount();
    view.unmount();
    await act(async () => resize.resolve(host.unlistenResize));
    expect(host.unlistenResize).toHaveBeenCalledTimes(1);
    await act(async () => move.resolve(host.unlistenMove));
    expect(host.unlistenMove).toHaveBeenCalledTimes(1);
  });

  it("ignores stale reads across visibility lifetimes and cancels hidden actions", async () => {
    const oldRead = deferred<boolean>();
    host.isMaximized.mockReturnValueOnce(oldRead.promise);
    const view = await mount();
    click("Maximize");
    view.rerender(<WindowControls visible={false} labels={labels} />);
    view.rerender(<WindowControls visible labels={labels} />);
    await act(async () => {});
    await act(async () => oldRead.resolve(true));
    await advance(100);
    expect(screen.getByRole("button", { name: "Maximize" })).toBeTruthy();
    expect(host.toggleMaximize).not.toHaveBeenCalled();
  });

  it("keeps Windows titlebar toggles native and macOS double-click handling enabled", async () => {
    const event = { target: document.createElement("div"), button: 0, detail: 2, preventDefault: vi.fn() };
    titlebarMaximizeHandlers().onMouseDown(event);
    titlebarMaximizeHandlers().onDoubleClick(event);
    await act(async () => {});
    expect(host.toggleMaximize).not.toHaveBeenCalled();
    expect(event.preventDefault).not.toHaveBeenCalled();
    host.platform = "mac";
    titlebarMaximizeHandlers().onMouseDown(event);
    titlebarMaximizeHandlers().onDoubleClick(event);
    await act(async () => {});
    expect(event.preventDefault).toHaveBeenCalledTimes(1);
    expect(host.maximize).toHaveBeenCalledTimes(1);
    expect(host.toggleMaximize).not.toHaveBeenCalled();
  });
});
