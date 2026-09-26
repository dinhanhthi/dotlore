import { beforeEach, describe, expect, it } from "vitest";

import { demoFiles } from "../fixtures/files";
import { LINKABLE_ROWS, demoRoots } from "../fixtures/roots";
import { afterMountFor } from "../scenarios/apply";
import { scenarios } from "../scenarios/index";
import { route } from "./invokeRouter";
import { MOCK_FILE_PATHS } from "./plugin-dialog";
import { emptyPopulated, resetStore, store } from "./store";

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

  it("seeds entries from defaultPatterns, matching files or empty keys", async () => {
    resetStore({
      roots: [],
      files: {
        notes: {
          "CLAUDE.md": {
            text: "hi\n",
            binary: false,
            too_large: false,
            bytes: 3,
            state: "Synced",
          },
          "README.md": {
            text: "nope\n",
            binary: false,
            too_large: false,
            bytes: 5,
            state: "Synced",
          },
        },
      },
      conflicts: {},
      entries: { notes: [] },
      defaultPatterns: ["CLAUDE.md", "docs/"],
    });
    await route("add_root", { path: "/Users/demo/Projects/notes", slug: "notes" });
    expect(store.entries.notes).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ key: "CLAUDE.md", kind: "file" }),
        expect.objectContaining({ key: "docs/", kind: "directory" }),
      ]),
    );
    expect(store.entries.notes?.some((entry) => entry.key === "README.md")).toBe(
      false,
    );
    expect(store.files.notes?.["CLAUDE.md"]?.text).toBe("hi\n");
    expect(store.files.notes?.["docs/"]).toBeUndefined();
  });

  it("creates empty tracked keys for default pattern names when no files match", async () => {
    await route("add_root", {
      path: "/Users/demo/Projects/fresh",
      slug: "fresh",
    });
    expect(store.entries.fresh?.length).toBeGreaterThan(0);
    expect(store.entries.fresh).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ key: "CLAUDE.md", kind: "file" }),
      ]),
    );
    expect(store.files.fresh?.["CLAUDE.md"]).toEqual(
      expect.objectContaining({ text: "", bytes: 0, state: "Synced" }),
    );
  });
});

describe("tracked_files", () => {
  it("omits files outside the include-list", async () => {
    resetStore({
      files: {
        demo: {
          "CLAUDE.md": {
            text: "hi\n",
            binary: false,
            too_large: false,
            bytes: 3,
            state: "Synced",
          },
          "README.md": {
            text: "nope\n",
            binary: false,
            too_large: false,
            bytes: 5,
            state: "Synced",
          },
        },
      },
      entries: { demo: [{ key: "CLAUDE.md", kind: "file", covering: [] }] },
    });
    await expect(route("tracked_files", { slug: "demo" })).resolves.toEqual([
      { rel: "CLAUDE.md", bytes: 3, state: "Synced", sensitivity: null },
    ]);
  });

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
      { rel: "CLAUDE.md", bytes: 3, state: "Synced", sensitivity: null },
      { rel: "huge.bin", bytes: 2_000_000, state: "TooLarge", sensitivity: null },
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
        demo: { "": [{ name: "extra", kind: "file", rel: "extra", sensitivity: null }] },
      },
    });

    await route("remove_root", { slug: "demo" });

    expect(store.entries.demo).toBeUndefined();
    expect(store.pickerExtra.demo).toBeUndefined();

    await route("add_root", { path: "/Users/demo/Projects/demo", slug: "demo" });
    expect(store.entries.demo?.some((entry) => entry.key === "a.md")).toBe(false);
    expect(store.entries.demo?.length).toBeGreaterThan(0);
  });
});

describe("wipe_cloud_data", () => {
  it("re-adds every current root and keeps the roots list intact", async () => {
    resetStore();
    const slugs = store.roots.map((row) => row.slug);
    expect(slugs.length).toBeGreaterThan(0);

    await expect(route("wipe_cloud_data", {})).resolves.toEqual({
      readded: slugs,
      failed: [],
    });
    expect(store.roots.map((row) => row.slug)).toEqual(slugs);
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
  it("seeds first-level demo includes (files stay files, nested paths are folders)", async () => {
    resetStore();
    await expect(route("list_entries", { slug: "dotlore" })).resolves.toEqual([
      { key: ".claude/", kind: "directory", covering: [] },
      { key: ".codex/", kind: "directory", covering: [] },
      { key: "CLAUDE.md", kind: "file", covering: [] },
      { key: "docs/", kind: "directory", covering: [] },
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
          "": [{ name: "link", kind: "symlink", rel: "link", sensitivity: null }],
        },
      },
    });
    await expect(
      route("list_entry_children", { slug: "demo", rel: "" }),
    ).resolves.toEqual([{ name: "a.md", kind: "file", rel: "a.md", sensitivity: null }]);
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
        sensitivity: null,
        secret_descendants: [],
        secret_descendants_more: false,
      },
    );
  });

  it("marks a secret file the way track_entry asks to confirm it", async () => {
    const file = { text: "k", binary: false, too_large: false };
    resetStore({ files: { demo: { "server.pem": file } }, entries: { demo: [] } });
    await expect(route("inspect_entry", { slug: "demo", rel: "server.pem" })).resolves.toMatchObject({
      sensitivity: "secret",
      secret_descendants: [],
      secret_descendants_more: false,
    });
    await expect(route("track_entry", { slug: "demo", rel: "server.pem" })).resolves.toEqual({
      outcome: "confirm_sensitive",
      paths: ["server.pem"],
      more: false,
    });
  });

  it("lists up to 20 sorted secret descendants of a folder", async () => {
    const file = { text: "k", binary: false, too_large: false };
    const files: Record<string, typeof file> = { "keys/notes.md": file };
    for (let n = 0; n < 21; n++) files[`keys/k${String(n).padStart(2, "0")}.pem`] = file;
    resetStore({ files: { demo: files }, entries: { demo: [] } });
    const expected = Array.from({ length: 20 }, (_, n) => `keys/k${String(n).padStart(2, "0")}.pem`);
    await expect(route("inspect_entry", { slug: "demo", rel: "keys" })).resolves.toMatchObject({
      sensitivity: null,
      secret_descendants: expected,
      secret_descendants_more: true,
    });
    await expect(route("track_entry", { slug: "demo", rel: "keys" })).resolves.toEqual({
      outcome: "confirm_sensitive",
      paths: expected,
      more: true,
    });
  });
});

describe("track_entry", () => {
  it("requires confirmation for a secret file without mutating entries", async () => {
    resetStore({ files: { demo: { "config/credentials.json": { text: "secret", binary: false, too_large: false } } }, entries: { demo: [] } });
    await expect(route("track_entry", { slug: "demo", rel: "config/credentials.json" })).resolves.toEqual({
      outcome: "confirm_sensitive",
      paths: ["config/credentials.json"],
      more: false,
    });
    expect(store.entries.demo).toEqual([]);
    await expect(route("track_entry", { slug: "demo", rel: "config/credentials.json", confirmedSensitive: true })).resolves.toEqual({ outcome: "done" });
    expect(store.entries.demo).toEqual(expect.arrayContaining([expect.objectContaining({ key: "config/credentials.json" })]));
  });

  it("shows sensitivity on tracked files and picker rows", async () => {
    resetStore({ files: { demo: {
      ".env.production": { text: "secret", binary: false, too_large: false },
      ".mcp.json": { text: "{}", binary: false, too_large: false },
    } }, entries: { demo: [
      { key: ".env.production", kind: "file", covering: [] },
      { key: ".mcp.json", kind: "file", covering: [] },
    ] } });
    expect(await route("tracked_files", { slug: "demo" })).toEqual(expect.arrayContaining([
      expect.objectContaining({ rel: ".env.production", sensitivity: "secret" }),
      expect.objectContaining({ rel: ".mcp.json", sensitivity: "tokenHint" }),
    ]));
    expect(await route("list_entry_children", { slug: "demo", rel: "" })).toEqual(expect.arrayContaining([
      expect.objectContaining({ rel: ".env.production", sensitivity: "secret" }),
      expect.objectContaining({ rel: ".mcp.json", sensitivity: "tokenHint" }),
    ]));
  });

  it("classifies against the edited sensitive patterns", async () => {
    const file = { text: "k", binary: false, too_large: false };
    resetStore({
      files: { demo: { "x.secret": file, ".env": file, "deep/certs/a.txt": file, "deep/credentials.md": file } },
      entries: { demo: [] },
    });
    const builtin = (await route("sensitive_patterns", {})) as string[];
    expect(builtin).toEqual(expect.arrayContaining([".env", "!credentials*.md"]));
    expect(await route("list_entry_children", { slug: "demo", rel: "deep" })).toEqual(
      expect.arrayContaining([expect.objectContaining({ rel: "deep/credentials.md", sensitivity: null })]),
    );

    await route("set_sensitive_patterns", { patterns: ["*.secret", "certs/", "!deep/certs/a.txt"] });
    expect(await route("sensitive_patterns", {})).toEqual(["*.secret", "certs/", "!deep/certs/a.txt"]);
    expect(await route("list_entry_children", { slug: "demo", rel: "" })).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ rel: "x.secret", sensitivity: "secret" }),
        expect.objectContaining({ rel: ".env", sensitivity: null }),
      ]),
    );
    await expect(route("track_entry", { slug: "demo", rel: "x.secret" })).resolves.toMatchObject({
      outcome: "confirm_sensitive",
    });
    await expect(route("track_entry", { slug: "demo", rel: ".env" })).resolves.toEqual({ outcome: "done" });
    expect(await route("tracked_files", { slug: "demo" })).toEqual(
      expect.arrayContaining([expect.objectContaining({ rel: ".env", sensitivity: null })]),
    );

    await route("set_sensitive_patterns", { patterns: ["certs/"] });
    await expect(route("inspect_entry", { slug: "demo", rel: "deep" })).resolves.toMatchObject({
      secret_descendants: ["deep/certs/a.txt"],
    });
    resetStore();
    expect(store.sensitivePatterns).toEqual(builtin);
  });

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

  it("punches a hole in an inherited-only path and keeps file bytes", async () => {
    resetStore({
      files: {
        demo: {
          "docs/a.md": { text: "a\n", binary: false, too_large: false },
          "docs/b.md": { text: "b\n", binary: false, too_large: false },
        },
      },
      entries: {
        demo: [{ key: "docs/", kind: "directory", covering: [] }],
      },
    });
    await route("untrack_entry", { slug: "demo", rel: "docs/a.md" });
    expect(store.files.demo?.["docs/a.md"]?.text).toBe("a\n");
    expect(store.excludes.demo).toContain("docs/a.md");
    expect(store.entries.demo).toEqual([
      { key: "docs/", kind: "directory", covering: [] },
    ]);
    await expect(route("tracked_files", { slug: "demo" })).resolves.toEqual([
      expect.objectContaining({ rel: "docs/b.md" }),
    ]);
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

describe("pattern catalogs", () => {
  it("overrides claude without touching defaultPatterns and rejects an unknown id", async () => {
    resetStore();
    const before = [...store.defaultPatterns];
    await route("set_pattern_catalog", {
      catalog: "claude",
      patterns: ["custom.md"],
    });
    const catalogs = (await route("pattern_catalogs", {})) as {
      id: string;
      label: string;
      lines: string[];
    }[];
    expect(catalogs.find((row) => row.id === "claude")).toEqual({
      id: "claude",
      label: "Claude",
      lines: ["custom.md"],
    });
    expect(store.defaultPatterns).toEqual(before);
    await expect(
      route("set_pattern_catalog", { catalog: "nope", patterns: ["x"] }),
    ).rejects.toThrow(/unknown pattern catalog/);
    expect(store.agentPatterns).not.toHaveProperty("nope");
  });
});

const MOCK_STATE_KEYS = [
  "agentPatterns",
  "conflicts",
  "defaultIgnore",
  "defaultPatterns",
  "entries",
  "excludes",
  "files",
  "gitMissing",
  "linkable",
  "loginItem",
  "maxFileMb",
  "maxSeedFolderMb",
  "pickerExtra",
  "providerDir",
  "roots",
  "sensitivePatterns",
] as const;

describe("store fixtures", () => {
  it("emptyPopulated and resetStore enumerate every MockState field", () => {
    const seed = emptyPopulated();
    expect(Object.keys(seed).sort()).toEqual([...MOCK_STATE_KEYS]);
    expect(seed.linkable).toEqual(LINKABLE_ROWS);
    expect(seed.defaultPatterns.length).toBeGreaterThan(0);
    expect(seed.defaultIgnore.length).toBeGreaterThan(0);
    expect(seed.maxFileMb).toBe(50);
    expect(seed.maxSeedFolderMb).toBe(200);

    resetStore({
      providerDir: "/tmp/cloud",
      gitMissing: true,
      loginItem: false,
      roots: [],
      files: {},
      conflicts: {},
      linkable: [],
      entries: {},
      excludes: {},
      pickerExtra: {},
      defaultPatterns: ["only"],
      defaultIgnore: "x",
      maxFileMb: 7,
      maxSeedFolderMb: 8,
    });
    expect(Object.keys(store).sort()).toEqual([...MOCK_STATE_KEYS]);
    expect(store.linkable).toEqual([]);
    expect(store.defaultPatterns).toEqual(["only"]);
    expect(store.defaultIgnore).toBe("x");
    expect(store.maxFileMb).toBe(7);
    expect(store.maxSeedFolderMb).toBe(8);
  });
});

describe("demo fixtures", () => {
  it("demoRoots has linked rows plus a fifth unlinked cloud row", () => {
    const roots = demoRoots();
    expect(roots).toHaveLength(5);
    const linked = roots.filter((row) => row.linked);
    const unlinked = roots.filter((row) => !row.linked);
    expect(linked).toHaveLength(4);
    expect(unlinked).toHaveLength(1);
    expect(unlinked[0]).toEqual({
      slug: LINKABLE_ROWS[0]!.slug,
      path: "",
      name: LINKABLE_ROWS[0]!.display_name,
      is_agent: LINKABLE_ROWS[0]!.is_agent,
      linked: false,
      status: { kind: "Pending" },
    });
    for (const row of linked) {
      expect(row.linked).toBe(true);
      expect(row.path.length).toBeGreaterThan(0);
    }
  });

  it("demoFiles carry bytes and state in each weight class", () => {
    const limit = 50 * 1024 * 1024;
    const records = Object.values(demoFiles()).flatMap((group) =>
      Object.values(group),
    );
    expect(records.length).toBeGreaterThan(0);
    for (const record of records) {
      expect(typeof record.bytes).toBe("number");
      expect(record.state).toMatch(/^(Synced|TooLarge|Pending)$/);
    }
    expect(
      records.some(
        (record) => record.state === "Synced" && (record.bytes ?? 0) < limit / 2,
      ),
    ).toBe(true);
    expect(
      records.some((record) => {
        const bytes = record.bytes ?? 0;
        return (
          record.state !== "TooLarge" &&
          bytes >= limit * 0.55 &&
          bytes <= limit * 0.65
        );
      }),
    ).toBe(true);
    expect(records.some((record) => record.state === "TooLarge")).toBe(true);
  });

  it("emptyPopulated entries cover every demo file key", () => {
    const seed = emptyPopulated();
    for (const [slug, recs] of Object.entries(seed.files)) {
      const entries = seed.entries[slug] ?? [];
      for (const rel of Object.keys(recs)) {
        const covered = entries.some((entry) => {
          if (entry.kind === "file") return entry.key === rel;
          const base = entry.key.replace(/\/$/, "");
          return rel === base || rel.startsWith(`${base}/`);
        });
        expect(covered, `${slug}/${rel}`).toBe(true);
      }
    }
  });
});

describe("mockapp scenarios", () => {
  it("adds unlinked project, include-list editor, and oversized entry", () => {
    const ids = scenarios.map((scenario) => scenario.id);
    expect(ids).toEqual(
      expect.arrayContaining([
        "unlinked-project",
        "include-list-editor",
        "oversized-entry",
        "sensitive-files",
      ]),
    );
    expect(afterMountFor("unlinked-project")).toEqual(expect.any(Function));
    expect(afterMountFor("include-list-editor")).toEqual(expect.any(Function));
    expect(afterMountFor("oversized-entry")).toEqual(expect.any(Function));
    expect(afterMountFor("sensitive-files")).toEqual(expect.any(Function));
  });
});
