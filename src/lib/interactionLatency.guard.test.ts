import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const root = join(__dirname, "../..");

function source(path: string): string {
  return readFileSync(join(root, path), "utf8");
}

describe("desktop interaction latency guards", () => {
  it("dispatches Tauri IPC without yielding through a dynamic import", () => {
    const host = source("src/lib/api/host.ts");
    expect(host).toContain('from "@tauri-apps/api/core"');
    expect(host).toContain("return tauriInvoke<T>(cmd, args)");
    expect(host).not.toContain('await import("@tauri-apps/api/core")');
  });

  it("opens Windows file dialogs on rfd's dedicated async STA thread", () => {
    const fs = source("src-tauri/src/commands/fs.rs");
    expect(fs).toContain("rfd::AsyncFileDialog::new()");
    expect(fs).not.toMatch(/spawn_blocking\([\s\S]{0,180}pick_folder/);
  });

  it("adds the returned project without redundant list round-trips", () => {
    const workbench = source("src/app/AppWorkbench.tsx");
    const finalize = workbench.slice(
      workbench.indexOf("const finalizeAddedProject"),
      workbench.indexOf("gitWorktreeHostRef", workbench.indexOf("const finalizeAddedProject")),
    );
    expect(finalize).toContain("mergeProject(p)");
    expect(finalize).not.toContain("api.projectsList()");
  });

  it("paints model selection before awaiting provider activation", () => {
    const workbench = source("src/app/AppWorkbench.tsx");
    const pick = workbench.slice(
      workbench.indexOf("const handleModelPick"),
      workbench.indexOf("const handleContextWindow"),
    );
    expect(pick.indexOf("setModelId(pick.modelId)")).toBeGreaterThanOrEqual(0);
    expect(pick.indexOf("setModelId(pick.modelId)")).toBeLessThan(
      pick.indexOf('await api.providersActivate("official")'),
    );
  });
});
