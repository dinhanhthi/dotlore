import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { TooltipProvider } from "@/components/ui/tooltip";
import { emptyRootsState, RootsContext, type RootsContextValue } from "@/lib/roots";
import type { RootRow } from "@/lib/types";

import { pickerStateAfterIdentityChange } from "./picker";
import {
  FileTree,
  showAgentEmptyHint,
  treeAwaitingLoad,
  treeDialogsAfterRootChange,
  treeLoadMatches,
} from "./FileTree";

const linked: RootRow = {
  slug: "dotlore",
  path: "/Users/demo/git/dotlore",
  name: "dotlore",
  is_agent: false,
  linked: true,
  status: { kind: "Synced" },
};

const unlinked: RootRow = {
  ...linked,
  path: "",
  linked: false,
  status: { kind: "Pending" },
};

function renderTree(row: RootRow): string {
  const value: RootsContextValue = {
    ...emptyRootsState,
    roots: [row],
    selectedSlug: row.slug,
    selectRoot: () => {},
    selectFile: () => {},
    openResolver: () => {},
    openFirstConflict: () => {},
    closeResolver: () => {},
    showAllProjects: () => {},
    showStarred: () => {},
    showConflicts: () => {},
    toggleStar: () => {},
    applyProvider: () => {},
    refreshRoots: async () => {},
    inflight: 0,
    busy: false,
    locked: false,
    banner: null,
    setBanner: () => {},
    addProject: async () => {},
  };
  return renderToStaticMarkup(
    <TooltipProvider>
      <RootsContext.Provider value={value}>
        <FileTree />
      </RootsContext.Provider>
    </TooltipProvider>,
  );
}

function buttonWithLabel(html: string, label: string): string {
  const match = html.match(
    new RegExp(`<button[^>]*aria-label="${label}"[^>]*>`, "i"),
  );
  if (!match) {
    throw new Error(`no button with aria-label="${label}" in:\n${html}`);
  }
  return match[0];
}

function isDisabled(button: string): boolean {
  return /\sdisabled(?:=|>|\s)/.test(button) || button.includes("data-disabled");
}

describe("FileTree seeding", () => {
  it("shows a loading state in the tree while files are being added", () => {
    const value: RootsContextValue = {
      ...emptyRootsState,
      selectedSlug: "site",
      seeding: [{ slug: "site", path: "/Users/thi/src/site", name: "site" }],
      roots: [
        {
          slug: "site",
          path: "/Users/thi/src/site",
          name: "site",
          is_agent: false,
          linked: true,
          status: { kind: "Pending" },
        },
      ],
      selectRoot: () => {},
      selectFile: () => {},
      openResolver: () => {},
      openFirstConflict: () => {},
      closeResolver: () => {},
      showAllProjects: () => {},
      showStarred: () => {},
      showConflicts: () => {},
      toggleStar: () => {},
      applyProvider: () => {},
      refreshRoots: async () => {},
      addProject: async () => {},
      inflight: 0,
      busy: false,
      locked: false,
      banner: null,
      setBanner: () => {},
    };
    const html = renderToStaticMarkup(
      <TooltipProvider>
        <RootsContext.Provider value={value}>
          <FileTree />
        </RootsContext.Provider>
      </TooltipProvider>,
    );
    expect(html).toContain("Adding files…");
    expect(html).toContain('aria-busy="true"');
    expect(html).not.toContain('aria-label="Add to track"');
  });
});

describe("FileTree dialogs", () => {
  it("closes the picker and clears untrackTarget when selectedSlug or linked changes", () => {
    expect(treeDialogsAfterRootChange()).toEqual({
      pickerOpen: false,
      untrackTarget: null,
      files: [],
      listed: [],
    });
  });

  it("clears staged picker marks when the slug changes or the dialog closes", () => {
    expect(pickerStateAfterIdentityChange()).toEqual({
      pending: {},
      expanded: {},
    });
  });

  it("shows a skeleton while a linked project's files are still loading", () => {
    expect(
      treeAwaitingLoad({ slug: "work", linked: true }, { slug: "personal", linked: true }),
    ).toBe(true);
    expect(treeAwaitingLoad({ slug: "work", linked: true }, null)).toBe(true);
    expect(
      treeAwaitingLoad({ slug: "work", linked: true }, { slug: "work", linked: true }),
    ).toBe(false);
    expect(treeAwaitingLoad({ slug: "work", linked: false }, null)).toBe(false);
  });

  it("drops a tree apply that was started for a different slug", () => {
    expect(
      treeLoadMatches(
        { slug: "personal", linked: true },
        { slug: "work", linked: true },
      ),
    ).toBe(false);
    expect(
      treeLoadMatches(
        { slug: "work", linked: true },
        { slug: "work", linked: true },
      ),
    ).toBe(true);
  });
});

describe("FileTree header", () => {
  it("shows the project name and a file skeleton before a linked tree loads", () => {
    const html = renderTree(linked);
    expect(html).toContain('aria-label="Loading files"');
    expect(html).toContain("dotlore");
    expect(html).not.toContain('aria-label="Add to track"');
  });

  it("disables Add to track on an unlinked row", () => {
    expect(isDisabled(buttonWithLabel(renderTree(unlinked), "Add to track"))).toBe(
      true,
    );
  });
});

describe("FileTree agent empty hint", () => {
  const agent: RootRow = { ...linked, is_agent: true };

  it("shows the hint for a linked agent with no matching files", () => {
    expect(showAgentEmptyHint(agent, 0, false)).toBe(true);
  });

  it("hides the hint for projects, unlinked agents, searches, and agents with files", () => {
    expect(showAgentEmptyHint(linked, 0, false)).toBe(false);
    expect(showAgentEmptyHint({ ...agent, linked: false }, 0, false)).toBe(false);
    expect(showAgentEmptyHint(agent, 0, true)).toBe(false);
    expect(showAgentEmptyHint(agent, 3, false)).toBe(false);
  });
});
