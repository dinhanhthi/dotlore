import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { TooltipProvider } from "@/components/ui/tooltip";
import { emptyRootsState, RootsContext, type RootsContextValue } from "@/lib/roots";
import { buildTree, filterTree } from "@/lib/tree";
import type { ConflictView, RootRow } from "@/lib/types";

import { pickerStateAfterIdentityChange } from "./picker";
import {
  FileTree,
  fileSensitivityMap,
  memoizedScopedViews,
  projectSizeLabel,
  quickResolveIsDisabled,
  quickTargetForScope,
  scopedViews,
  showAgentEmptyHint,
  treeEmptyMessage,
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
    setResolverDirty: () => {},
    showAllProjects: () => {},
    showStarred: () => {},
    showConflicts: () => {},
    showErrors: () => {},
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
      setResolverDirty: () => {},
      showAllProjects: () => {},
      showStarred: () => {},
      showConflicts: () => {},
      showErrors: () => {},
      toggleStar: () => {},
      applyProvider: () => {},
      refreshRoots: async () => {},
      addProject: async () => {},
      inflight: 0,
      busy: false,
      locked: false,
      banner: null,
      commandErrors: [],
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

  it("renders the sensitive-only toggle off and disabled without secret files", () => {
    const toggle = buttonWithLabel(renderTree(unlinked), "Show only sensitive files");
    expect(toggle).toContain('aria-pressed="false"');
    expect(isDisabled(toggle)).toBe(true);
  });

  it("disables Add to track on an unlinked row", () => {
    expect(isDisabled(buttonWithLabel(renderTree(unlinked), "Add to track"))).toBe(
      true,
    );
  });

  it("puts a size filter at the end of the footer", () => {
    const html = renderTree(unlinked);
    expect(html).toContain("Not linked");
    const sensitive = html.indexOf('aria-label="Show only sensitive files"');
    const size = html.indexOf('aria-label="Filter by size"');
    expect(html.indexOf("Not linked")).toBeLessThan(sensitive);
    expect(sensitive).toBeLessThan(size);
    expect(isDisabled(buttonWithLabel(html, "Filter by size"))).toBe(true);
    expect(html).toContain("Size");
  });
});

describe("treeEmptyMessage", () => {
  it("uses the size line only when size is the only filter", () => {
    expect(treeEmptyMessage(false, false, true)).toBe("No files in this size");
    expect(treeEmptyMessage(false, true, false)).toBe("No sensitive files");
    expect(treeEmptyMessage(true, false, true)).toBe("No matches");
    expect(treeEmptyMessage(false, true, true)).toBe("No matches");
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

describe("projectSizeLabel", () => {
  it("counts files for a linked root", () => {
    expect(projectSizeLabel([{ rel: "a", bytes: 2048, state: "Synced", sensitivity: null }], true)).toBe(
      "1 file · 2.0 KB",
    );
  });

  it("says Not linked instead of counting a list that was never loaded", () => {
    expect(projectSizeLabel([], false)).toBe("Not linked");
  });
});

describe("fileSensitivityMap", () => {
  it("keeps the sensitivity of each tracked path for TreeNode", () => {
    expect(fileSensitivityMap([
      { rel: "config/credentials.json", bytes: 4, state: "Synced", sensitivity: "secret" },
      { rel: ".mcp.json", bytes: 2, state: "Synced", sensitivity: "tokenHint" },
    ])).toEqual(new Map([
      ["config/credentials.json", "secret"],
      [".mcp.json", "tokenHint"],
    ]));
  });
});

function conflictView(
  live: string,
  overrides: Partial<ConflictView> = {},
): ConflictView {
  return {
    live,
    sibling: `${live}.sib`,
    loserId8: "11111111",
    loserName: "studio",
    loserIsMe: false,
    ...overrides,
  };
}

function viewsByRel(views: ConflictView[]): Map<string, ConflictView[]> {
  const map = new Map<string, ConflictView[]>();
  for (const view of views) {
    const rel = view.live.replace(/\\/g, "/");
    map.set(rel, [...(map.get(rel) ?? []), view]);
  }
  return map;
}

describe("scopedViews", () => {
  const notes = conflictView("notes/a.md");
  const extra = conflictView("notes-extra/a.md", {
    loserId8: "22222222",
    loserName: "laptop",
    sibling: "notes-extra/a.md.sib",
  });
  const byRel = viewsByRel([notes, extra]);

  it("keeps a file under its folder and leaves a sibling folder prefix out", () => {
    expect(scopedViews(byRel, "notes", "folder")).toEqual([notes]);
    expect(scopedViews(byRel, "notes-extra", "folder")).toEqual([extra]);
    expect(scopedViews(byRel, "notes/a.md", "file")).toEqual([notes]);
  });

  it("returns the same array when the same path is asked again", () => {
    const read = memoizedScopedViews(byRel);
    const folder = read("notes", "folder");
    expect(read("notes", "folder")).toBe(folder);
    expect(read("notes/a.md", "file")).toEqual([notes]);
    expect(read("notes/a.md", "file")).not.toBe(folder);
  });
});

describe("quickTargetForScope", () => {
  const studio = conflictView("notes/a.md", { sibling: "notes/a.studio.md" });
  const laptop = conflictView("notes/b.md", {
    sibling: "notes/b.laptop.md",
    loserId8: "22222222",
    loserName: "laptop",
  });
  const outside = conflictView("notes-extra/a.md", { sibling: "notes-extra/a.studio.md" });
  const mine = conflictView("notes/mine.md", {
    sibling: "notes/mine.me.md",
    loserId8: "aaaaaaaa",
    loserName: "this-mac",
    loserIsMe: true,
  });
  const byRel = viewsByRel([studio, laptop, outside, mine]);

  it("builds a folder batch from the scope and keeps a file scope to one file", () => {
    expect(
      quickTargetForScope(
        byRel,
        { kind: "folder", path: "notes" },
        { deviceId: "11111111" },
        "from-menu",
      ),
    ).toEqual({
      choice: { deviceId: "11111111" },
      device: "from-menu",
      scopeCount: 3,
      files: [
        {
          rel: "notes/a.md",
          keep: "other",
          siblingRel: "notes/a.studio.md",
          views: [studio],
        },
      ],
    });
    expect(
      quickTargetForScope(byRel, { kind: "file", path: "notes/a.md" }, "live"),
    ).toEqual({
      choice: "live",
      scopeCount: 1,
      files: [{ rel: "notes/a.md", keep: "live", views: [studio] }],
    });
    expect(
      quickTargetForScope(byRel, { kind: "file", path: "notes/mine.md" }, "unnamed", "cloud"),
    ).toEqual({
      choice: "unnamed",
      device: "cloud",
      scopeCount: 1,
      files: [{ rel: "notes/mine.md", keep: "live", views: [mine] }],
    });
    expect(
      quickTargetForScope(byRel, { kind: "file", path: "notes/a.md" }, "unnamed", "cloud"),
    ).toBeNull();
  });
});

describe("quickResolveIsDisabled", () => {
  it("is disabled while locked, while a resolver is open, or while a quick target is set", () => {
    expect(quickResolveIsDisabled(false, null, false)).toBe(false);
    expect(quickResolveIsDisabled(true, null, false)).toBe(true);
    expect(quickResolveIsDisabled(false, "notes/a.md", false)).toBe(true);
    expect(quickResolveIsDisabled(false, null, true)).toBe(true);
  });
});

describe("sensitive-only filter", () => {
  it("hides plain files and keeps secret files with their folders", () => {
    const files = [
      { rel: "config/credentials.json", bytes: 4, state: "Synced" as const, sensitivity: "secret" as const },
      { rel: "config/app.json", bytes: 2, state: "Synced" as const, sensitivity: null },
      { rel: ".mcp.json", bytes: 2, state: "Synced" as const, sensitivity: "tokenHint" as const },
      { rel: "notes.md", bytes: 2, state: "Synced" as const, sensitivity: null },
    ];
    const sensitivityByRel = fileSensitivityMap(files);
    const visible = filterTree(
      buildTree(files),
      "",
      (rel) => sensitivityByRel.get(rel) === "secret",
    );
    expect(visible.map((node) => node.path)).toEqual(["config"]);
    expect(visible[0]!.children.map((node) => node.path)).toEqual([
      "config/credentials.json",
    ]);
  });
});
