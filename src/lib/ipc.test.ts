import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(),
}));

import { invoke } from "@tauri-apps/api/core";
import { getWorkSnapshot, listLinkable, runTask, trackedFiles } from "./ipc";

const invokeMock = vi.mocked(invoke);

beforeEach(() => {
  invokeMock.mockReset();
});

describe("trackedFiles", () => {
  it("returns TrackedFile records", async () => {
    const files = [
      { rel: "CLAUDE.md", bytes: 12, state: "Synced" },
      { rel: "docs/a.md", bytes: 4, state: "Pending" },
    ];
    invokeMock.mockResolvedValue(files);
    await expect(trackedFiles("dotlore")).resolves.toEqual(files);
  });
});

describe("listLinkable", () => {
  it("returns LinkableRow records", async () => {
    const rows = [
      {
        slug: "old-mac-notes",
        display_name: "Old Mac Notes",
        is_agent: false,
      },
    ];
    invokeMock.mockResolvedValue(rows);
    await expect(listLinkable()).resolves.toEqual(rows);
  });
});

describe("runTask", () => {
  it("carries the label while it runs and clears it afterwards", async () => {
    let seen: string | null = null;
    await runTask("Wiping cloud data…", async () => {
      seen = getWorkSnapshot().taskLabel;
    });
    expect(seen).toBe("Wiping cloud data…");
    expect(getWorkSnapshot().taskLabel).toBeNull();
  });

  it("clears the label when the task throws", async () => {
    await expect(
      runTask("Linking x…", () => Promise.reject(new Error("boom"))),
    ).rejects.toThrow("boom");
    expect(getWorkSnapshot().taskLabel).toBeNull();
    expect(getWorkSnapshot().banner).toBe("boom");
  });

  it("refuses a second task while one holds the slot, and does not run it", async () => {
    const second = vi.fn(async () => "ran");
    const first = runTask("Rebuilding x…", async () => {
      return await runTask("Wiping cloud data…", second);
    });
    await expect(first).resolves.toBeNull();
    expect(second).not.toHaveBeenCalled();
    expect(getWorkSnapshot().banner).toBe("Wait for the current task to finish");
    expect(getWorkSnapshot().taskLabel).toBeNull();
  });
});
