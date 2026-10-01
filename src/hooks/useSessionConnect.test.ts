/**
 * @vitest-environment jsdom
 */
import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, renderHook } from "@testing-library/react";
import * as api from "@/lib/api";
import { IDLE_SNAPSHOT } from "@/lib/session";
import { mapSessionListRow } from "@/lib/app/sidebarModels";
import { createT, LOCALES } from "@/i18n";
import { sessionDeleteConfirmation } from "@/lib/sessionCliSource";

afterEach(() => { cleanup(); vi.restoreAllMocks(); });
import {
  createSessionConnectHost,
  useSessionConnect,
} from "./useSessionConnect";

describe("useSessionConnect", () => {
  const source = mapSessionListRow({
    id: "source", title: "Terminal", projectId: "project", updatedAt: "2026-01-01",
    cliSource: { sourceHome: "/tmp/cli", relativeDir: "history/source", agentSessionId: "agent", cwd: "/tmp/project", revision: "1", appOwned: false },
  });

  it("rejects implicit/retry/forced connects but permits explicit send with restored project context", async () => {
    vi.spyOn(api, "hasHost").mockReturnValue(true);
    vi.spyOn(api, "sessionStop").mockResolvedValue(undefined as never);
    vi.spyOn(api, "sessionConnect").mockResolvedValue({ ...IDLE_SNAPSHOT, sessionId: source.id, state: "ready" });
    const host = createSessionConnectHost();
    host.session = { ...IDLE_SNAPSHOT, sessionId: source.id };
    host.findRow = () => source;
    host.viewingSessionIdRef.current = source.id;
    host.activeProject = { id: "project", name: "Project", path: "/tmp/project", trusted: true, pathOk: true };
    const { result } = renderHook(() => useSessionConnect({ hostRef: { current: host }, liveMapEnabled: false, viewedSessionId: source.id }));
    await act(async () => {
      expect(await result.current.ensureConnected()).toBeNull();
      expect(await result.current.ensureConnected(true)).toBeNull();
      host.findRow = () => null;
      expect(await result.current.ensureConnected()).toBeNull();
      host.findRow = () => source;
      host.connecting = true;
      result.current.retryAgentConnect();
      host.connecting = false;
    });
    expect(api.sessionConnect).not.toHaveBeenCalled();
    expect(api.sessionStop).not.toHaveBeenCalled();
    await act(async () => {
      expect(await result.current.ensureConnected({ sessionId: source.id, intent: "send" })).toBe(source.id);
    });
    expect(api.sessionConnect).toHaveBeenCalledExactlyOnceWith({ projectPath: "/tmp/project", sessionId: source.id, mode: "agent", sshAlias: null });
    expect(source.cliSource?.appOwned).toBe(false);
  });

  it.each(LOCALES)("has localized external deletion copy for %s", (locale) => {
    const tr = createT(locale);
    const own = { ...source, cliSource: { ...source.cliSource!, appOwned: true } };
    expect(sessionDeleteConfirmation([source], tr)).toBe(tr("session.deleteExternalConfirm", { name: source.title }));
    expect(sessionDeleteConfirmation([own], tr)).toBe(tr("session.deleteConfirm", { name: own.title }));
    expect(sessionDeleteConfirmation([source, own], tr)).toContain(tr("session.deleteExternalNote"));
    expect(sessionDeleteConfirmation([own, own], tr)).not.toContain(tr("session.deleteExternalNote"));
  });

  it("rejects a second connect claim for the same session", () => {
    const hostRef = { current: createSessionConnectHost() };
    const { result } = renderHook(() =>
      useSessionConnect({
        hostRef,
        liveMapEnabled: false,
        viewedSessionId: null,
      }),
    );
    expect(result.current.claimSessionConnection("s1")).toBe(true);
    expect(result.current.claimSessionConnection("s1")).toBe(false);
    result.current.releaseSessionConnection(["s1"]);
    expect(result.current.claimSessionConnection("s1")).toBe(true);
  });
});
