import { sessionMessages } from "@/lib/api";
import type { ChatMessage } from "@/lib/session";
import { projectJournalToChat } from "@/lib/sessionJournalHydrate";

export async function refreshCliContinuationJournal(
  sessionId: string,
  optimisticIds: readonly string[],
  isCurrent: () => boolean,
  patch: (id: string, reduce: (messages: ChatMessage[]) => ChatMessage[]) => void,
): Promise<boolean> {
  const stored = await sessionMessages(sessionId);
  if (!isCurrent()) return false;
  const history = projectJournalToChat({ stored, cached: undefined, liveState: "ready", sourceAuthoritative: true });
  // Native copies have new journal identities; preserve only the pending send.
  patch(sessionId, (current) => [
    ...history,
    ...current.filter((message) => optimisticIds.includes(message.id)),
  ]);
  return true;
}
