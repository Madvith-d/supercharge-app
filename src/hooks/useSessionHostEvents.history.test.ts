/** @vitest-environment jsdom */
import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as api from "@/lib/api";
import { IDLE_SNAPSHOT, type ChatMessage } from "@/lib/session";
import { emptyLiveSnapshot } from "@/lib/sessionLiveStore";
import { beginSourceJournalRead } from "@/lib/sessionJournalHydrate";
import { useSessionHostEvents, type SessionHostEventsCtx } from "./useSessionHostEvents";

type Changed = { reason?: string; sessionId?: string };
type Rows = Awaited<ReturnType<typeof api.sessionMessages>>;
const source = {
  id: "viewed", title: "Terminal history", projectId: null, updatedAt: "2026-01-01",
  cliSource: { sourceHome: "/tmp/cli", relativeDir: "history/source", agentSessionId: "agent", cwd: null, revision: "1", appOwned: false },
};
const original: (ChatMessage & { createdAt: string })[] = [
  { id: "u", role: "user", content: "Question", createdAt: "2026-01-01" },
  { id: "a", role: "assistant", content: "A long terminal answer", createdAt: "2026-01-01" },
];
const replacements = [
  { name: "append", rows: [...original, { ...original[0], id: "new", content: "New terminal turn" }] },
  { name: "rewind", rows: original.slice(0, 1) },
  { name: "shorter body", rows: [original[0], { ...original[1], content: "Short" }] },
  { name: "empty", rows: [] },
];

async function setup() {
  let changed!: (payload: Changed) => void;
  let reconciled!: (payload: { sessionId: string }) => void;
  const unlisten = vi.fn();
  vi.spyOn(api, "listen").mockImplementation(async (event, handler) => {
    if (event === "sessions://changed") changed = handler;
    if (event === "session://journal_reconciled") reconciled = handler;
    return unlisten;
  });
  const ctx = {
    viewingSessionIdRef: { current: null as string | null },
    openingSessionIdRef: { current: null },
    sessionsRef: { current: [structuredClone(source)] },
    liveMapRef: { current: {} } as SessionHostEventsCtx["liveMapRef"],
    liveHostRef: { current: { ...IDLE_SNAPSHOT } },
    isSecondaryWindowRef: { current: false },
    secondaryFocusSessionIdRef: { current: null },
    messagesBySessionRef: { current: new Map<string, ChatMessage[]>() },
    patchSessionMessages: vi.fn(),
    tryApplyAutomationFromSession: vi.fn().mockResolvedValue(undefined),
    setLiveHost: vi.fn(),
    setLocalError: vi.fn(),
  } satisfies Partial<SessionHostEventsCtx>;
  const hook = renderHook(() => useSessionHostEvents(ctx as unknown as SessionHostEventsCtx));
  await act(async () => { await Promise.resolve(); });
  expect(ctx.setLocalError).not.toHaveBeenCalled();
  ctx.viewingSessionIdRef.current = "viewed";
  return { ...hook, ctx, changed, reconciled, unlisten };
}

describe("CLI history transcript refresh", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.spyOn(api, "isTauri").mockReturnValue(true);
    vi.spyOn(api, "sessionGetState").mockResolvedValue({ ...IDLE_SNAPSHOT });
    vi.spyOn(api, "sessionMessages").mockResolvedValue([
      { id: "message", role: "assistant", content: "Terminal answer", createdAt: "2026-01-03T00:00:00Z" },
    ]);
    vi.spyOn(api, "cliHistorySync");
  });
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  it("rehydrates the viewed journal once for a burst of inventory changes without another sync", async () => {
    const { ctx, changed } = await setup();
    await act(async () => {
      changed({ reason: "cli_history_sync" });
      changed({ reason: "cli_history_sync" });
      await vi.advanceTimersByTimeAsync(150);
    });
    expect(api.sessionMessages).toHaveBeenCalledExactlyOnceWith("viewed", { reconcile: false });
    expect(ctx.messagesBySessionRef.current.get("viewed")?.[0].content).toBe("Terminal answer");
    expect(ctx.patchSessionMessages).toHaveBeenCalledTimes(1);
    await act(async () => { await vi.advanceTimersByTimeAsync(5000); });
    expect(api.sessionMessages).toHaveBeenCalledTimes(1);
    expect(api.cliHistorySync).not.toHaveBeenCalled();
  });

  it.each(replacements)("replaces cached history on $name refresh", async ({ rows }) => {
    const { ctx, changed } = await setup();
    ctx.messagesBySessionRef.current.set("viewed", original);
    vi.mocked(api.sessionMessages).mockResolvedValueOnce(rows);
    await act(async () => {
      changed({ reason: "cli_history_sync" });
      await vi.advanceTimersByTimeAsync(150);
    });
    const next = ctx.messagesBySessionRef.current.get("viewed");
    expect(next?.map(({ id, content }) => ({ id, content }))).toEqual(rows.map(({ id, content }) => ({ id, content })));
    expect(ctx.patchSessionMessages.mock.calls[0][1](original)).toEqual(next);
  });

  it.each(["owned", "removed", "host busy", "background busy"])("drops a pending source read when %s", async (change) => {
    const { ctx, changed } = await setup();
    ctx.messagesBySessionRef.current.set("viewed", original);
    let finish!: (rows: Rows) => void;
    vi.mocked(api.sessionMessages).mockReturnValueOnce(new Promise((resolve) => { finish = resolve; }));
    await act(async () => {
      changed({ reason: "cli_history_sync" });
      await vi.advanceTimersByTimeAsync(150);
    });
    if (change === "owned") ctx.sessionsRef.current[0].cliSource.appOwned = true;
    if (change === "removed") ctx.sessionsRef.current = [];
    if (change === "host busy") ctx.liveHostRef.current = { ...IDLE_SNAPSHOT, sessionId: "viewed", state: "streaming" };
    if (change === "background busy") ctx.liveMapRef.current = { viewed: { ...emptyLiveSnapshot("viewed"), state: "awaiting_permission" } };
    await act(async () => finish([]));
    expect(ctx.patchSessionMessages).not.toHaveBeenCalled();
    expect(ctx.messagesBySessionRef.current.get("viewed")).toBe(original);
  });

  it("does not overwrite a newer open or optimistic continuation with a pending refresh", async () => {
    const { ctx, changed } = await setup();
    let finish!: (rows: Rows) => void;
    vi.mocked(api.sessionMessages).mockReturnValueOnce(new Promise((resolve) => { finish = resolve; }));
    await act(async () => {
      changed({ reason: "cli_history_sync" });
      await vi.advanceTimersByTimeAsync(150);
    });
    ctx.messagesBySessionRef.current.set("viewed", original);
    await act(async () => finish([]));
    expect(ctx.patchSessionMessages).not.toHaveBeenCalled();
    expect(ctx.messagesBySessionRef.current.get("viewed")).toBe(original);
  });

  it("drops a refresh after navigating away and reopening while the new read is pending", async () => {
    const { ctx, changed } = await setup();
    ctx.messagesBySessionRef.current.set("viewed", original);
    let finish!: (rows: Rows) => void;
    vi.mocked(api.sessionMessages).mockReturnValueOnce(new Promise((resolve) => { finish = resolve; }));
    await act(async () => {
      changed({ reason: "cli_history_sync" });
      await vi.advanceTimersByTimeAsync(150);
    });
    ctx.viewingSessionIdRef.current = "other";
    ctx.viewingSessionIdRef.current = "viewed";
    beginSourceJournalRead("viewed");
    await act(async () => finish([]));
    expect(ctx.patchSessionMessages).not.toHaveBeenCalled();
    expect(ctx.messagesBySessionRef.current.get("viewed")).toBe(original);
  });

  it.each(["owned", "removed", "busy"])("does not read %s history on inventory refresh", async (change) => {
    const { ctx, changed } = await setup();
    if (change === "owned") ctx.sessionsRef.current[0].cliSource.appOwned = true;
    if (change === "removed") ctx.sessionsRef.current = [];
    if (change === "busy") ctx.liveHostRef.current = { ...IDLE_SNAPSHOT, sessionId: "viewed", state: "streaming" };
    await act(async () => {
      changed({ reason: "cli_history_sync" });
      await vi.advanceTimersByTimeAsync(150);
    });
    expect(api.sessionMessages).not.toHaveBeenCalled();
  });

  it("drops an in-flight refresh after unmount", async () => {
    const { ctx, changed, unmount } = await setup();
    let finish!: (rows: Rows) => void;
    vi.mocked(api.sessionMessages).mockReturnValueOnce(new Promise((resolve) => { finish = resolve; }));
    await act(async () => {
      changed({ reason: "cli_history_sync" });
      await vi.advanceTimersByTimeAsync(150);
    });
    unmount();
    await act(async () => finish(original));
    expect(ctx.patchSessionMessages).not.toHaveBeenCalled();
  });

  it("invalidates an older response as soon as a newer inventory event arrives", async () => {
    const { ctx, changed } = await setup();
    let finish!: (rows: Rows) => void;
    vi.mocked(api.sessionMessages).mockReturnValueOnce(new Promise((resolve) => { finish = resolve; }));
    await act(async () => {
      changed({ reason: "cli_history_sync" });
      await vi.advanceTimersByTimeAsync(150);
      changed({ reason: "cli_history_sync" });
      finish(original);
    });
    expect(ctx.patchSessionMessages).not.toHaveBeenCalled();
    await act(async () => { await vi.advanceTimersByTimeAsync(150); });
    expect(ctx.messagesBySessionRef.current.get("viewed")?.[0].content).toBe("Terminal answer");
  });

  it("does not overwrite the latest refresh with an out-of-order response", async () => {
    const { ctx, changed } = await setup();
    let finish!: (rows: Rows) => void;
    vi.mocked(api.sessionMessages).mockReturnValueOnce(new Promise((resolve) => { finish = resolve; }));
    await act(async () => {
      changed({ reason: "cli_history_sync" });
      await vi.advanceTimersByTimeAsync(150);
      changed({ reason: "cli_history_sync" });
      await vi.advanceTimersByTimeAsync(150);
      finish(original);
    });
    expect(ctx.patchSessionMessages).toHaveBeenCalledTimes(1);
    expect(ctx.messagesBySessionRef.current.get("viewed")?.[0].content).toBe("Terminal answer");
  });

  it("does not invalidate an App-owned journal read for unrelated CLI inventory changes", async () => {
    const { ctx, changed, reconciled } = await setup();
    ctx.sessionsRef.current[0].cliSource.appOwned = true;
    let finish!: (rows: Rows) => void;
    vi.mocked(api.sessionMessages).mockReturnValueOnce(new Promise((resolve) => { finish = resolve; }));
    await act(async () => {
      reconciled({ sessionId: "viewed" });
      changed({ reason: "cli_history_sync" });
      finish(original);
    });
    expect(ctx.patchSessionMessages).toHaveBeenCalledTimes(1);
  });

  it("drops pending source media resolution after ownership changes", async () => {
    const { ctx, changed } = await setup();
    let finish!: (rows: Awaited<ReturnType<typeof api.sessionResolveRelativeMedia>>) => void;
    vi.spyOn(api, "sessionResolveRelativeMedia").mockReturnValueOnce(new Promise((resolve) => { finish = resolve; }));
    vi.mocked(api.sessionMessages).mockResolvedValueOnce([
      { id: "a", role: "assistant", content: "![answer](images/answer.png)", createdAt: "2026-01-01" },
    ]);
    await act(async () => {
      changed({ reason: "cli_history_sync" });
      await vi.advanceTimersByTimeAsync(150);
    });
    expect(api.sessionResolveRelativeMedia).toHaveBeenCalledTimes(1);
    const painted = ctx.messagesBySessionRef.current.get("viewed");
    ctx.sessionsRef.current[0].cliSource.appOwned = true;
    await act(async () => finish([{ path: "/tmp/answer.png", name: "answer.png", isDir: false }]));
    expect(ctx.patchSessionMessages).toHaveBeenCalledTimes(1);
    expect(ctx.messagesBySessionRef.current.get("viewed")).toBe(painted);
  });

  it("preserves the last projection when a refresh fails", async () => {
    const { ctx, changed } = await setup();
    ctx.messagesBySessionRef.current.set("viewed", original);
    vi.mocked(api.sessionMessages).mockRejectedValueOnce(new Error("read failed"));
    await act(async () => {
      changed({ reason: "cli_history_sync" });
      await vi.advanceTimersByTimeAsync(150);
    });
    expect(ctx.messagesBySessionRef.current.get("viewed")).toBe(original);
    expect(ctx.patchSessionMessages).not.toHaveBeenCalled();
  });

  it("ignores unrelated changes and cancels navigation before the refresh", async () => {
    const { ctx, changed } = await setup();
    await act(async () => {
      changed({ reason: "title" });
      changed({ reason: "cli_history_sync", sessionId: "other" });
      await vi.advanceTimersByTimeAsync(150);
    });
    expect(api.sessionMessages).not.toHaveBeenCalled();
    changed({ reason: "cli_history_sync" });
    ctx.viewingSessionIdRef.current = "other";
    await act(async () => { await vi.advanceTimersByTimeAsync(150); });
    expect(api.sessionMessages).not.toHaveBeenCalled();
  });

  it("does not apply a history response after switching chats", async () => {
    const { ctx, changed } = await setup();
    let resolve!: (rows: Awaited<ReturnType<typeof api.sessionMessages>>) => void;
    vi.mocked(api.sessionMessages).mockReturnValueOnce(new Promise((done) => { resolve = done; }));
    await act(async () => {
      changed({ reason: "cli_history_sync" });
      await vi.advanceTimersByTimeAsync(150);
    });
    ctx.viewingSessionIdRef.current = "other";
    await act(async () => resolve([]));
    expect(ctx.patchSessionMessages).not.toHaveBeenCalled();
  });

  it("cleans up pending refreshes and ignores late events", async () => {
    const { unmount, changed, unlisten } = await setup();
    changed({ reason: "cli_history_sync" });
    unmount();
    await act(async () => {
      changed({ reason: "cli_history_sync" });
      await vi.advanceTimersByTimeAsync(5000);
    });
    expect(unlisten).toHaveBeenCalled();
    expect(api.sessionMessages).not.toHaveBeenCalled();
  });
});
