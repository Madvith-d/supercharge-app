/** @vitest-environment jsdom */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, renderHook } from "@testing-library/react";
import * as api from "@/lib/api";
import type { Project, SessionRow } from "@/lib/app/sidebarModels";
import { IDLE_SNAPSHOT, type ChatMessage } from "@/lib/session";
import { sessionShellStore } from "@/lib/sessionShellStore";
import { sessionLiveMapStore } from "@/lib/sessionLiveMapStore";
import { sessionTranscriptStore } from "@/lib/sessionTranscriptStore";
import { beginSourceJournalRead } from "@/lib/sessionJournalHydrate";
import { createSessionNavHost, useSessionNavigation } from "./useSessionNavigation";

const project: Project = { id: "project", name: "Source project", path: "/tmp/source-project", trusted: true, pathOk: true };
const source: SessionRow = {
  id: "source", title: "Terminal history", projectId: project.id, updatedAt: "2026-01-01",
  cliSource: { sourceHome: "/tmp/cli", relativeDir: "history/source", agentSessionId: "agent", cwd: project.path, revision: "1", appOwned: false },
};

function setup(row = source) {
  const host = createSessionNavHost();
  for (const group of Object.values(host)) {
    for (const key of Object.keys(group)) (group as Record<string, unknown>)[key] = vi.fn();
  }
  host.catalog.resolveProject = vi.fn((s) => s.projectId === project.id ? project : null);
  host.catalog.findRow = vi.fn((id) => id === row.id ? row : null);
  host.catalog.listLiveIds = () => [row.id];
  host.connect.isSecondaryWindow = () => false;
  host.connect.isProjectWarmable = () => true;
  host.connect.claim = vi.fn(() => true);
  const viewingSessionIdRef = { current: null as string | null };
  const hook = renderHook(() => useSessionNavigation({
    hostRef: { current: host }, focusedSessionId: null, viewingSessionIdRef, bumpViewEpoch: vi.fn(),
  }));
  return { ...hook, host, viewingSessionIdRef };
}

beforeEach(() => {
  vi.useFakeTimers();
  sessionShellStore.setSession(IDLE_SNAPSHOT);
  sessionShellStore.setLiveHost(IDLE_SNAPSHOT);
  sessionLiveMapStore.resetForTests();
  sessionTranscriptStore.resetForTests();
  vi.spyOn(api, "isTauri").mockReturnValue(true);
  vi.spyOn(api, "sessionMessages").mockResolvedValue([{ id: "u", role: "user", content: "history", createdAt: "2026-01-01" }]);
  vi.spyOn(api, "sessionConnect").mockResolvedValue({ ...IDLE_SNAPSHOT, sessionId: "owned", state: "ready" });
});
afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.useRealTimers(); });

describe("source-only navigation", () => {
  const cached: (ChatMessage & { createdAt: string })[] = [
    { id: "u", role: "user", content: "Question", createdAt: "2026-01-01" },
    { id: "a", role: "assistant", content: "A longer cached answer", createdAt: "2026-01-01" },
  ];

  it.each([
    { name: "append", rows: [...cached, { ...cached[0], id: "new", content: "New turn" }] },
    { name: "rewind", rows: cached.slice(0, 1) },
    { name: "shorter body", rows: [cached[0], { ...cached[1], content: "Short" }] },
    { name: "empty", rows: [] },
  ])("uses the authoritative $name projection when reopening", async ({ rows }) => {
    const { result } = setup();
    vi.mocked(api.sessionMessages).mockResolvedValueOnce(cached).mockResolvedValueOnce(rows);
    await act(async () => { await result.current.openSession(source); });
    await act(async () => { await result.current.openSession(source); });
    expect(sessionTranscriptStore.getMessages().map(({ id, content }) => ({ id, content })))
      .toEqual(rows.map(({ id, content }) => ({ id, content })));
    expect(sessionTranscriptStore.getCached(source.id)).toEqual(sessionTranscriptStore.getMessages());
  });

  it.each(["owned", "sending", "connecting", "removed"])("drops source results when %s during the read", async (change) => {
    const { result, host } = setup();
    sessionTranscriptStore.cacheSession(source.id, cached);
    let finish!: (rows: Awaited<ReturnType<typeof api.sessionMessages>>) => void;
    vi.mocked(api.sessionMessages).mockReturnValueOnce(new Promise((resolve) => { finish = resolve; }));
    let pending!: Promise<void>;
    await act(async () => { pending = result.current.openSession(source); });
    if (change === "owned") host.catalog.findRow = () => ({ ...source, cliSource: { ...source.cliSource!, appOwned: true } });
    if (change === "removed") host.catalog.findRow = () => null;
    if (change === "sending") host.connect.isSendInFlight = () => true;
    if (change === "connecting") host.connect.isConnecting = () => true;
    await act(async () => { finish([]); await pending; });
    expect(sessionTranscriptStore.getMessages()).toBe(cached);
    expect(host.hydrate.applyOpenResult).not.toHaveBeenCalled();
    expect(api.sessionConnect).not.toHaveBeenCalled();
  });

  it("does not clear a newer same-session open claim or loading indicator", async () => {
    const { result, host } = setup();
    const finishes: Array<(rows: Awaited<ReturnType<typeof api.sessionMessages>>) => void> = [];
    vi.mocked(api.sessionMessages).mockImplementation(() => new Promise((resolve) => { finishes.push(resolve); }));
    let first!: Promise<void>;
    let second!: Promise<void>;
    await act(async () => { first = result.current.openSession(source); });
    await act(async () => { second = result.current.openSession(source); });
    await act(async () => { finishes[0]([]); await first; });
    expect(result.current.openingSessionIdRef.current).toBe(source.id);
    expect(sessionTranscriptStore.getMetaSnapshot().journalLoading).toBe(true);
    expect(host.hydrate.applyOpenResult).not.toHaveBeenCalled();
    await act(async () => { finishes[1](cached); await second; });
    expect(sessionTranscriptStore.getMessages().map((m) => m.content)).toEqual(cached.map((m) => m.content));
    expect(result.current.openingSessionIdRef.current).toBe(null);
  });

  it("restores navigation context when a newer source refresh supersedes the open read", async () => {
    const { result, host } = setup();
    let finish!: (rows: Awaited<ReturnType<typeof api.sessionMessages>>) => void;
    vi.mocked(api.sessionMessages).mockReturnValueOnce(new Promise((resolve) => { finish = resolve; }));
    let pending!: Promise<void>;
    await act(async () => { pending = result.current.openSession(source); });
    beginSourceJournalRead(source.id);
    await act(async () => { finish(cached); await pending; });
    expect(sessionTranscriptStore.getMessages()).toEqual([]);
    expect(host.hydrate.applyOpenResult).not.toHaveBeenCalled();
    expect(host.catalog.setActiveProject).toHaveBeenCalledWith(project);
    expect(host.gates.restoreForSession).toHaveBeenCalled();
  });

  it("drops an old source response after navigating away and back", async () => {
    const { result } = setup();
    let finish!: (rows: Awaited<ReturnType<typeof api.sessionMessages>>) => void;
    vi.mocked(api.sessionMessages).mockReturnValueOnce(new Promise((resolve) => { finish = resolve; }));
    let first!: Promise<void>;
    await act(async () => { first = result.current.openSession(source); });
    await act(async () => { await result.current.openSession({ ...source, id: "other" }); });
    vi.mocked(api.sessionMessages).mockResolvedValueOnce(cached);
    await act(async () => { await result.current.openSession(source); finish([]); await first; });
    expect(sessionTranscriptStore.getMessages().map((m) => m.content)).toEqual(cached.map((m) => m.content));
    expect(sessionTranscriptStore.getCached(source.id)).toEqual(sessionTranscriptStore.getMessages());
  });

  it.each(["direct", "ref"])("hydrates and restores project without connecting or reconciling via %s", async (route) => {
    const { result, host } = setup({ ...source, archived: true });
    await act(async () => {
      const open = route === "ref" ? result.current.openSessionRef.current : result.current.openSession;
      await open({ ...source, projectId: null, cliSource: undefined });
      await vi.advanceTimersByTimeAsync(5_000);
    });
    expect(sessionTranscriptStore.getMessages()[0]?.content).toBe("history");
    expect(host.catalog.setActiveProject).toHaveBeenCalledWith(project);
    expect(host.composer.restoreForSession).toHaveBeenCalledWith(source.id);
    expect(host.gates.restoreForSession).toHaveBeenCalled();
    expect(api.sessionMessages).toHaveBeenCalledExactlyOnceWith(source.id, { reconcile: false });
    expect(api.sessionConnect).not.toHaveBeenCalled();
    expect(host.connect.claim).not.toHaveBeenCalled();
  });

  it("restores an unbound source to no project rather than keeping the previous project", async () => {
    const row = { ...source, projectId: null };
    const { result, host } = setup(row);
    await act(async () => { await result.current.openSession(row); });
    expect(host.catalog.setActiveProject).toHaveBeenCalledWith(null);
  });

  it("does not warm-connect search hits before their catalog provenance arrives", async () => {
    const { result, host } = setup();
    host.catalog.findRow = () => null;
    await act(async () => {
      await result.current.openSession({ ...source, cliSource: undefined });
      await vi.advanceTimersByTimeAsync(5_000);
    });
    expect(api.sessionConnect).not.toHaveBeenCalled();
    expect(api.sessionMessages).toHaveBeenCalledExactlyOnceWith(source.id, { reconcile: false });
  });

  it("keeps ordinary app-owned history warm connection", async () => {
    const row = { ...source, id: "owned", cliSource: { ...source.cliSource!, appOwned: true } };
    const { result } = setup(row);
    await act(async () => {
      await result.current.openSession(row);
      await vi.advanceTimersByTimeAsync(5_000);
    });
    expect(api.sessionConnect).toHaveBeenCalledExactlyOnceWith({ projectPath: project.path, sessionId: "owned", sshAlias: null });
  });

  it("rechecks provenance before deferred warm connect and reconcile", async () => {
    const row = { ...source, cliSource: null };
    const { result, host } = setup(row);
    await act(async () => { await result.current.openSession(row); });
    host.catalog.findRow = () => source;
    await act(async () => { await vi.advanceTimersByTimeAsync(5_000); });
    expect(api.sessionConnect).not.toHaveBeenCalled();
    expect(api.sessionMessages).toHaveBeenCalledTimes(1);
  });

  it("does not restore stale project context when a source load loses a rapid switch", async () => {
    let finish!: (rows: Awaited<ReturnType<typeof api.sessionMessages>>) => void;
    vi.mocked(api.sessionMessages).mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
    const { result, host } = setup();
    let first!: Promise<void>;
    await act(async () => { first = result.current.openSession(source); });
    const next = { ...source, id: "other", projectId: null };
    await act(async () => {
      await result.current.openSession(next);
      finish([]);
      await first;
    });
    expect(host.catalog.setActiveProject).toHaveBeenCalledTimes(1);
    expect(host.catalog.setActiveProject).toHaveBeenLastCalledWith(null);
  });
});
