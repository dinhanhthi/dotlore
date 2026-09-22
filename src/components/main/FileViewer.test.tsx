import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it, vi } from "vitest";

import { TooltipProvider } from "@/components/ui/tooltip";
import {
  emptyRootsState,
  RootsContext,
  type RootsContextValue,
} from "@/lib/roots";
import type { RootRow } from "@/lib/types";

import { FileViewer } from "./FileViewer";

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
    showAllProjects: () => {},
    showStarred: () => {},
    toggleStar: () => {},
    applyProvider: () => {},
    refreshRoots: async () => {},
    inflight: 0,
    busy: false,
    banner: null,
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
