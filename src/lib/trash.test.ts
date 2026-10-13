import { describe, expect, it } from "vitest";

import { canRestore, filesUnder, passLabel, statusLabel, trashTree } from "./trash";
import type { TrashFile, TrashProject } from "./types";

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
