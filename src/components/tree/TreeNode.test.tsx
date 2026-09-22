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
      rootPath="/Users/demo/git/dotlore"
      maxFileBytes={50 * 1024 * 1024}
    />,
  );
}

describe("TreeNode untrack", () => {
  it("does not show an inline Untrack button", () => {
    expect(renderNode("CLAUDE.md")).not.toContain('aria-label="Untrack"');
    expect(renderNode("docs/readme.md")).not.toContain('aria-label="Untrack"');
  });

  it("wraps each row in a context-menu trigger", () => {
    expect(renderNode("CLAUDE.md")).toContain("data-slot=\"context-menu-trigger\"");
    expect(renderNode("docs/readme.md")).toContain("data-slot=\"context-menu-trigger\"");
  });

  it("identifies the covering entry on an inherited-only node", () => {
    expect(renderNode("docs/readme.md")).toContain("Covered by docs/");
  });
});
