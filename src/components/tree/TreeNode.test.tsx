import type { ComponentProps, ReactNode } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { TreeNode as TreeNodeData } from "@/lib/tree";
import type { EntryView, Sensitivity } from "@/lib/types";

import { TreeNode } from "./TreeNode";

type MenuItemProps = { children?: ReactNode; disabled?: boolean; onClick?: () => void };

const { menuItems } = vi.hoisted(() => ({ menuItems: [] as MenuItemProps[] }));

// Renders menu content inline (a closed base-ui menu renders nothing) and
// records each item's props so a test can inspect and click it.
vi.mock("@/components/ui/context-menu", async () => {
  const actual = await vi.importActual<typeof import("@/components/ui/context-menu")>(
    "@/components/ui/context-menu",
  );
  return {
    ...actual,
    ContextMenuContent: ({ children }: { children?: ReactNode }) => <div>{children}</div>,
    ContextMenuItem: (props: MenuItemProps) => {
      menuItems.push(props);
      return <div role="menuitem">{props.children}</div>;
    },
    ContextMenuSeparator: () => <hr />,
  };
});

beforeEach(() => {
  menuItems.length = 0;
});

function markItems(): MenuItemProps[] {
  return menuItems.filter((item) => item.children === "Mark as sensitive");
}

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

function renderNode(
  path: string,
  sensitivity: Sensitivity | null = null,
  extra: Partial<ComponentProps<typeof TreeNode>> = {},
): string {
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
      {...extra}
    />,
  );
}

describe("TreeNode folder", () => {
  it("does not offer expand when a size filter left the folder with no children", () => {
    const html = renderToStaticMarkup(
      <TreeNode
        node={{
          name: "cache",
          path: "cache",
          kind: "folder",
          children: [],
          bytes: 6 * 1024 * 1024,
          state: "Synced",
        }}
        depth={0}
        selectedRel={null}
        conflictSet={new Set()}
        isOpen={() => true}
        onToggle={() => {}}
        onSelect={() => {}}
        entries={entries}
        onUntrack={() => {}}
        rootPath="/Users/demo/git/dotlore"
        maxFileBytes={50 * 1024 * 1024}
        sensitivityByRel={new Map()}
      />,
    );
    expect(html).not.toContain("Expand cache");
    expect(html).not.toContain("Collapse cache");
    expect(html).toContain("cache");
    expect(html).toContain("6.0 MB");
  });
});

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
  it("shows the key mark for a Secret file, immediately before size", () => {
    const html = renderNode("config/credentials.json", "secret");
    expect(html).toContain('aria-label="Sensitive file — may contain secrets"');
    expect(html).toContain("lucide-key-round");
    expect(html).not.toContain("lucide-triangle-alert");
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
    expect(html).toMatch(/<span class="[^"]*text-status-sensitive[^"]*">credentials\.json<\/span>/);
  });

  it("keeps the normal name color on a TokenHint file and a plain file", () => {
    expect(renderNode(".mcp.json", "tokenHint")).not.toMatch(/text-status-sensitive[^"]*">\.mcp\.json/);
    expect(renderNode("CLAUDE.md")).not.toMatch(/text-status-sensitive[^"]*">CLAUDE\.md/);
  });

  it("does not show either icon for a plain file", () => {
    const html = renderNode("CLAUDE.md");
    expect(html).not.toContain('aria-label="Sensitive file');
    expect(html).not.toContain('aria-label="May contain API tokens"');
  });
});

describe("TreeNode scopedViews", () => {
  it("asks for the folder and each open child by path and kind", () => {
    const seen: Array<[string, "file" | "folder"]> = [];
    const html = renderToStaticMarkup(
      <TreeNode
        node={{
          name: "notes",
          path: "notes",
          kind: "folder",
          bytes: 0,
          state: "Synced",
          children: [file("notes/a.md")],
        }}
        depth={0}
        selectedRel={null}
        conflictSet={new Set()}
        isOpen={() => true}
        onToggle={() => {}}
        onSelect={() => {}}
        entries={entries}
        onUntrack={() => {}}
        rootPath="/Users/demo/git/dotlore"
        maxFileBytes={50 * 1024 * 1024}
        sensitivityByRel={new Map()}
        scopedViews={(path, kind) => {
          seen.push([path, kind]);
          return [];
        }}
        onQuickResolve={() => {}}
        quickResolveDisabled={false}
      />,
    );
    expect(seen).toEqual([
      ["notes", "folder"],
      ["notes/a.md", "file"],
    ]);
    expect(html).toContain(">a.md<");
  });
});

describe("TreeNode mark as sensitive", () => {
  it("offers the item on a plain file and calls the handler with its rel", () => {
    const marked: string[] = [];
    renderNode("docs/notes.md", null, { onMarkSensitive: (rel) => marked.push(rel) });
    const items = markItems();
    expect(items).toHaveLength(1);
    expect(items[0].disabled).toBe(false);
    items[0].onClick?.();
    expect(marked).toEqual(["docs/notes.md"]);
  });

  it("offers the item on a TokenHint file", () => {
    renderNode(".mcp.json", "tokenHint", { onMarkSensitive: () => {} });
    expect(markItems()).toHaveLength(1);
  });

  it("does not offer the item on a Secret file", () => {
    renderNode("config/credentials.json", "secret", { onMarkSensitive: () => {} });
    expect(markItems()).toHaveLength(0);
  });

  it("does not offer the item without a handler", () => {
    renderNode("docs/notes.md");
    expect(markItems()).toHaveLength(0);
  });

  it("disables the item when markSensitiveDisabled is set", () => {
    renderNode("docs/notes.md", null, {
      onMarkSensitive: () => {},
      markSensitiveDisabled: true,
    });
    expect(markItems()[0].disabled).toBe(true);
  });

  it("does not offer the item on a folder but does on its open child file", () => {
    const marked: string[] = [];
    renderToStaticMarkup(
      <TreeNode
        node={{
          name: "notes",
          path: "notes",
          kind: "folder",
          bytes: 0,
          state: "Synced",
          children: [file("notes/a.md")],
        }}
        depth={0}
        selectedRel={null}
        conflictSet={new Set()}
        isOpen={() => true}
        onToggle={() => {}}
        onSelect={() => {}}
        entries={entries}
        onUntrack={() => {}}
        rootPath="/Users/demo/git/dotlore"
        maxFileBytes={50 * 1024 * 1024}
        sensitivityByRel={new Map()}
        onMarkSensitive={(rel) => marked.push(rel)}
        markSensitiveDisabled
      />,
    );
    const items = markItems();
    expect(items).toHaveLength(1);
    expect(items[0].disabled).toBe(true);
    items[0].onClick?.();
    expect(marked).toEqual(["notes/a.md"]);
  });
});
