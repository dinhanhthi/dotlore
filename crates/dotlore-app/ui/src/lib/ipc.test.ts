import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(),
}));

import { invoke } from "@tauri-apps/api/core";
import { listLinkable, trackedFiles } from "./ipc";

const invokeMock = vi.mocked(invoke);

beforeEach(() => {
  invokeMock.mockReset();
});

describe("trackedFiles", () => {
  it("maps TrackedFile records to rel strings", async () => {
    invokeMock.mockResolvedValue([
      { rel: "CLAUDE.md", bytes: 12, state: "Synced" },
      { rel: "docs/a.md", bytes: 4, state: "Pending" },
    ]);
    await expect(trackedFiles("dotlore")).resolves.toEqual([
      "CLAUDE.md",
      "docs/a.md",
    ]);
  });
});

describe("listLinkable", () => {
  it("maps LinkableRow records to slug strings", async () => {
    invokeMock.mockResolvedValue([
      {
        slug: "old-mac-notes",
        display_name: "Old Mac Notes",
        is_agent: false,
      },
    ]);
    await expect(listLinkable()).resolves.toEqual(["old-mac-notes"]);
  });
});
