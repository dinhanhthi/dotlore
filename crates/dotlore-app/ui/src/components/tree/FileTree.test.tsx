import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { TooltipProvider } from "@/components/ui/tooltip";
import { emptyRootsState, RootsContext, type RootsContextValue } from "@/lib/roots";
import type { RootRow } from "@/lib/types";

import { pickerStateAfterIdentityChange } from "./EntryPickerDialog";
import { FileTree, treeDialogsAfterRootChange, treeLoadMatches } from "./FileTree";

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
    toggleStar: () => {},
    applyProvider: () => {},
    refreshRoots: async () => {},
    inflight: 0,
    busy: false,
    banner: null,
    setBanner: () => {},
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

describe("FileTree dialogs", () => {
  it("closes the picker and clears untrackTarget when selectedSlug or linked changes", () => {
    expect(treeDialogsAfterRootChange()).toEqual({
      pickerOpen: false,
      untrackTarget: null,
      files: [],
      listed: [],
    });
  });

  it("clears nested picker untrack when the slug changes or the dialog closes", () => {
    expect(pickerStateAfterIdentityChange()).toEqual({
      rel: "",
      children: [],
      selected: null,
      preview: null,
      untrackTarget: null,
    });
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
  it("offers Add to track next to Sync now on a linked project", () => {
    const html = renderTree(linked);
    expect(isDisabled(buttonWithLabel(html, "Add to track"))).toBe(false);
    expect(html).toContain("Add to track");
  });

  it("disables Add to track on an unlinked row", () => {
    expect(isDisabled(buttonWithLabel(renderTree(unlinked), "Add to track"))).toBe(
      true,
    );
  });
});
