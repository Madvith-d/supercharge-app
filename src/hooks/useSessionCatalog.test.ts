/**
 * @vitest-environment jsdom
 *
 * Catalog list + multi-select live here. Open/new-chat live in useSessionNavigation.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, renderHook } from "@testing-library/react";
import * as api from "@/lib/api";
import {
  sessionSidebarSelectOrder,
  useSessionCatalog,
} from "./useSessionCatalog";
import type { SessionRow } from "@/lib/app/sidebarModels";

function row(
  partial: Partial<SessionRow> & { id: string },
): SessionRow {
  return {
    id: partial.id,
    title: partial.title ?? partial.id,
    projectId: partial.projectId ?? null,
    updatedAt: partial.updatedAt ?? "2026-01-02T00:00:00Z",
    archived: partial.archived,
    pinned: partial.pinned,
  };
}

function setup(projects: { id: string }[] = [{ id: "p1" }]) {
  return renderHook(() =>
    useSessionCatalog({
      projects,
      isDialogOpen: () => false,
    }),
  );
}

describe("sessionSidebarSelectOrder", () => {
  it("lists project sessions then orphans, pinned first", () => {
    const sessions = [
      row({ id: "old", projectId: "p1", updatedAt: "2026-01-01T00:00:00Z" }),
      row({
        id: "pin",
        projectId: "p1",
        pinned: true,
        updatedAt: "2026-01-01T00:00:00Z",
      }),
      row({ id: "orphan", projectId: null }),
    ];
    expect(sessionSidebarSelectOrder(sessions, [{ id: "p1" }])).toEqual([
      "pin",
      "old",
      "orphan",
    ]);
  });

  it("puts pinned chats from any folder at the global top", () => {
    const sessions = [
      row({ id: "p1-old", projectId: "p1", updatedAt: "2026-01-03T00:00:00Z" }),
      row({
        id: "p2-pin",
        projectId: "p2",
        pinned: true,
        updatedAt: "2026-01-01T00:00:00Z",
      }),
      row({ id: "orphan", projectId: null }),
    ];
    expect(
      sessionSidebarSelectOrder(sessions, [{ id: "p1" }, { id: "p2" }]),
    ).toEqual(["p2-pin", "p1-old", "orphan"]);
  });
});

describe("useSessionCatalog", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    vi.spyOn(api, "cliHistorySync").mockResolvedValue(false);
  });
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  it("enter seeds selection; exit clears it", () => {
    const { result } = setup();
    act(() => {
      result.current.enterSessionSelectMode("a");
    });
    expect(result.current.sessionSelectMode).toBe(true);
    expect([...result.current.selectedSessionIds]).toEqual(["a"]);
    act(() => {
      result.current.exitSessionSelectMode();
    });
    expect(result.current.sessionSelectMode).toBe(false);
    expect(result.current.selectedSessionIds.size).toBe(0);
  });

  it("Cmd-style toggle enters select mode and keeps prior ids", () => {
    const { result } = setup();
    act(() => {
      result.current.toggleSessionSelected("a");
    });
    expect(result.current.sessionSelectMode).toBe(true);
    expect([...result.current.selectedSessionIds]).toEqual(["a"]);
    act(() => {
      result.current.toggleSessionSelected("b");
    });
    expect([...result.current.selectedSessionIds].sort()).toEqual(["a", "b"]);
  });

  it("Shift-click selects a contiguous range in sidebar order", () => {
    const { result } = setup();
    act(() => {
      result.current.setSessions([
        row({ id: "a", projectId: "p1", updatedAt: "2026-01-03T00:00:00Z" }),
        row({ id: "b", projectId: "p1", updatedAt: "2026-01-02T00:00:00Z" }),
        row({ id: "c", projectId: "p1", updatedAt: "2026-01-01T00:00:00Z" }),
      ]);
    });
    act(() => {
      result.current.enterSessionSelectMode("a");
    });
    act(() => {
      result.current.toggleSessionSelected("c", { shiftKey: true });
    });
    expect([...result.current.selectedSessionIds].sort()).toEqual([
      "a",
      "b",
      "c",
    ]);
  });

  it("refreshSessions replaces the catalog from the host list", async () => {
    vi.spyOn(api, "sessionsList").mockResolvedValue([
      {
        id: "n1",
        title: "New",
        projectId: null,
        updatedAt: "2026-01-01T00:00:00Z",
        modelId: null,
      },
    ]);
    vi.spyOn(api, "trayRefresh").mockResolvedValue(undefined as never);
    const { result } = setup();
    await act(async () => {
      await result.current.refreshSessions();
    });
    expect(result.current.sessions.map((s) => s.id)).toEqual(["n1"]);
    expect(api.trayRefresh).toHaveBeenCalled();
  });

  it("reloads the catalog when sessions://changed fires", async () => {
    vi.useFakeTimers();
    vi.spyOn(api, "hasHost").mockReturnValue(true);
    let onChanged: ((payload: { reason?: string; sessionId?: string }) => void) | undefined;
    vi.spyOn(api, "listen").mockImplementation(async (event, handler) => {
      if (event === "sessions://changed") {
        onChanged = handler as typeof onChanged;
      }
      return () => {};
    });
    vi.spyOn(api, "sessionsList").mockResolvedValue([
      {
        id: "fresh",
        title: "Fresh",
        projectId: null,
        updatedAt: "2026-01-03T00:00:00Z",
        modelId: null,
      },
    ]);
    vi.spyOn(api, "trayRefresh").mockResolvedValue(undefined as never);
    const { result } = setup();
    await act(async () => {
      await Promise.resolve();
    });
    expect(onChanged).toBeTypeOf("function");
    await act(async () => {
      onChanged?.({ reason: "turn", sessionId: "fresh" });
      vi.advanceTimersByTime(150);
      await Promise.resolve();
    });
    expect(result.current.sessions.map((s) => s.id)).toEqual(["fresh"]);
  });

  it("discovers terminal history at startup without any manual import", async () => {
    vi.useFakeTimers();
    vi.spyOn(api, "hasHost").mockReturnValue(true);
    vi.spyOn(api, "isDesktopHost").mockReturnValue(true);
    vi.spyOn(document, "visibilityState", "get").mockReturnValue("visible");
    let onChanged: ((payload: { reason?: string }) => void) | undefined;
    vi.spyOn(api, "listen").mockImplementation(async (_event, handler) => {
      onChanged = handler;
      return () => {};
    });
    const listRow = { id: "cli-row", title: "Terminal chat", projectId: null,
      updatedAt: "2026-01-03T00:00:00Z", modelId: null };
    vi.spyOn(api, "sessionsList").mockResolvedValue([listRow]);
    vi.spyOn(api, "trayRefresh").mockResolvedValue(undefined as never);
    vi.spyOn(api, "cliSessionImport");
    vi.spyOn(api, "cliSessionsImportAll");
    vi.mocked(api.cliHistorySync).mockImplementationOnce(async () => {
      expect(onChanged).toBeTypeOf("function");
      onChanged?.({ reason: "cli_history_sync" });
      return true;
    });
    const { result } = setup();
    await act(async () => { await Promise.resolve(); });
    await act(async () => { await vi.advanceTimersByTimeAsync(150); });
    expect(result.current.sessions.map((s) => s.id)).toEqual(["cli-row"]);
    expect(api.cliSessionImport).not.toHaveBeenCalled();
    expect(api.cliSessionsImportAll).not.toHaveBeenCalled();
    expect(api.cliHistorySync).toHaveBeenCalledTimes(1);
    await act(async () => { await vi.advanceTimersByTimeAsync(30_000); });
    expect(api.cliHistorySync).toHaveBeenCalledTimes(4);
    expect(api.sessionsList).toHaveBeenCalledTimes(1);
    expect(api.trayRefresh).toHaveBeenCalledTimes(1);
  });

  it("rejects older catalog responses and late responses after cleanup", async () => {
    const pending: Array<(rows: Awaited<ReturnType<typeof api.sessionsList>>) => void> = [];
    vi.spyOn(api, "sessionsList").mockImplementation(() => new Promise((resolve) => pending.push(resolve)));
    vi.spyOn(api, "trayRefresh").mockResolvedValue(undefined as never);
    const { result, unmount } = setup();
    let older!: Promise<void>;
    let newer!: Promise<void>;
    act(() => {
      older = result.current.refreshSessions();
      newer = result.current.refreshSessions();
    });
    await act(async () => {
      pending[1]([{ id: "new", title: "New", projectId: null, modelId: null, updatedAt: "2026-01-03" }]);
      await newer;
      pending[0]([]);
      await older;
    });
    expect(result.current.sessions.map((s) => s.id)).toEqual(["new"]);
    expect(api.trayRefresh).toHaveBeenCalledTimes(1);
    const late = result.current.refreshSessions();
    unmount();
    await act(async () => { pending[2]([]); await late; });
    expect(api.trayRefresh).toHaveBeenCalledTimes(1);
  });

  it("coalesces events and disposes late listener registration", async () => {
    vi.useFakeTimers();
    vi.spyOn(api, "hasHost").mockReturnValue(true);
    let onChanged!: (payload: unknown) => void;
    let registered!: (unlisten: () => void) => void;
    const unlisten = vi.fn();
    vi.spyOn(api, "listen").mockImplementation((_event, handler) => {
      onChanged = handler;
      return new Promise((resolve) => { registered = resolve; });
    });
    vi.spyOn(api, "sessionsList").mockResolvedValue([]);
    vi.spyOn(api, "trayRefresh").mockResolvedValue(undefined as never);
    const { unmount } = setup();
    await act(async () => {
      onChanged({});
      onChanged({});
      await vi.advanceTimersByTimeAsync(150);
    });
    expect(api.sessionsList).toHaveBeenCalledTimes(1);
    onChanged({});
    unmount();
    await act(async () => {
      registered(unlisten);
      onChanged({});
      await vi.advanceTimersByTimeAsync(1000);
    });
    expect(unlisten).toHaveBeenCalledTimes(1);
    expect(api.sessionsList).toHaveBeenCalledTimes(1);
  });

  it("retries failed listener registration before starting inventory", async () => {
    vi.useFakeTimers();
    vi.spyOn(api, "hasHost").mockReturnValue(true);
    vi.spyOn(api, "isDesktopHost").mockReturnValue(true);
    vi.spyOn(document, "visibilityState", "get").mockReturnValue("visible");
    vi.spyOn(api, "listen").mockRejectedValueOnce(new Error("transport"))
      .mockResolvedValue(() => {});
    const { unmount } = setup();
    await act(async () => { await Promise.resolve(); });
    expect(api.cliHistorySync).not.toHaveBeenCalled();
    await act(async () => { await vi.advanceTimersByTimeAsync(1000); });
    expect(api.listen).toHaveBeenCalledTimes(2);
    expect(api.cliHistorySync).toHaveBeenCalledTimes(1);
    unmount();
    expect(vi.getTimerCount()).toBe(0);
  });

  it("drops selection for archived sessions", () => {
    const { result } = setup();
    act(() => {
      result.current.setSessions([
        row({ id: "keep" }),
        row({ id: "gone" }),
      ]);
    });
    act(() => {
      result.current.enterSessionSelectMode("gone");
    });
    act(() => {
      result.current.setSessions([
        row({ id: "keep" }),
        row({ id: "gone", archived: true }),
      ]);
    });
    expect([...result.current.selectedSessionIds]).toEqual([]);
    expect(result.current.selectableSessionCount).toBe(1);
  });
});
