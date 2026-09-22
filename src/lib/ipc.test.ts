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
