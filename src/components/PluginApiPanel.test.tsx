// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({
  isDesktopHost: vi.fn(() => true),
  pluginApiStatus: vi.fn(),
  pluginApiCatalog: vi.fn(),
  accountStatus: vi.fn(),
  pluginApiConnect: vi.fn(),
  pluginApiDisconnect: vi.fn(),
  pluginApiList: vi.fn(),
  pluginApiValidateInline: vi.fn(),
  pluginApiInstall: vi.fn(),
  pluginApiAction: vi.fn(),
  pluginApiBridgeStatus: vi.fn(),
  pluginApiBridgeApply: vi.fn(),
  pluginApiBridgeRetry: vi.fn(),
  pluginApiBridgeVerify: vi.fn(),
  pluginApiBridgeRemove: vi.fn(),
  waitForPluginApiOperation: vi.fn(),
  newPluginApiIdempotencyKey: vi.fn((prefix: string) => `key-${prefix}`),
}));
vi.mock("@/lib/api", () => api);

import { PluginApiPanel } from "./PluginApiPanel";
import type { PluginApiPublicPlugin } from "@/lib/api/pluginApi";

const disconnected = {
  connected: false,
  endpoint: null,
  connectionId: null,
  capabilities: null,
};
const connected = {
  connected: true,
  endpoint: "http://127.0.0.1:4319/",
  connectionId: "connection-1",
  capabilities: {
    scopes: ["read", "manage"],
    workspaceIds: ["local"],
  },
};
const account = {
  profile: {
    signedIn: true,
    authMode: "oauth",
    email: "user@example.test",
    displayName: "User",
    userId: "user-1",
    teamId: null,
    principalType: "user",
    expiresAt: null,
    expired: false,
    hasRefresh: true,
    oidcIssuer: null,
  },
  hasOfficialKey: false,
  hasRelayKey: false,
  relayBaseUrl: null,
  cliAuthPresent: true,
  cliFound: true,
  cliPath: "/usr/bin/supercharge",
  channel: "official",
  billing: {},
  heatmap: [],
  callLogs: [],
  usageManageUrl: "",
  subscribeUrl: "",
};

function plugin(
  revision: number,
  options: { trusted?: boolean; enabled?: boolean; configured?: boolean } = {},
): PluginApiPublicPlugin {
  return {
    id: "reviewed-mcp",
    revision,
    trusted: options.trusted ?? false,
    enabled: options.enabled ?? false,
    activeDigest: "a".repeat(64),
    configuredKeys: options.configured ? ["token"] : [],
    releases: [
      {
        digest: "a".repeat(64),
        version: "1.0.0",
        installedAt: "2026-10-01T00:00:00.000Z",
      },
    ],
    manifest: {
      schemaVersion: 1,
      id: "reviewed-mcp",
      name: "Reviewed MCP",
      version: "1.0.0",
      apiVersion: "1",
      permissions: ["mcp:stdio"],
      components: [],
      mcpServers: {
        main: {
          transport: "stdio",
          command: "node",
          args: [],
          env: {},
        },
      },
      hooks: [],
      userConfig: {
        token: { required: true, sensitive: true },
      },
    },
  };
}

beforeEach(() => {
  api.pluginApiCatalog.mockResolvedValue({ schemaVersion: 1, plugins: [] });
  api.isDesktopHost.mockReturnValue(true);
  api.accountStatus.mockResolvedValue(account);
  api.pluginApiBridgeStatus.mockResolvedValue({
    configured: false,
    revision: null,
    appSessionId: "session-1",
    pluginId: null,
    serverId: null,
    serverName: null,
    workspaceId: null,
    releaseDigest: null,
    endpoint: null,
    generation: null,
    agentSessionId: null,
    policyExpiresAt: null,
    status: "removed",
    updatedAt: null,
    lastError: null,
    tools: [],
  });
  api.newPluginApiIdempotencyKey.mockImplementation(
    (prefix: string) => `key-${prefix}`,
  );
});

afterEach(() => {
  cleanup();
  vi.resetAllMocks();
});

describe("PluginApiPanel", () => {
  it("shows the shared catalog separately and installs its pinned release into the connected service", async () => {
    api.pluginApiStatus.mockResolvedValue(connected);
    api.pluginApiList.mockResolvedValue({ plugins: [] });
    api.pluginApiCatalog.mockResolvedValue({ schemaVersion: 1, plugins: [{
      id: "vectra-echo", name: "Echo MCP", version: "1.0.0", description: "Safe echo fixture",
      source: { type: "archive", url: "https://infra.xibeai.in/managed-mcp/vectra-echo-1.0.0.tar.gz", sha256: "a".repeat(64) },
      manifest: { ...plugin(1).manifest, id: "vectra-echo", name: "Echo MCP", userConfig: {} },
    }] });
    api.pluginApiInstall.mockResolvedValue({ id: "operation-1", status: "running" });
    api.waitForPluginApiOperation.mockResolvedValue({ id: "operation-1", status: "succeeded", result: { id: "vectra-echo" } });
    render(<PluginApiPanel locale="en" />);
    await screen.findByText("Echo MCP · 1.0.0");
    fireEvent.click(screen.getByRole("button", { name: "Install into this service" }));
    await waitFor(() => expect(api.pluginApiInstall).toHaveBeenCalledWith(
      "connection-1",
      { type: "archive", url: "https://infra.xibeai.in/managed-mcp/vectra-echo-1.0.0.tar.gz", sha256: "a".repeat(64) },
      "key-catalog-vectra-echo-1.0.0",
    ));
  });

  it("submits the Plugin API key once and clears the password field", async () => {
    api.pluginApiStatus.mockResolvedValue(disconnected);
    api.pluginApiConnect.mockResolvedValue(connected);
    api.pluginApiList.mockResolvedValue({ plugins: [] });

    render(<PluginApiPanel locale="en" />);
    const key = await screen.findByLabelText("Plugin API key");
    fireEvent.change(key, { target: { value: "x".repeat(32) } });
    fireEvent.click(screen.getByRole("button", { name: "Connect" }));

    await waitFor(() =>
      expect(api.pluginApiConnect).toHaveBeenCalledWith(
        "http://127.0.0.1:4319",
        "x".repeat(32),
      ),
    );
    await screen.findByText("Connected");
    expect((key as HTMLInputElement).value).toBe("");
    expect(screen.getByText("Signed in")).toBeTruthy();
  });

  it("submits a scoped bridge key once, then verifies only after consent", async () => {
    const enabled = plugin(4, {
      configured: true,
      trusted: true,
      enabled: true,
    });
    api.pluginApiStatus.mockResolvedValue(connected);
    api.pluginApiList.mockResolvedValue({ plugins: [enabled] });
    const applied = {
      configured: true,
      revision: 1,
      appSessionId: "11111111-1111-4111-8111-111111111111",
      pluginId: "reviewed-mcp",
      serverId: "main",
      serverName: "supercharge-plugin-api-111111111111",
      workspaceId: "local",
      releaseDigest: "a".repeat(64),
      endpoint: connected.endpoint,
      generation: "22222222-2222-4222-8222-222222222222",
      agentSessionId: "agent-1",
      policyExpiresAt: "2026-10-02T00:00:00Z",
      status: "applied" as const,
      updatedAt: "2026-10-01T00:00:00Z",
      lastError: null,
      tools: [{ name: "t_echo", description: "Echo" }],
    };
    api.pluginApiBridgeApply.mockResolvedValue(applied);
    api.pluginApiBridgeVerify.mockResolvedValue({
      ...applied,
      status: "ready",
    });

    render(
      <PluginApiPanel
        locale="en"
        appSessionId="11111111-1111-4111-8111-111111111111"
        agentSessionId="agent-1"
      />,
    );
    const bridgeKey = await screen.findByLabelText("Bridge-only key");
    fireEvent.change(bridgeKey, { target: { value: "b".repeat(32) } });
    fireEvent.click(
      screen.getByRole("button", { name: "Apply to selected session" }),
    );

    await waitFor(() =>
      expect(api.pluginApiBridgeApply).toHaveBeenCalledWith({
        connectionId: "connection-1",
        appSessionId: "11111111-1111-4111-8111-111111111111",
        pluginId: "reviewed-mcp",
        serverId: "main",
        workspaceId: "local",
        bridgeKey: "b".repeat(32),
        expectedRevision: null,
      }),
    );
    expect((bridgeKey as HTMLInputElement).value).toBe("");
    expect(
      (
        screen.getByRole("button", {
          name: "Run verification call",
        }) as HTMLButtonElement
      ).disabled,
    ).toBe(true);
    fireEvent.click(
      screen.getByRole("checkbox", { name: /consent to one real call/i }),
    );
    fireEvent.click(
      screen.getByRole("button", { name: "Run verification call" }),
    );
    await waitFor(() =>
      expect(api.pluginApiBridgeVerify).toHaveBeenCalledWith({
        appSessionId: "11111111-1111-4111-8111-111111111111",
        expectedRevision: 1,
        tool: "t_echo",
        arguments: {},
        consent: true,
      }),
    );
  });

  it("uses current revisions for configure, exact approval, and enable", async () => {
    const installed = plugin(1);
    const configured = plugin(2, { configured: true });
    const trusted = plugin(3, { configured: true, trusted: true });
    const enabled = plugin(4, {
      configured: true,
      trusted: true,
      enabled: true,
    });
    api.pluginApiStatus.mockResolvedValue(connected);
    api.pluginApiList
      .mockResolvedValueOnce({ plugins: [installed] })
      .mockResolvedValueOnce({ plugins: [configured] })
      .mockResolvedValueOnce({ plugins: [trusted] })
      .mockResolvedValueOnce({ plugins: [enabled] });
    api.pluginApiAction
      .mockResolvedValueOnce({ id: "configure", status: "running" })
      .mockResolvedValueOnce({ id: "trust", status: "running" })
      .mockResolvedValueOnce({ id: "enable", status: "running" });
    api.waitForPluginApiOperation
      .mockResolvedValueOnce({
        id: "configure",
        status: "succeeded",
        result: configured,
      })
      .mockResolvedValueOnce({
        id: "trust",
        status: "succeeded",
        result: trusted,
      })
      .mockResolvedValueOnce({
        id: "enable",
        status: "succeeded",
        result: enabled,
      });

    render(<PluginApiPanel locale="en" />);
    const secret = await screen.findByLabelText("New value for token *");
    fireEvent.change(secret, { target: { value: "write-only-value" } });
    fireEvent.click(
      screen.getByRole("button", { name: "Save write-only configuration" }),
    );

    await waitFor(() =>
      expect(api.pluginApiAction).toHaveBeenNthCalledWith(
        1,
        "connection-1",
        "reviewed-mcp",
        {
          action: "configure",
          expectedRevision: 1,
          values: { token: "write-only-value" },
        },
        "key-configure-reviewed-mcp",
      ),
    );
    await waitFor(() => expect((secret as HTMLInputElement).value).toBe(""));

    fireEvent.click(
      screen.getByRole("checkbox", {
        name: /reviewed this exact definition/i,
      }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Approve permission" }));
    await waitFor(() =>
      expect(api.pluginApiAction).toHaveBeenNthCalledWith(
        2,
        "connection-1",
        "reviewed-mcp",
        {
          action: "trust",
          expectedRevision: 2,
          permissions: ["mcp:stdio"],
        },
        "key-trust-reviewed-mcp",
      ),
    );

    fireEvent.click(
      await screen.findByRole("button", { name: "Enable managed MCP" }),
    );
    await waitFor(() =>
      expect(api.pluginApiAction).toHaveBeenNthCalledWith(
        3,
        "connection-1",
        "reviewed-mcp",
        { action: "enable", expectedRevision: 3 },
        "key-enable-reviewed-mcp",
      ),
    );
    await screen.findByText("Not applied to an agent session");
  });
});
