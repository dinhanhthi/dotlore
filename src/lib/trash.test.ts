import { describe, expect, it } from "vitest";

import {
  canRestore,
  cutoffDay,
  filesUnder,
  filterTrash,
  olderThan,
  passLabel,
  statusLabel,
  trashTotals,
  trashTree,
} from "./trash";
import type { TrashFile, TrashPass, TrashProject } from "./types";

function file(rel: string, bytes = 0, status: TrashFile["status"] = "same"): TrashFile {
  return { rel, bytes, status };
}

describe("passLabel", () => {
  it("formats day and time and numbers -N siblings", () => {
    expect(passLabel("2026-10-13", "142501")).toBe("Oct 13, 2026, 14:25:01");
    expect(passLabel("2026-10-13", "142501-1")).toBe("Oct 13, 2026, 14:25:01 (2)");
    expect(passLabel("2026-01-05", "000000-2")).toBe("Jan 5, 2026, 00:00:00 (3)");
    expect(passLabel("undated", "1760000000")).toBe("Undated");
    expect(passLabel("undated", "1760000000-1")).toBe("Undated (2)");
  });
});

describe("trashTree", () => {
  it("builds folders via buildTree and keeps status by rel", () => {
    const { nodes, statusByRel } = trashTree([
      file("docs/a.md", 3, "differs"),
      file("CLAUDE.md", 5, null),
    ]);
    expect(nodes.map((node) => [node.path, node.kind, node.bytes])).toEqual([
      ["docs", "folder", 3],
      ["CLAUDE.md", "file", 5],
    ]);
    expect(nodes[0]!.children.map((node) => node.path)).toEqual(["docs/a.md"]);
    expect(statusByRel.get("docs/a.md")).toBe("differs");
    expect(statusByRel.get("CLAUDE.md")).toBeNull();
    expect(statusByRel.has("docs")).toBe(false);
  });
});

describe("filesUnder", () => {
  it("lists every file below a folder", () => {
    const files = [
      file("docs/a.md"),
      file("docs/sub/b.md"),
      file("docs2/x.md"),
      file("docs.md"),
    ];
    expect(filesUnder(files, null)).toEqual(files);
    expect(filesUnder(files, "docs").map((f) => f.rel)).toEqual([
      "docs/a.md",
      "docs/sub/b.md",
    ]);
    expect(filesUnder(files, "docs/sub").map((f) => f.rel)).toEqual(["docs/sub/b.md"]);
    expect(filesUnder(files, "docs2/x.md").map((f) => f.rel)).toEqual(["docs2/x.md"]);
  });
});

describe("statusLabel", () => {
  it("maps the four states", () => {
    expect(statusLabel("not_in_project")).toBe("Not in project");
    expect(statusLabel("differs")).toBe("Differs from project");
    expect(statusLabel("same")).toBe("Same as project");
    expect(statusLabel("unknown")).toBe("Cannot compare");
    expect(statusLabel(null)).toBe("Project removed");
  });
});

describe("canRestore", () => {
  it("is false for removed projects", () => {
    const project: TrashProject = { slug: "p", root: "/Users/me/p", passes: [] };
    expect(canRestore(project)).toBe(true);
    expect(canRestore({ ...project, root: null })).toBe(false);
  });
});

describe("cutoffDay", () => {
  it("subtracts months on local calendar fields and clamps to month end", () => {
    expect(cutoffDay(new Date(2026, 9, 13), 1)).toBe("2026-09-13");
    expect(cutoffDay(new Date(2026, 2, 31), 1)).toBe("2026-02-28");
    expect(cutoffDay(new Date(2024, 2, 31), 1)).toBe("2024-02-29");
    expect(cutoffDay(new Date(2026, 0, 15), 3)).toBe("2025-10-15");
    expect(cutoffDay(new Date(2026, 9, 13), 12)).toBe("2025-10-13");
  });
});

function pass(day: string, files: TrashFile[]): TrashPass {
  return { day, pass: "120000", path: `/trash/${day}/120000`, files };
}

const projects: TrashProject[] = [
  {
    slug: "a",
    root: "/Users/me/a",
    passes: [
      pass("2026-08-01", [file("x.md", 10), file("y.md", 20)]),
      pass("2026-09-13", [file("z.md", 5)]),
      pass("undated", [file("u.md", 100)]),
    ],
  },
  {
    slug: "b",
    root: null,
    passes: [pass("2026-09-12", [file("w.md", 1)])],
  },
];

describe("trashTotals", () => {
  it("sums passes, files and bytes over every pass", () => {
    expect(trashTotals(projects)).toEqual({ passes: 4, files: 5, bytes: 136 });
    expect(trashTotals([])).toEqual({ passes: 0, files: 0, bytes: 0 });
  });
});

describe("olderThan", () => {
  it("counts dated passes before the cutoff, not undated or the cutoff day", () => {
    expect(olderThan(projects, "2026-09-13")).toEqual({ passes: 2, files: 3, bytes: 31 });
    expect(olderThan(projects, "2026-08-01")).toEqual({ passes: 0, files: 0, bytes: 0 });
  });
});

describe("filterTrash", () => {
  const nested: TrashProject[] = [
    {
      slug: "a",
      root: "/Users/me/a",
      passes: [
        pass("2026-10-01", [file("docs/Guide/intro.md"), file("docs/other.md"), file("CLAUDE.md")]),
        pass("2026-10-02", [file("notes.md")]),
      ],
    },
    { slug: "b", root: null, passes: [pass("2026-10-03", [file("misc.txt")])] },
  ];

  it("an empty query keeps every project", () => {
    expect(filterTrash(nested, "")).toBe(nested);
    expect(filterTrash(nested, "  ")).toBe(nested);
  });

  it("keeps files whose name or path matches, case-insensitively, and drops empty passes and projects", () => {
    const got = filterTrash(nested, "INTRO");
    expect(got.map((project) => project.slug)).toEqual(["a"]);
    expect(got[0]?.passes.map((item) => item.day)).toEqual(["2026-10-01"]);
    expect(got[0]?.passes[0]?.files.map((item) => item.rel)).toEqual(["docs/Guide/intro.md"]);
  });

  it("a matching folder keeps every file below it", () => {
    const got = filterTrash(nested, "guide");
    expect(got[0]?.passes[0]?.files.map((item) => item.rel)).toEqual(["docs/Guide/intro.md"]);
    const docs = filterTrash(nested, "docs");
    expect(docs[0]?.passes[0]?.files.map((item) => item.rel)).toEqual([
      "docs/Guide/intro.md",
      "docs/other.md",
    ]);
  });

  it("no match leaves no project", () => {
    expect(filterTrash(nested, "nothing-here")).toEqual([]);
  });
});
