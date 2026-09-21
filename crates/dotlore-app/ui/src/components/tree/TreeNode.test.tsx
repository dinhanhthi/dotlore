import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import type { TreeNode as TreeNodeData } from "@/lib/tree";
import type { EntryView } from "@/lib/types";

import { TreeNode } from "./TreeNode";

const file = (path: string): TreeNodeData => ({
  name: path.split("/").at(-1) ?? path,
  path,
  kind: "file",
  children: [],
  bytes: 0,
  state: "Synced",
});

const entries: EntryView[] = [
  { key: "docs/", kind: "directory", covering: [] },
  { key: "CLAUDE.md", kind: "file", covering: [] },
];

function renderNode(path: string): string {
  return renderToStaticMarkup(
    <TreeNode
      node={file(path)}
      depth={0}
      selectedRel={null}
      conflictSet={new Set()}
      isOpen={() => false}
      onToggle={() => {}}
      onSelect={() => {}}
      entries={entries}
      onUntrack={() => {}}
      maxFileBytes={50 * 1024 * 1024}
    />,
  );
}

describe("TreeNode untrack", () => {
  it("offers Untrack on an explicit include entry", () => {
    expect(renderNode("CLAUDE.md")).toContain("Untrack");
  });

  it("does not offer Untrack on an inherited-only node", () => {
    expect(renderNode("docs/readme.md")).not.toContain("Untrack");
  });

  it("identifies the covering entry on an inherited-only node", () => {
    expect(renderNode("docs/readme.md")).toContain("Covered by docs/");
  });
});
