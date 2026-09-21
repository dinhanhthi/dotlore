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

describe("remove_root", () => {
  it("drops entries and picker extras so a re-add starts empty", async () => {
    resetStore({
      roots: [
        {
          slug: "demo",
          path: "/Users/demo/Projects/demo",
          name: "demo",
          is_agent: false,
          linked: true,
          status: { kind: "Synced" },
        },
      ],
      files: {
        demo: { "a.md": { text: "old\n", binary: false, too_large: false } },
      },
      conflicts: {},
      entries: { demo: [{ key: "a.md", kind: "file", covering: [] }] },
      pickerExtra: {
        demo: { "": [{ name: "extra", kind: "file", rel: "extra" }] },
      },
    });

    await route("remove_root", { slug: "demo" });

    expect(store.entries.demo).toBeUndefined();
    expect(store.pickerExtra.demo).toBeUndefined();

    await route("add_root", { path: "/Users/demo/Projects/demo", slug: "demo" });
    expect(store.entries.demo).toEqual([]);
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

describe("list_entries", () => {
  it("seeds explicit entries from demo file keys", async () => {
    resetStore();
    await expect(route("list_entries", { slug: "dotlore" })).resolves.toEqual([
      { key: ".claude/settings.json", kind: "file", covering: [] },
      { key: "CLAUDE.md", kind: "file", covering: [] },
      { key: "docs/architecture.md", kind: "file", covering: [] },
    ]);
  });
});

describe("list_entry_children", () => {
  it("rejects a path that leaves the project root", async () => {
    resetStore({
      files: {
        demo: { "a.md": { text: "x\n", binary: false, too_large: false } },
      },
    });
    await expect(
      route("list_entry_children", { slug: "demo", rel: ".." }),
    ).rejects.toThrow(/unsafe path/);
  });

  it("omits symlink candidates", async () => {
    resetStore({
      files: {
        demo: { "a.md": { text: "x\n", binary: false, too_large: false } },
      },
      pickerExtra: {
        demo: {
          "": [{ name: "link", kind: "symlink", rel: "link" }],
        },
      },
    });
    await expect(
      route("list_entry_children", { slug: "demo", rel: "" }),
    ).resolves.toEqual([{ name: "a.md", kind: "file", rel: "a.md" }]);
  });
});

describe("inspect_entry", () => {
  it("returns bytes and the folder limit for a file", async () => {
    resetStore({
      files: {
        demo: { "a.md": { text: "hi\n", binary: false, too_large: false } },
      },
    });
    await expect(route("inspect_entry", { slug: "demo", rel: "a.md" })).resolves.toEqual(
      {
        kind: "file",
        bytes: 3,
        folder_limit: 200 * 1024 * 1024,
        confirmation_required: false,
        skipped_too_large: [],
      },
    );
  });
});

describe("track_entry", () => {
  const folderFiles = {
    "big/a.bin": {
      text: null,
      binary: true,
      too_large: false,
      bytes: 40 * 1024 * 1024,
    },
    "big/b.bin": {
      text: null,
      binary: true,
      too_large: false,
      bytes: 40 * 1024 * 1024,
    },
    "big/c.bin": {
      text: null,
      binary: true,
      too_large: false,
      bytes: 40 * 1024 * 1024,
    },
    "big/d.bin": {
      text: null,
      binary: true,
      too_large: false,
      bytes: 40 * 1024 * 1024,
    },
    "big/e.bin": {
      text: null,
      binary: true,
      too_large: false,
      bytes: 41 * 1024 * 1024,
    },
  };

  it("does not mutate when an oversized folder is not confirmed", async () => {
    resetStore({ files: { demo: folderFiles } });
    const result = await route("track_entry", { slug: "demo", rel: "big" });
    expect(result).toEqual(
      expect.objectContaining({
        outcome: "needs_confirmation",
        confirmation_required: true,
      }),
    );
    expect(store.entries.demo ?? []).not.toEqual(
      expect.arrayContaining([expect.objectContaining({ key: "big/" })]),
    );
  });

  it("requires a fresh confirmation when the folder grew", async () => {
    resetStore({ files: { demo: folderFiles } });
    const preview = (await route("inspect_entry", {
      slug: "demo",
      rel: "big",
    })) as { bytes: number };
    store.files.demo!["big/e.bin"] = {
      text: null,
      binary: true,
      too_large: false,
      bytes: 50 * 1024 * 1024,
    };
    const result = await route("track_entry", {
      slug: "demo",
      rel: "big",
      confirmedFolderBytes: preview.bytes,
    });
    expect(result).toEqual(
      expect.objectContaining({ outcome: "needs_confirmation" }),
    );
    expect(store.entries.demo ?? []).not.toEqual(
      expect.arrayContaining([expect.objectContaining({ key: "big/" })]),
    );
  });

  it("rejects an oversized file and does not add it", async () => {
    resetStore({
      files: {
        demo: {
          "huge.bin": {
            text: null,
            binary: true,
            too_large: false,
            bytes: 51 * 1024 * 1024,
          },
        },
      },
      entries: { demo: [] },
    });
    await expect(
      route("track_entry", { slug: "demo", rel: "huge.bin" }),
    ).rejects.toThrow(/51|limit/);
    expect(store.entries.demo ?? []).toEqual([]);
  });
});

describe("untrack_entry", () => {
  it("keeps file bytes in the store after untrack", async () => {
    resetStore({
      files: {
        demo: { "a.md": { text: "keep\n", binary: false, too_large: false } },
      },
    });
    await route("untrack_entry", { slug: "demo", rel: "a.md" });
    expect(store.files.demo?.["a.md"]?.text).toBe("keep\n");
    expect(store.entries.demo).toEqual([]);
  });

  it("rejects an inherited-only path and names the covering entry", async () => {
    resetStore({
      files: {
        demo: {
          "docs/a.md": { text: "a\n", binary: false, too_large: false },
        },
      },
      entries: {
        demo: [{ key: "docs/", kind: "directory", covering: [] }],
      },
    });
    await expect(
      route("untrack_entry", { slug: "demo", rel: "docs/a.md" }),
    ).rejects.toThrow(/docs\//);
    expect(store.files.demo?.["docs/a.md"]?.text).toBe("a\n");
  });
});

describe("settings defaults", () => {
  it("returns 50 / 200 and seeded pattern text from emptyPopulated", async () => {
    resetStore();
    await expect(route("default_patterns", {})).resolves.toEqual(
      store.defaultPatterns,
    );
    expect(store.defaultPatterns.length).toBeGreaterThan(0);
    await expect(route("default_ignore", {})).resolves.toBe(store.defaultIgnore);
    expect(store.defaultIgnore.length).toBeGreaterThan(0);
    await expect(route("max_file_mb", {})).resolves.toBe(50);
    await expect(route("max_seed_folder_mb", {})).resolves.toBe(200);
  });

  it("round-trips the eight getters and setters", async () => {
    resetStore();
    await route("set_default_patterns", { patterns: ["docs/", "CLAUDE.md"] });
    await route("set_default_ignore", { ignore: "/chats/\n" });
    await route("set_max_file_mb", { mb: 12 });
    await route("set_max_seed_folder_mb", { mb: 80 });
    await expect(route("default_patterns", {})).resolves.toEqual([
      "docs/",
      "CLAUDE.md",
    ]);
    await expect(route("default_ignore", {})).resolves.toBe("/chats/\n");
    await expect(route("max_file_mb", {})).resolves.toBe(12);
    await expect(route("max_seed_folder_mb", {})).resolves.toBe(80);
  });

  it("restores settings fields when resetStore omits them", async () => {
    store.maxFileMb = 3;
    store.maxSeedFolderMb = 9;
    store.defaultPatterns = ["gone"];
    store.defaultIgnore = "gone";
    resetStore({ roots: [], files: {}, conflicts: {} });
    expect(store.maxFileMb).toBe(50);
    expect(store.maxSeedFolderMb).toBe(200);
    expect(store.defaultPatterns).not.toEqual(["gone"]);
    expect(store.defaultIgnore).not.toBe("gone");
  });
});
