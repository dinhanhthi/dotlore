import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import type { TreeNode as TreeNodeData } from "@/lib/tree";
import type { EntryView, Sensitivity } from "@/lib/types";

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

function renderNode(path: string, sensitivity: Sensitivity | null = null): string {
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
      sensitivityByRel={new Map([[path, sensitivity]])}
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

describe("TreeNode sensitivity", () => {
  it("shows the warning for a Secret file, immediately before size", () => {
    const html = renderNode("config/credentials.json", "secret");
    expect(html).toContain('aria-label="Sensitive file — may contain secrets"');
    expect(html).not.toContain("text-amber-500");
    expect(html.indexOf('aria-label="Sensitive file')).toBeLessThan(html.indexOf("0 bytes"));
  });

  it("shows a muted hint for a TokenHint file", () => {
    expect(renderNode(".mcp.json", "tokenHint")).toContain(
      'aria-label="May contain API tokens"',
    );
  });

  it("shows a Secret file's name in the warning color", () => {
    const html = renderNode("config/credentials.json", "secret");
    expect(html).toMatch(/<span class="[^"]*text-status-conflict[^"]*">credentials\.json<\/span>/);
  });

  it("keeps the normal name color on a TokenHint file and a plain file", () => {
    expect(renderNode(".mcp.json", "tokenHint")).not.toMatch(/text-status-conflict[^"]*">\.mcp\.json/);
    expect(renderNode("CLAUDE.md")).not.toMatch(/text-status-conflict[^"]*">CLAUDE\.md/);
  });

  it("does not show either icon for a plain file", () => {
    const html = renderNode("CLAUDE.md");
    expect(html).not.toContain('aria-label="Sensitive file');
    expect(html).not.toContain('aria-label="May contain API tokens"');
  });
});
