import type {
  PluginApiHttpServer,
  PluginApiManifest,
  PluginApiPermission,
  PluginApiStdioServer,
} from "./api/pluginApi";

export type InlineMcpTransport = "stdio" | "http";

export type InlineMcpDraft = {
  id: string;
  name: string;
  description: string;
  version: string;
  serverId: string;
  transport: InlineMcpTransport;
  command: string;
  argsText: string;
  url: string;
  config: InlineMcpConfigDraft[];
};

export type InlineMcpConfigDraft = {
  key: string;
  targetName: string;
  target: "env" | "header";
  description: string;
  required: boolean;
};

export const EMPTY_INLINE_MCP_DRAFT: InlineMcpDraft = {
  id: "",
  name: "",
  description: "",
  version: "1.0.0",
  serverId: "main",
  transport: "stdio",
  command: "",
  argsText: "",
  url: "",
  config: [],
};

const idPattern = /^[a-z][a-z0-9-]{0,63}$/;
const envPattern = /^[A-Za-z_][A-Za-z0-9_]*$/;
const semverPattern =
  /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-((?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*)(?:\.(?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*))*))?(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?$/;

export function parseArgumentArray(text: string): string[] {
  const trimmed = text.trim();
  if (!trimmed) return [];
  let parsed: unknown;
  try {
    parsed = JSON.parse(trimmed);
  } catch {
    throw new Error("Arguments must be a JSON array of strings");
  }
  if (
    !Array.isArray(parsed) ||
    parsed.length > 64 ||
    parsed.some((value) => typeof value !== "string" || value.length > 8192)
  ) {
    throw new Error("Arguments must be a JSON array containing at most 64 strings");
  }
  return parsed;
}

function validateId(value: string, label: string): string {
  const id = value.trim();
  if (!idPattern.test(id)) {
    throw new Error(
      `${label} must start with a lowercase letter and contain only lowercase letters, digits, and hyphens`,
    );
  }
  return id;
}

function configFields(draft: InlineMcpDraft) {
  const fields: PluginApiManifest["userConfig"] = {};
  const env: Record<string, string> = {};
  const headers: Record<string, string> = {};
  for (const raw of draft.config) {
    if (
      (draft.transport === "stdio" && raw.target !== "env") ||
      (draft.transport === "http" && raw.target !== "header")
    ) {
      throw new Error(
        draft.transport === "stdio"
          ? "stdio secrets must map to environment variables"
          : "HTTP secrets must map to request headers",
      );
    }
    const key = validateId(raw.key, "Secret key");
    const targetName = raw.targetName.trim();
    if (!targetName) throw new Error("Every secret needs an environment or header name");
    if (raw.target === "env" && !envPattern.test(targetName)) {
      throw new Error(`Invalid environment variable name: ${targetName}`);
    }
    if (raw.target === "header" && /[\r\n:]/.test(targetName)) {
      throw new Error(`Invalid HTTP header name: ${targetName}`);
    }
    if (Object.hasOwn(fields, key)) throw new Error(`Duplicate secret key: ${key}`);
    fields[key] = {
      description: raw.description.trim() || undefined,
      required: raw.required,
      sensitive: true,
    };
    const placeholder = `\${config.${key}}`;
    if (raw.target === "env") env[targetName] = placeholder;
    else headers[targetName] = placeholder;
  }
  return { fields, env, headers };
}

export function buildInlineMcpManifest(
  draft: InlineMcpDraft,
): PluginApiManifest {
  const id = validateId(draft.id, "Plugin ID");
  const serverId = validateId(draft.serverId, "Server ID");
  const name = draft.name.trim();
  if (!name || name.length > 120) {
    throw new Error("Name must contain 1–120 characters");
  }
  const version = draft.version.trim();
  if (!semverPattern.test(version)) {
    throw new Error("Version must be valid semantic versioning, such as 1.0.0");
  }
  const { fields, env, headers } = configFields(draft);
  let server: PluginApiStdioServer | PluginApiHttpServer;
  let permission: PluginApiPermission;
  if (draft.transport === "stdio") {
    const command = draft.command.trim();
    if (!command) throw new Error("Command is required for a stdio MCP server");
    server = {
      transport: "stdio",
      command,
      args: parseArgumentArray(draft.argsText),
      env,
    };
    permission = "mcp:stdio";
  } else {
    const url = draft.url.trim();
    let parsed: URL;
    try {
      parsed = new URL(url);
    } catch {
      throw new Error("Enter a valid Streamable HTTP MCP URL");
    }
    if (parsed.protocol !== "https:" && parsed.protocol !== "http:") {
      throw new Error("Streamable HTTP MCP must use HTTP or HTTPS");
    }
    server = { transport: "http", url, headers };
    permission = "mcp:http";
  }
  return {
    schemaVersion: 1,
    id,
    name,
    version,
    apiVersion: "1",
    ...(draft.description.trim()
      ? { description: draft.description.trim() }
      : {}),
    permissions: [permission],
    components: [],
    mcpServers: { [serverId]: server },
    hooks: [],
    userConfig: fields,
  };
}

function asRecord(value: unknown): Record<string, unknown> | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

function secretKey(name: string, used: Set<string>): string {
  const base =
    name
      .trim()
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "")
      .replace(/^[^a-z]+/, "")
      .slice(0, 56) || "secret";
  let candidate = base;
  let suffix = 2;
  while (used.has(candidate)) candidate = `${base.slice(0, 59)}-${suffix++}`;
  used.add(candidate);
  return candidate;
}

function importedConfig(server: Record<string, unknown>): InlineMcpConfigDraft[] {
  const used = new Set<string>();
  const result: InlineMcpConfigDraft[] = [];
  const add = (target: "env" | "header", raw: unknown) => {
    const values = asRecord(raw);
    if (!values) return;
    for (const name of Object.keys(values)) {
      result.push({
        key: secretKey(name, used),
        targetName: name,
        target,
        description: "",
        required: true,
      });
    }
  };
  add("env", server.env);
  add("header", server.headers);
  return result;
}

/**
 * Import only the reviewed server shape. Existing env/header values are dropped
 * because imported definitions may contain credentials and React state is not a
 * secret store. Their names become new write-only configuration fields.
 */
export function importInlineMcpDraft(value: unknown): InlineMcpDraft {
  const root = asRecord(value);
  if (!root) throw new Error("Imported definition must be a JSON object");
  const manifest = asRecord(root.manifest) ?? root;
  const serversValue = asRecord(manifest.mcpServers);
  const sourceServer = asRecord(root.server);
  const entries = serversValue ? Object.entries(serversValue) : [];
  if (serversValue && entries.length !== 1) {
    throw new Error("Import exactly one reviewed MCP server");
  }
  let serverId = typeof root.serverId === "string" ? root.serverId : "main";
  let server = sourceServer;
  if (!server && entries.length === 1) {
    serverId = entries[0]![0];
    server = asRecord(entries[0]![1]);
  }
  if (!server) {
    const only = Object.entries(root);
    if (only.length === 1 && asRecord(only[0]![1])) {
      serverId = only[0]![0];
      server = asRecord(only[0]![1]);
    } else if (typeof root.command === "string" || typeof root.url === "string") {
      server = root;
    }
  }
  const rawTransport = server?.transport ?? server?.type;
  const transport: InlineMcpTransport | null =
    rawTransport === "stdio" ||
    (rawTransport === undefined && typeof server?.command === "string")
      ? "stdio"
      : rawTransport === "http" ||
          rawTransport === "streamable-http" ||
          rawTransport === "streamable_http" ||
          (rawTransport === undefined && typeof server?.url === "string")
        ? "http"
        : null;
  if (!server || !transport) {
    throw new Error(
      "Import one stdio or Streamable HTTP MCP server, or an inline Plugin API manifest containing exactly one server",
    );
  }
  const manifestId = typeof manifest.id === "string" ? manifest.id : "";
  const manifestName = typeof manifest.name === "string" ? manifest.name : "";
  const manifestVersion =
    typeof manifest.version === "string" ? manifest.version : "1.0.0";
  const description =
    typeof manifest.description === "string" ? manifest.description : "";
  const args = Array.isArray(server.args)
    ? server.args.filter((value): value is string => typeof value === "string")
    : [];
  return {
    ...EMPTY_INLINE_MCP_DRAFT,
    id: manifestId,
    name: manifestName || manifestId,
    description,
    version: manifestVersion,
    serverId,
    transport,
    command: typeof server.command === "string" ? server.command : "",
    argsText: JSON.stringify(args, null, 2),
    url: typeof server.url === "string" ? server.url : "",
    config: importedConfig(server),
  };
}
