// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";

const host = vi.hoisted(() => ({
  invoke: vi.fn(),
  isDesktopHost: vi.fn(() => true),
}));
vi.mock("./host", () => host);

import {
  pluginApiAction,
  pluginApiBridgeApply,
  pluginApiBridgeRemove,
  pluginApiBridgeRetry,
  pluginApiBridgeStatus,
  pluginApiBridgeVerify,
  pluginApiConnect,
  pluginApiInstall,
  waitForPluginApiOperation,
  type PluginApiManifest,
  type PluginApiOperation,
} from "./pluginApi";

const manifest: PluginApiManifest = {
  schemaVersion: 1,
  id: "reviewed-mcp",
  name: "Reviewed MCP",
  version: "1.0.0",
  apiVersion: "1",
  permissions: ["mcp:stdio"],
  components: [],
  mcpServers: {
    main: { transport: "stdio", command: "node", args: [], env: {} },
  },
  hooks: [],
  userConfig: {},
};

const running: PluginApiOperation = {
  id: "operation-1",
  status: "running",
  createdAt: "2026-10-01T00:00:00.000Z",
  updatedAt: "2026-10-01T00:00:00.000Z",
};

afterEach(() => {
  vi.clearAllMocks();
  host.isDesktopHost.mockReturnValue(true);
});

describe("desktop Plugin API client", () => {
  it("submits the API key only to the backend connect command", async () => {
    host.invoke.mockResolvedValue({
      connected: true,
      endpoint: "http://127.0.0.1:4319/",
      connectionId: "connection-1",
      capabilities: { workspaceIds: ["local"] },
    });

    await pluginApiConnect("http://127.0.0.1:4319", "x".repeat(32));

    expect(host.invoke).toHaveBeenCalledWith("plugin_api_connect", {
      endpoint: "http://127.0.0.1:4319",
      apiKey: "x".repeat(32),
    });
  });

  it("uses the Rust enum field names inside install and action requests", async () => {
    host.invoke.mockResolvedValue(running);

    await pluginApiInstall(
      "connection-1",
      { type: "inline", manifest },
      "install-key",
    );
    await pluginApiAction(
      "connection-1",
      "reviewed-mcp",
      { action: "enable", expectedRevision: 3 },
      "enable-key",
    );

    expect(host.invoke).toHaveBeenNthCalledWith(1, "plugin_api_request", {
      connectionId: "connection-1",
      request: {
        kind: "install",
        source: { type: "inline", manifest },
        idempotency_key: "install-key",
      },
    });
    expect(host.invoke).toHaveBeenNthCalledWith(2, "plugin_api_request", {
      connectionId: "connection-1",
      request: {
        kind: "action",
        plugin_id: "reviewed-mcp",
        action: { action: "enable", expectedRevision: 3 },
        idempotency_key: "enable-key",
      },
    });
  });

  it("uses narrow session-scoped bridge commands", async () => {
    host.invoke.mockResolvedValue({ status: "waiting" });

    await pluginApiBridgeStatus("session-1");
    await pluginApiBridgeApply({
      connectionId: "connection-1",
      appSessionId: "session-1",
      pluginId: "reviewed-mcp",
      serverId: "main",
      workspaceId: "local",
      bridgeKey: "b".repeat(32),
      expectedRevision: null,
    });
    await pluginApiBridgeRetry("session-1", 2);
    await pluginApiBridgeVerify({
      appSessionId: "session-1",
      expectedRevision: 2,
      tool: "t_echo",
      arguments: {},
      consent: true,
    });
    await pluginApiBridgeRemove("session-1", 2);

    expect(host.invoke).toHaveBeenNthCalledWith(1, "plugin_api_bridge_status", {
      appSessionId: "session-1",
    });
    expect(host.invoke).toHaveBeenNthCalledWith(2, "plugin_api_bridge_apply", {
      connectionId: "connection-1",
      appSessionId: "session-1",
      pluginId: "reviewed-mcp",
      serverId: "main",
      workspaceId: "local",
      bridgeKey: "b".repeat(32),
      expectedRevision: null,
    });
    expect(host.invoke).toHaveBeenNthCalledWith(3, "plugin_api_bridge_retry", {
      appSessionId: "session-1",
      expectedRevision: 2,
    });
    expect(host.invoke).toHaveBeenNthCalledWith(4, "plugin_api_bridge_verify", {
      appSessionId: "session-1",
      expectedRevision: 2,
      tool: "t_echo",
      arguments: {},
      consent: true,
    });
    expect(host.invoke).toHaveBeenNthCalledWith(5, "plugin_api_bridge_remove", {
      appSessionId: "session-1",
      expectedRevision: 2,
    });
  });

  it("waits for a terminal operation instead of treating 202 as success", async () => {
    const succeeded = { ...running, status: "succeeded" as const, result: { ok: true } };
    host.invoke.mockResolvedValue(succeeded);

    await expect(
      waitForPluginApiOperation("connection-1", running, {
        pollIntervalMs: 100,
      }),
    ).resolves.toEqual(succeeded);
    expect(host.invoke).toHaveBeenCalledWith("plugin_api_request", {
      connectionId: "connection-1",
      request: { kind: "operation", operation_id: "operation-1" },
    });
  });
});
