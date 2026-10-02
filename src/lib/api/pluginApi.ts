/** Desktop-only client for the standalone Plugin API Tauri adapter. */

import { invoke, isDesktopHost } from "./host";

export type PluginApiPermission = "mcp:stdio" | "mcp:http";

export type PluginApiStdioServer = {
  transport: "stdio";
  command: string;
  args: string[];
  env: Record<string, string>;
};

export type PluginApiHttpServer = {
  transport: "http";
  url: string;
  headers: Record<string, string>;
};

export type PluginApiManifest = {
  schemaVersion: 1;
  id: string;
  name: string;
  version: string;
  apiVersion: "1";
  description?: string;
  permissions: PluginApiPermission[];
  components: [];
  mcpServers: Record<string, PluginApiStdioServer | PluginApiHttpServer>;
  hooks: [];
  userConfig: Record<
    string,
    {
      description?: string;
      required: boolean;
      sensitive: boolean;
    }
  >;
};

export type PluginApiCatalogEntry = {
  id: string;
  name: string;
  version: string;
  description: string;
  source: { type: "archive"; url: string; sha256: string };
  manifest: PluginApiManifest;
};

export async function pluginApiCatalog(): Promise<{ schemaVersion: 1; plugins: PluginApiCatalogEntry[] }> {
  requireDesktop();
  return invoke("plugin_api_catalog");
}

export type PluginApiSource =
  | { type: "inline"; manifest: PluginApiManifest }
  | { type: "archive"; url: string; sha256: string };

export type PluginApiCapabilities = {
  workspaceIds?: string[];
  workspaceRoots?: Record<string, string>;
  scopes?: Array<"read" | "manage" | "execute">;
  allowedPluginIds?: string[];
  allowedMcpServers?: Record<string, string[]>;
  executionBoundary?: "isolated-account" | "isolated-container";
  agentUid?: number;
  [key: string]: unknown;
};

export type PluginApiConnectionStatus = {
  connected: boolean;
  endpoint: string | null;
  connectionId: string | null;
  capabilities: PluginApiCapabilities | null;
};

export type PluginApiPublicPlugin = {
  id: string;
  revision: number;
  enabled: boolean;
  trusted: boolean;
  activeDigest: string;
  manifest: PluginApiManifest;
  configuredKeys: string[];
  releases: Array<{ digest: string; version: string; installedAt: string }>;
};

export type PluginApiOperationStatus =
  | "running"
  | "succeeded"
  | "failed"
  | "cancelled"
  | "interrupted";

export type PluginApiBridgeLifecycleStatus =
  | "saved"
  | "waiting"
  | "applied"
  | "ready"
  | "failed"
  | "removing"
  | "removed";

export type PluginApiBridgeStatus = {
  configured: boolean;
  revision: number | null;
  appSessionId: string;
  pluginId: string | null;
  serverId: string | null;
  serverName: string | null;
  workspaceId: string | null;
  releaseDigest: string | null;
  endpoint: string | null;
  generation: string | null;
  agentSessionId: string | null;
  policyExpiresAt: string | null;
  status: PluginApiBridgeLifecycleStatus;
  updatedAt: string | null;
  lastError: string | null;
  tools: Array<{ name: string; description: string | null }>;
};

export type PluginApiOperation<T = unknown> = {
  id: string;
  status: PluginApiOperationStatus;
  createdAt: string;
  updatedAt: string;
  workspaceId?: string;
  result?: T;
  error?: { code: string; message: string };
};

export type PluginApiAction =
  | {
      action: "trust";
      expectedRevision: number;
      permissions: PluginApiPermission[];
    }
  | {
      action: "enable" | "disable" | "uninstall";
      expectedRevision: number;
    }
  | {
      action: "configure";
      expectedRevision: number;
      values: Record<string, string>;
    }
  | {
      action: "update";
      expectedRevision: number;
      source: PluginApiSource;
    }
  | {
      action: "rollback";
      expectedRevision: number;
      digest: string;
    };

type PluginApiRequest =
  | { kind: "list" }
  | { kind: "validate"; manifest: PluginApiManifest }
  | { kind: "install"; source: PluginApiSource; idempotency_key: string }
  | {
      kind: "action";
      plugin_id: string;
      action: PluginApiAction;
      idempotency_key: string;
    }
  | { kind: "operation"; operation_id: string };

function requireDesktop() {
  if (!isDesktopHost()) {
    throw new Error("Plugin API management requires the Supercharge desktop app");
  }
}

export async function pluginApiStatus(): Promise<PluginApiConnectionStatus> {
  if (!isDesktopHost()) {
    return {
      connected: false,
      endpoint: null,
      connectionId: null,
      capabilities: null,
    };
  }
  return invoke<PluginApiConnectionStatus>("plugin_api_status");
}

export async function pluginApiConnect(
  endpoint: string,
  apiKey: string,
): Promise<PluginApiConnectionStatus> {
  requireDesktop();
  return invoke<PluginApiConnectionStatus>("plugin_api_connect", {
    endpoint,
    apiKey,
  });
}

export async function pluginApiDisconnect(): Promise<PluginApiConnectionStatus> {
  requireDesktop();
  return invoke<PluginApiConnectionStatus>("plugin_api_disconnect");
}

async function request<T>(
  connectionId: string,
  input: PluginApiRequest,
): Promise<T> {
  requireDesktop();
  return invoke<T>("plugin_api_request", {
    connectionId,
    request: input,
  });
}

export function pluginApiList(
  connectionId: string,
): Promise<{ plugins: PluginApiPublicPlugin[] }> {
  return request(connectionId, { kind: "list" });
}

export function pluginApiValidateInline(
  connectionId: string,
  manifest: PluginApiManifest,
): Promise<{ valid: true; manifest: PluginApiManifest }> {
  return request(connectionId, { kind: "validate", manifest });
}

export function pluginApiInstall(
  connectionId: string,
  source: PluginApiSource,
  idempotencyKey: string,
): Promise<PluginApiOperation<PluginApiPublicPlugin>> {
  return request(connectionId, {
    kind: "install",
    source,
    idempotency_key: idempotencyKey,
  });
}

export function pluginApiAction(
  connectionId: string,
  pluginId: string,
  action: PluginApiAction,
  idempotencyKey: string,
): Promise<PluginApiOperation<PluginApiPublicPlugin>> {
  return request(connectionId, {
    kind: "action",
    plugin_id: pluginId,
    action,
    idempotency_key: idempotencyKey,
  });
}

export function pluginApiOperation<T = unknown>(
  connectionId: string,
  operationId: string,
): Promise<PluginApiOperation<T>> {
  return request(connectionId, {
    kind: "operation",
    operation_id: operationId,
  });
}

export class PluginApiOperationError extends Error {
  constructor(public readonly operation: PluginApiOperation) {
    super(
      operation.error?.message ||
        `Plugin API operation ended with ${operation.status}`,
    );
    this.name = "PluginApiOperationError";
  }
}

function sleep(ms: number, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal?.aborted) {
      reject(signal.reason ?? new DOMException("Aborted", "AbortError"));
      return;
    }
    const timer = window.setTimeout(() => {
      signal?.removeEventListener("abort", onAbort);
      resolve();
    }, ms);
    const onAbort = () => {
      window.clearTimeout(timer);
      reject(signal?.reason ?? new DOMException("Aborted", "AbortError"));
    };
    signal?.addEventListener("abort", onAbort, { once: true });
  });
}

export async function waitForPluginApiOperation<T>(
  connectionId: string,
  initial: PluginApiOperation<T>,
  options: {
    signal?: AbortSignal;
    timeoutMs?: number;
    pollIntervalMs?: number;
    onUpdate?: (operation: PluginApiOperation<T>) => void;
  } = {},
): Promise<PluginApiOperation<T>> {
  const timeoutMs = options.timeoutMs ?? 125_000;
  const pollIntervalMs = Math.min(
    5_000,
    Math.max(100, options.pollIntervalMs ?? 400),
  );
  const deadline = Date.now() + timeoutMs;
  let operation = initial;
  options.onUpdate?.(operation);

  while (operation.status === "running") {
    if (Date.now() >= deadline) {
      throw new Error(
        "Plugin API operation is still running; refresh its status before retrying",
      );
    }
    await sleep(Math.min(pollIntervalMs, deadline - Date.now()), options.signal);
    operation = await pluginApiOperation<T>(connectionId, operation.id);
    options.onUpdate?.(operation);
  }

  if (operation.status !== "succeeded") {
    throw new PluginApiOperationError(operation);
  }
  return operation;
}

export async function pluginApiBridgeStatus(
  appSessionId: string,
): Promise<PluginApiBridgeStatus> {
  requireDesktop();
  return invoke<PluginApiBridgeStatus>("plugin_api_bridge_status", {
    appSessionId,
  });
}

export async function pluginApiBridgeApply(input: {
  connectionId: string;
  appSessionId: string;
  pluginId: string;
  serverId: string;
  workspaceId: string;
  bridgeKey: string;
  expectedRevision?: number | null;
}): Promise<PluginApiBridgeStatus> {
  requireDesktop();
  return invoke<PluginApiBridgeStatus>("plugin_api_bridge_apply", {
    ...input,
    expectedRevision: input.expectedRevision ?? null,
  });
}

export async function pluginApiBridgeRetry(
  appSessionId: string,
  expectedRevision: number,
): Promise<PluginApiBridgeStatus> {
  requireDesktop();
  return invoke<PluginApiBridgeStatus>("plugin_api_bridge_retry", {
    appSessionId,
    expectedRevision,
  });
}

export async function pluginApiBridgeVerify(input: {
  appSessionId: string;
  expectedRevision: number;
  tool: string;
  arguments: Record<string, unknown>;
  consent: boolean;
}): Promise<PluginApiBridgeStatus> {
  requireDesktop();
  return invoke<PluginApiBridgeStatus>("plugin_api_bridge_verify", input);
}

export async function pluginApiBridgeRemove(
  appSessionId: string,
  expectedRevision: number,
): Promise<PluginApiBridgeStatus> {
  requireDesktop();
  return invoke<PluginApiBridgeStatus>("plugin_api_bridge_remove", {
    appSessionId,
    expectedRevision,
  });
}

export function newPluginApiIdempotencyKey(prefix: string): string {
  const safePrefix =
    prefix.toLowerCase().replace(/[^a-z0-9_-]+/g, "-").slice(0, 24) ||
    "request";
  const id =
    typeof crypto !== "undefined" && typeof crypto.randomUUID === "function"
      ? crypto.randomUUID()
      : `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
  return `desktop-${safePrefix}-${id}`.slice(0, 128);
}
