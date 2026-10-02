import { afterEach, describe, expect, it, vi } from "vitest";
import * as host from "./host";
import { cliHistorySync } from "../api";

describe("cliHistorySync", () => {
  afterEach(() => vi.restoreAllMocks());

  it("exposes the inventory command through the public API", async () => {
    vi.spyOn(host, "isTauri").mockReturnValue(true);
    vi.spyOn(host, "isMirrorClient").mockReturnValue(false);
    const invoke = vi.spyOn(host, "invoke").mockResolvedValue(true);
    await expect(cliHistorySync()).resolves.toBe(true);
    expect(invoke).toHaveBeenCalledWith("cli_history_sync");
  });

  it("does not invoke a desktop-only command from browser or mirror", async () => {
    const tauri = vi.spyOn(host, "isTauri").mockReturnValue(false);
    const mirror = vi.spyOn(host, "isMirrorClient").mockReturnValue(false);
    const invoke = vi.spyOn(host, "invoke");
    await expect(cliHistorySync()).resolves.toBe(false);
    tauri.mockReturnValue(true);
    mirror.mockReturnValue(true);
    await expect(cliHistorySync()).resolves.toBe(false);
    expect(invoke).not.toHaveBeenCalled();
  });
});
