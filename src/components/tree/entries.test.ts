import { describe, expect, it } from "vitest";

import {
  coveringEntry,
  explicitEntry,
  parentRel,
  untrackCopy,
  untrackTarget,
} from "./entries";
import type { EntryView } from "@/lib/types";

const docs: EntryView = { key: "docs/", kind: "directory", covering: [] };
const readme: EntryView = {
  key: "docs/readme.md",
  kind: "file",
  covering: ["docs/"],
};
const claude: EntryView = { key: "CLAUDE.md", kind: "file", covering: [] };

describe("explicitEntry", () => {
  it("matches a file node to its include-list key", () => {
    expect(explicitEntry("CLAUDE.md", [claude, docs])).toEqual(claude);
  });

  it("matches a folder node to a trailing-slash directory key", () => {
    expect(explicitEntry("docs", [claude, docs])).toEqual(docs);
  });
});

describe("coveringEntry", () => {
  it("names the covering directory for an inherited-only path", () => {
    expect(coveringEntry("docs/readme.md", [docs])).toEqual(docs);
  });

  it("returns null when the path is itself an explicit entry", () => {
    expect(coveringEntry("docs/readme.md", [docs, readme])).toBeNull();
  });
});

describe("parentRel", () => {
  it("returns null at the picker root so parent navigation can hide", () => {
    expect(parentRel("")).toBeNull();
  });

  it("returns the empty root from a first-level child", () => {
    expect(parentRel("docs")).toBe("");
  });
});

describe("untrackCopy", () => {
  it("says the entry is removed on every Mac and files stay on disk", () => {
    const text = untrackCopy(claude, [claude]);
    expect(text).toMatch(/every Mac/);
    expect(text).toMatch(/stay on disk/);
  });

  it("says an inherited path is excluded from its covering folder", () => {
    const text = untrackCopy(readme, [readme]);
    expect(text).toContain("docs/");
    expect(text.toLowerCase()).toContain("excluded");
    expect(text.toLowerCase()).toContain("stops syncing");
  });

  it("promises files stop syncing only when nothing else covers them", () => {
    expect(untrackCopy(claude, [claude]).toLowerCase()).toContain("stop syncing");
  });
});

describe("untrackTarget", () => {
  it("returns the explicit include entry", () => {
    expect(untrackTarget("CLAUDE.md", "file", [claude, docs])).toEqual(claude);
  });

  it("synthesizes a hole-punch target for an inherited file", () => {
    expect(untrackTarget("docs/readme.md", "file", [docs])).toEqual({
      key: "docs/readme.md",
      kind: "file",
      covering: ["docs/"],
    });
  });

  it("uses a directory key for an inherited folder", () => {
    expect(untrackTarget("docs/secret", "folder", [docs])).toEqual({
      key: "docs/secret/",
      kind: "directory",
      covering: ["docs/"],
    });
  });

  it("targets a folder that only holds explicit entries", () => {
    const json: EntryView = { key: "plugins/a.json", kind: "file", covering: [] };
    expect(untrackTarget("plugins", "folder", [json])).toEqual({
      key: "plugins/",
      kind: "directory",
      covering: [],
    });
  });

  it("describes a folder target as removing the entries under it", () => {
    const json: EntryView = { key: "plugins/a.json", kind: "file", covering: [] };
    const target = untrackTarget("plugins", "folder", [json])!;
    expect(untrackCopy(target, [json])).toContain("Every include entry under plugins/");
  });

  it("returns null for a folder with no entry at or under it", () => {
    expect(untrackTarget("other", "folder", [docs])).toBeNull();
  });
});

