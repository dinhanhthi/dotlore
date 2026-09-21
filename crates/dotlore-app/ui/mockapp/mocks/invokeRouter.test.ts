import { beforeEach, describe, expect, it } from "vitest";

import { route } from "./invokeRouter";
import { MOCK_FILE_PATHS } from "./plugin-dialog";
import { resetStore, store } from "./store";

beforeEach(() => {
  resetStore({ roots: [], files: {}, conflicts: {} });
});

describe("add_root", () => {
  it("rejects a listed file path with the same message Rust uses", async () => {
    expect(MOCK_FILE_PATHS.size).toBeGreaterThan(0);
    for (const path of MOCK_FILE_PATHS) {
      await expect(route("add_root", { path })).rejects.toThrow(
        `${path} is not a directory`,
      );
      expect(store.roots.some((row) => row.path === path)).toBe(false);
    }
  });

  it("accepts a folder whose basename looks like a file", async () => {
    const path = "/Users/demo/Projects/project.v2";
    expect(MOCK_FILE_PATHS.has(path)).toBe(false);
    await route("add_root", { path });
    expect(store.roots.some((row) => row.path === path)).toBe(true);
  });

  it("rejects Makefile because the path is listed, not because of its name", async () => {
    const path = [...MOCK_FILE_PATHS].find((item) =>
      item.endsWith("/Makefile"),
    );
    expect(path).toBeDefined();
    await expect(route("add_root", { path })).rejects.toThrow(
      `${path} is not a directory`,
    );
  });

  it("marks the new row as linked", async () => {
    await route("add_root", { path: "/Users/demo/Projects/notes" });
    expect(store.roots[0]).toEqual(expect.objectContaining({ linked: true }));
  });
});

describe("tracked_files", () => {
  it("returns TrackedFile records, not bare paths", async () => {
    resetStore({
      files: {
        demo: {
          "CLAUDE.md": { text: "hi\n", binary: false, too_large: false },
          "huge.bin": { text: null, binary: true, too_large: true },
        },
      },
    });
    await expect(route("tracked_files", { slug: "demo" })).resolves.toEqual([
      { rel: "CLAUDE.md", bytes: 3, state: "Synced" },
      { rel: "huge.bin", bytes: 2_000_000, state: "TooLarge" },
    ]);
  });
});

describe("list_linkable", () => {
  it("returns LinkableRow records, not bare slugs", async () => {
    await expect(route("list_linkable", {})).resolves.toEqual([
      {
        slug: "old-mac-notes",
        display_name: "Old Mac Notes",
        is_agent: false,
      },
    ]);
  });

  it("omits slugs already in store.roots", async () => {
    resetStore({
      roots: [
        {
          slug: "old-mac-notes",
          path: "/Users/demo/Notes/old",
          name: "old",
          is_agent: false,
          linked: true,
          status: { kind: "Synced" },
        },
      ],
      files: {},
      conflicts: {},
    });
    await expect(route("list_linkable", {})).resolves.toEqual([]);
  });
});

describe("link_root", () => {
  it("marks the row linked and drops that slug from linkable", async () => {
    await route("link_root", {
      slug: "old-mac-notes",
      path: "/Users/demo/Notes/old",
    });
    expect(store.roots[0]).toEqual(
      expect.objectContaining({ linked: true, slug: "old-mac-notes" }),
    );
    expect(store.linkable).toEqual([]);
  });
});
