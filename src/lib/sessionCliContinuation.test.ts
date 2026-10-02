import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ChatMessage } from "@/lib/session";
import { sessionMessages } from "@/lib/api";
import { refreshCliContinuationJournal } from "./sessionCliContinuation";

vi.mock("@/lib/api", () => ({ sessionMessages: vi.fn() }));

describe("CLI continuation journal identities", () => {
  beforeEach(() => vi.clearAllMocks());
  it("replaces source identities and stale turns while retaining the optimistic send", async () => {
    vi.mocked(sessionMessages).mockResolvedValue([
      { id: "native-copy-user", role: "user", content: "retained context", createdAt: "2026-10-02T00:00:00Z" },
    ]);
    let messages: ChatMessage[] = [
      { id: "source-user", role: "user", content: "retained context" },
      { id: "removed-source-turn", role: "assistant", content: "stale" },
      { id: "pending-user", role: "user", content: "continue" },
      { id: "pending-assistant", role: "assistant", content: "", streaming: true },
    ];
    const patch = vi.fn((_id: string, reduce: (previous: ChatMessage[]) => ChatMessage[]) => { messages = reduce(messages); });
    expect(await refreshCliContinuationJournal("chat", ["pending-user", "pending-assistant"], () => true, patch)).toBe(true);
    expect(messages.map((message) => message.id)).toEqual(["native-copy-user", "pending-user", "pending-assistant"]);
    expect(messages[2].streaming).toBe(true);
    expect(patch).toHaveBeenCalledWith("chat", expect.any(Function));
  });
  it("does not apply a read after its send was cancelled", async () => {
    let resolve!: (rows: Awaited<ReturnType<typeof sessionMessages>>) => void;
    vi.mocked(sessionMessages).mockReturnValue(new Promise((done) => { resolve = done; }));
    let current = true;
    const patch = vi.fn();
    const pending = refreshCliContinuationJournal("chat", [], () => current, patch);
    current = false;
    resolve([]);
    expect(await pending).toBe(false);
    expect(patch).not.toHaveBeenCalled();
  });
  it("propagates a failed native journal read instead of sending with stale history", async () => {
    vi.mocked(sessionMessages).mockRejectedValue(new Error("unreadable journal"));
    const patch = vi.fn();
    await expect(refreshCliContinuationJournal("chat", [], () => true, patch)).rejects.toThrow("unreadable journal");
    expect(patch).not.toHaveBeenCalled();
  });
});
