import { describe, expect, it } from "vitest";
import {
  EMPTY_INLINE_MCP_DRAFT,
  buildInlineMcpManifest,
  importInlineMcpDraft,
  parseArgumentArray,
} from "./pluginApiInline";

describe("inline Plugin API MCP definitions", () => {
  it("parses argument arrays without shell tokenization", () => {
    expect(parseArgumentArray('["--flag", "value with spaces"]')).toEqual([
      "--flag",
      "value with spaces",
    ]);
    expect(() => parseArgumentArray("--flag value")).toThrow(
      "JSON array of strings",
    );
  });

  it("builds a single reviewed stdio server with secret placeholders", () => {
    expect(
      buildInlineMcpManifest({
        ...EMPTY_INLINE_MCP_DRAFT,
        id: "reviewed-mcp",
        name: "Reviewed MCP",
        command: "/usr/bin/node",
        argsText: '["server.mjs"]',
        config: [
          {
            key: "token",
            targetName: "API_TOKEN",
            target: "env",
            description: "Provider token",
            required: true,
          },
        ],
      }),
    ).toEqual({
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
          command: "/usr/bin/node",
          args: ["server.mjs"],
          env: { API_TOKEN: "${config.token}" },
        },
      },
      hooks: [],
      userConfig: {
        token: {
          description: "Provider token",
          required: true,
          sensitive: true,
        },
      },
    });
  });

  it("drops imported env and header values instead of retaining possible secrets", () => {
    const stdio = importInlineMcpDraft({
      transport: "stdio",
      command: "node",
      args: ["server.mjs"],
      env: { TOKEN: "do-not-retain" },
    });
    expect(stdio.command).toBe("node");
    expect(stdio.argsText).toContain("server.mjs");
    expect(stdio.config).toEqual([
      {
        key: "token",
        targetName: "TOKEN",
        target: "env",
        description: "",
        required: true,
      },
    ]);
    expect(JSON.stringify(stdio)).not.toContain("do-not-retain");

    const http = importInlineMcpDraft({
      schemaVersion: 1,
      id: "remote-mcp",
      name: "Remote MCP",
      version: "1.2.3",
      mcpServers: {
        remote: {
          transport: "http",
          url: "https://mcp.example.test/rpc",
          headers: { Authorization: "Bearer do-not-retain" },
        },
      },
    });
    expect(http.transport).toBe("http");
    expect(http.url).toBe("https://mcp.example.test/rpc");
    expect(http.config).toEqual([
      {
        key: "authorization",
        targetName: "Authorization",
        target: "header",
        description: "",
        required: true,
      },
    ]);
    expect(JSON.stringify(http)).not.toContain("do-not-retain");
  });
});
