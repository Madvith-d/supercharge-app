import type { createT } from "@/i18n";
import type { SessionRow } from "./app/sidebarModels";

/** Camel-case provenance returned by sessions_list. */
export type CliSessionSource = {
  sourceHome: string;
  relativeDir: string;
  agentSessionId: string;
  cwd: string | null;
  title?: string | null;
  updatedAt?: string | null;
  revision: string;
  appOwned: boolean;
};

export function isExternalCliSession(
  row: Pick<SessionRow, "cliSource"> | null | undefined,
): boolean {
  return !!row?.cliSource && !row.cliSource.appOwned;
}

export function sessionDeleteConfirmation(
  rows: readonly SessionRow[],
  t: ReturnType<typeof createT>,
): string {
  if (rows.length === 1) {
    return t(isExternalCliSession(rows[0]) ? "session.deleteExternalConfirm" : "session.deleteConfirm", {
      name: rows[0].title || t("session.untitled"),
    });
  }
  const message = t("session.deleteManyConfirm", { n: String(rows.length) });
  return rows.some(isExternalCliSession)
    ? `${message}\n\n${t("session.deleteExternalNote")}`
    : message;
}
