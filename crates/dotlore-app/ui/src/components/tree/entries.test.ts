import { describe, expect, it } from "vitest";

import {
  coveringEntry,
  explicitEntry,
  parentRel,
  untrackCopy,
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
    const text = untrackCopy(claude);
    expect(text).toMatch(/every Mac/);
    expect(text).toMatch(/stay on disk/);
  });

  it("lists remaining coverage and does not promise files stop syncing", () => {
    const text = untrackCopy(readme);
    expect(text).toContain("docs/");
    expect(text.toLowerCase()).not.toContain("stop syncing");
  });

  it("promises files stop syncing only when nothing else covers them", () => {
    expect(untrackCopy(claude).toLowerCase()).toContain("stop syncing");
  });
});
