import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it, vi } from "vitest";

import { TooltipProvider } from "@/components/ui/tooltip";
import {
  emptyRootsState,
  RootsContext,
  type RootsContextValue,
} from "@/lib/roots";
import type { FileContent, RootRow } from "@/lib/types";

import { FilePane, FileViewer } from "./FileViewer";

const row: RootRow = {
  slug: "dotlore",
  path: "/Users/demo/git/dotlore",
  name: "dotlore",
  is_agent: false,
  linked: true,
  status: { kind: "Synced" },
};

function renderViewer(): string {
  const value: RootsContextValue = {
    ...emptyRootsState,
    roots: [row],
    selectedSlug: row.slug,
    selectedRel: "CLAUDE.md",
    selectRoot: () => {},
    selectFile: () => {},
    openResolver: () => {},
    openFirstConflict: () => {},
    closeResolver: () => {},
    setResolverDirty: () => {},
    showAllProjects: () => {},
    showStarred: () => {},
    showConflicts: () => {},
    showErrors: () => {},
    showTrash: () => {},
    showHistory: () => {},
    toggleStar: () => {},
    applyProvider: () => {},
    refreshRoots: async () => {},
    inflight: 0,
    busy: false,
    locked: false,
    banner: null,
    commandErrors: [],
    setBanner: () => {},
    addProject: async () => {},
  };
  return renderToStaticMarkup(
    <TooltipProvider>
      <RootsContext.Provider value={value}>
        <FileViewer slug={row.slug} rel="CLAUDE.md" />
      </RootsContext.Provider>
    </TooltipProvider>,
  );
}

function wordWrapButton(html: string): string {
  const match = html.match(/<button[^>]*aria-label="Word wrap"[^>]*>/i);
  if (!match) throw new Error(`no word-wrap button in:\n${html}`);
  return match[0];
}

describe("FileViewer word wrap", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("starts wrapped when no preference is stored", () => {
    expect(wordWrapButton(renderViewer())).toContain('aria-pressed="true"');
  });

  it("keeps an explicit off preference", () => {
    vi.stubGlobal("localStorage", { getItem: () => "0", setItem: () => {} });
    expect(wordWrapButton(renderViewer())).toContain('aria-pressed="false"');
  });
});

function renderPane(content: FileContent | null): string {
  return renderToStaticMarkup(
    <TooltipProvider>
      <FilePane
        rel="CLAUDE.md"
        content={content}
        error={null}
        actions={<button type="button" aria-label="Restore" />}
      />
    </TooltipProvider>,
  );
}

describe("FilePane", () => {
  it("renders the given actions and no Star or Remove controls", () => {
    const html = renderPane(null);
    expect(html).toContain('aria-label="Restore"');
    expect(html).not.toContain('aria-label="Star"');
    expect(html).not.toContain('aria-label="Unstar"');
    expect(html).not.toContain('aria-label="Remove from Dotlore"');
  });

  it("shows too-large and binary messages", () => {
    const base = { text: null, binary: false, too_large: false, bytes_len: 42 };
    expect(renderPane({ ...base, too_large: true })).toContain(
      "File too large to preview (42 bytes)",
    );
    expect(renderPane({ ...base, binary: true })).toContain(
      "Binary file — 42 bytes",
    );
  });
});
