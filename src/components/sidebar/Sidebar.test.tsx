import type { ComponentProps } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { emptyRootsState, RootsContext, type RootsContextValue } from "@/lib/roots";
import type { ConflictView, RootRow } from "@/lib/types";

import {
  Sidebar,
  applyRootMenuViews,
  closeRootMenu,
  keepAllForRow,
  openRootMenu,
  rootKeepDisabled,
  rootKeepTarget,
  type RootMenuSession,
} from "./Sidebar";

const { importInstalledAgents, reportError, refreshClicks, conflicts, menuOpenHandlers } =
  vi.hoisted(() => ({
    importInstalledAgents: vi.fn(
      async (): Promise<{
        added: string[];
        failed: { path: string; message: string }[];
      }> => ({ added: [], failed: [] }),
    ),
    reportError: vi.fn(),
    refreshClicks: [] as Array<
      (event: { nativeEvent: Event }) => void | Promise<void>
    >,
    conflicts: vi.fn(async (): Promise<ConflictView[]> => []),
    menuOpenHandlers: [] as Array<(open: boolean) => void>,
  }));

vi.mock("@/lib/ipc", () => ({
  BLOCKED: Symbol("blocked"),
  importInstalledAgents,
  reportError,
  subscribeWork: () => () => {},
  getWorkSnapshot: () => ({
    inflight: 0,
    syncing: 0,
    banner: null,
    taskLabel: null,
    trackConfirm: null,
    errors: [],
  }),
  linkRoot: vi.fn(),
  recoverRoot: vi.fn(),
  addRoot: vi.fn(),
  removeRoot: vi.fn(),
  conflicts,
}));

vi.mock("@/components/ui/context-menu", async () => {
  const actual = await vi.importActual<typeof import("@/components/ui/context-menu")>(
    "@/components/ui/context-menu",
  );
  return {
    ...actual,
    ContextMenu: (props: ComponentProps<typeof actual.ContextMenu>) => {
      if (props.onOpenChange) {
        menuOpenHandlers.push(props.onOpenChange as (open: boolean) => void);
      }
      return actual.ContextMenu(props);
    },
  };
});

vi.mock("@/components/ui/button", async () => {
  const actual = await vi.importActual<typeof import("@/components/ui/button")>(
    "@/components/ui/button",
  );
  return {
    ...actual,
    Button: (props: ComponentProps<typeof actual.Button>) => {
      if (props["aria-label"] === "Refresh agents" && props.onClick) {
        refreshClicks.push(
          props.onClick as (event: { nativeEvent: Event }) => void | Promise<void>,
        );
      }
      return actual.Button(props);
    },
  };
});

function renderSidebar(overrides: Partial<RootsContextValue> = {}) {
  refreshClicks.length = 0;
  menuOpenHandlers.length = 0;
  conflicts.mockClear();
  const refreshRoots = vi.fn(async () => {});
  const value: RootsContextValue = {
    ...emptyRootsState,
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
    refreshRoots,
    addProject: async () => {},
    inflight: 0,
    busy: false,
    locked: false,
    banner: null,
    commandErrors: [],
    setBanner: () => {},
    ...overrides,
  };
  const html = renderToStaticMarkup(
    <RootsContext.Provider value={value}>
      <Sidebar />
    </RootsContext.Provider>,
  );
  return { html, refreshRoots };
}

function clickRefresh() {
  return refreshClicks.at(-1)?.({ nativeEvent: new Event("click") });
}

/** The attribute, in any order within the tag — not the `disabled:` utility class. */
const REFRESH_DISABLED =
  /<button(?=[^>]*aria-label="Refresh agents")[^>]*\sdisabled=""/;

describe("Sidebar Errors row", () => {
  it("is hidden when nothing is in error", () => {
    const { html } = renderSidebar();
    expect(html).not.toContain(">Errors<");
  });

  it("shows the command-error count", () => {
    const { html } = renderSidebar({
      commandErrors: [
        { id: 1, message: "a", at: 0 },
        { id: 2, message: "b", at: 0 },
      ],
    });
    expect(html).toContain(">Errors<");
    expect(html).toMatch(/>Errors<[\s\S]*?>2</);
    expect(html.indexOf(">Starred<")).toBeLessThan(html.indexOf(">Errors<"));
  });
});

describe("Sidebar agents refresh", () => {
  beforeEach(() => {
    importInstalledAgents.mockReset();
    importInstalledAgents.mockResolvedValue({ added: [], failed: [] });
    reportError.mockReset();
  });

  it("calls importInstalledAgents and then refreshRoots", async () => {
    const { html, refreshRoots } = renderSidebar();

    expect(html).toContain('aria-label="Refresh agents"');
    expect(html).toContain('aria-label="Add agent"');
    expect(html).toContain('aria-label="Add project"');

    expect(refreshClicks.at(-1)).toBeTypeOf("function");
    await clickRefresh();

    expect(importInstalledAgents).toHaveBeenCalledOnce();
    expect(refreshRoots).toHaveBeenCalledOnce();
    expect(importInstalledAgents.mock.invocationCallOrder[0]).toBeLessThan(
      refreshRoots.mock.invocationCallOrder[0] ?? 0,
    );
  });

  it("does not refresh roots when importInstalledAgents throws", async () => {
    importInstalledAgents.mockRejectedValueOnce(new Error("import failed"));
    const { refreshRoots } = renderSidebar();

    await clickRefresh();

    expect(importInstalledAgents).toHaveBeenCalledOnce();
    expect(refreshRoots).not.toHaveBeenCalled();
  });

  it("shows the first import failure and still refreshes roots", async () => {
    importInstalledAgents.mockResolvedValueOnce({
      added: ["home-codex"],
      failed: [{ path: "/Users/x/.claude", message: "already tracked" }],
    });
    const { refreshRoots } = renderSidebar();

    await clickRefresh();

    expect(reportError).toHaveBeenCalledWith("already tracked");
    expect(refreshRoots).toHaveBeenCalledOnce();
  });

  it("disables the refresh button while a write is in flight", () => {
    const { html } = renderSidebar({ locked: true });
    expect(html).toMatch(REFRESH_DISABLED);
  });

  it("leaves the refresh button enabled when nothing is in flight", () => {
    const { html } = renderSidebar({ locked: false });
    expect(html).not.toMatch(REFRESH_DISABLED);
  });

  it("spins and disables the section buttons while roots are loading", () => {
    const { html } = renderSidebar({ loadingRoots: true });
    expect(html).toMatch(REFRESH_DISABLED);
    expect(html).toMatch(
      /<button(?=[^>]*aria-label="Refresh agents")[^>]*aria-busy="true"/,
    );
    expect(html).toMatch(/<button(?=[^>]*aria-label="Add agent")[^>]*\sdisabled=""/);
    expect(html).toMatch(/<button(?=[^>]*aria-label="Add project")[^>]*\sdisabled=""/);
  });
});

describe("Sidebar conflicts entry", () => {
  const conflicted = {
    slug: "home-claude",
    path: "/Users/x/.claude",
    name: ".claude",
    is_agent: true,
    linked: true,
    status: { kind: "Conflicts" as const, detail: 3 },
  };

  it("stays hidden while nothing conflicts", () => {
    const { html } = renderSidebar({ roots: [{ ...conflicted, status: { kind: "Synced" } }] });
    expect(html).not.toContain("Conflicts</span>");
  });

  it("appears with the total once a root conflicts", () => {
    const { html } = renderSidebar({ roots: [conflicted] });
    expect(html).toContain("Conflicts</span>");
    expect(html).toContain('aria-label="3 conflicts"');
  });
});

describe("Sidebar unlinked toggle", () => {
  const row = {
    path: "/Users/x/root",
    status: { kind: "Synced" as const },
  };
  const roots = [
    { ...row, slug: "linked-agent", name: "Linked agent", is_agent: true, linked: true },
    { ...row, slug: "remote-agent", name: "Remote agent", is_agent: true, linked: false },
    { ...row, slug: "linked-project", name: "Linked project", is_agent: false, linked: true },
    { ...row, slug: "remote-project", name: "Remote project", is_agent: false, linked: false },
  ];

  function stubHidden(ids: string[]) {
    vi.stubGlobal("localStorage", {
      getItem: (key: string) =>
        key === "dotlore.sidebar.hideUnlinked" ? JSON.stringify(ids) : null,
      setItem: () => {},
    });
  }

  beforeEach(() => {
    vi.unstubAllGlobals();
  });

  it("shows every root and an unpressed toggle per section by default", () => {
    const { html } = renderSidebar({ roots });
    for (const name of ["Linked agent", "Remote agent", "Linked project", "Remote project"]) {
      expect(html).toContain(name);
    }
    expect(html).toMatch(
      /<button(?=[^>]*aria-label="Hide unlinked agents")[^>]*aria-pressed="false"/,
    );
    expect(html).toMatch(
      /<button(?=[^>]*aria-label="Hide unlinked projects")[^>]*aria-pressed="false"/,
    );
  });

  it("hides unlinked rows only in the section whose toggle is on", () => {
    stubHidden(["agents"]);
    const { html } = renderSidebar({ roots });
    expect(html).not.toContain("Remote agent");
    expect(html).toContain("Linked agent");
    expect(html).toContain("Remote project");
    expect(html).toMatch(
      /<button(?=[^>]*aria-label="Show unlinked agents")[^>]*aria-pressed="true"/,
    );
  });

  it("omits the toggle from a section with no unlinked rows", () => {
    const { html } = renderSidebar({ roots: roots.filter((item) => item.linked) });
    expect(html).not.toContain("unlinked agents");
    expect(html).not.toContain("unlinked projects");
  });
});

function conflictView(live: string, overrides: Partial<ConflictView> = {}): ConflictView {
  return {
    live,
    sibling: `${live}.sib`,
    loserId8: "11111111",
    loserName: "studio",
    loserIsMe: false,
    ...overrides,
  };
}

const linkedConflict: RootRow = {
  slug: "home-claude",
  path: "/Users/x/.claude",
  name: ".claude",
  is_agent: true,
  linked: true,
  status: { kind: "Conflicts", detail: 3 },
};

const unlinkedConflict: RootRow = {
  ...linkedConflict,
  slug: "remote-claude",
  name: "Remote",
  linked: false,
};

describe("root menu fetch", () => {
  const row = { slug: "home-claude", linked: true, conflictCount: 3 };
  const studio = conflictView("notes/a.md", { sibling: "notes/a.studio.md" });

  it("fetches only for a linked row that already has conflicts", () => {
    expect(openRootMenu(null, row, 4)).toEqual({
      session: { slug: "home-claude", token: 4, open: true, views: "loading" },
      fetch: true,
    });
    expect(openRootMenu(null, { ...row, linked: false }, 4)).toEqual({
      session: null,
      fetch: false,
    });
    expect(openRootMenu(null, { ...row, conflictCount: 0 }, 4).fetch).toBe(false);
  });

  it("drops a response after the menu closes or a newer open", () => {
    const session: RootMenuSession = {
      slug: "home-claude",
      token: 1,
      open: true,
      views: "loading",
    };
    const closed = closeRootMenu(session, "home-claude");
    const response = { slug: "home-claude", token: 1, views: [studio] };
    expect(closed?.open).toBe(false);
    expect(applyRootMenuViews(closed, response)).toBe(closed);

    const reopened = openRootMenu(session, row, 2).session;
    expect(applyRootMenuViews(reopened, response)).toBe(reopened);
    expect(applyRootMenuViews(reopened, { ...response, token: 2 })?.views).toEqual([studio]);
  });

  it("omits keep items when the fetch is empty and shows them once views arrive", () => {
    const loading: RootMenuSession = {
      slug: "home-claude",
      token: 1,
      open: true,
      views: "loading",
    };
    expect(keepAllForRow(loading, "home-claude", true)).toBe("loading");
    expect(keepAllForRow({ ...loading, views: [] }, "home-claude", true)).toBeUndefined();
    expect(keepAllForRow({ ...loading, views: [studio] }, "home-claude", true)).toEqual({
      views: [studio],
    });
    expect(keepAllForRow({ ...loading, views: [studio] }, "home-claude", false)).toBeUndefined();
    expect(
      keepAllForRow({ ...loading, open: false, views: [studio] }, "home-claude", true),
    ).toBeUndefined();
  });
});

describe("rootKeepTarget", () => {
  const studio = conflictView("notes/a.md", { sibling: "notes/a.studio.md" });
  const laptopOnA = conflictView("notes/a.md", {
    sibling: "notes/a.laptop.md",
    loserId8: "22222222",
    loserName: "laptop",
  });
  const laptopOnly = conflictView("notes/b.md", {
    sibling: "notes/b.laptop.md",
    loserId8: "22222222",
    loserName: "laptop",
  });
  const mine = conflictView("notes/c.md", {
    sibling: "notes/c.me.md",
    loserId8: "aaaaaaaa",
    loserName: "this-mac",
    loserIsMe: true,
  });
  const views = [studio, laptopOnA, laptopOnly, mine];

  it("builds a root batch for this machine, one device, and the unnamed bucket", () => {
    expect(rootKeepTarget(views, "live")).toEqual({
      choice: "live",
      scopeCount: 3,
      files: [
        { rel: "notes/a.md", keep: "live", views: [studio, laptopOnA] },
        { rel: "notes/b.md", keep: "live", views: [laptopOnly] },
        {
          rel: "notes/c.md",
          keep: "other",
          siblingRel: "notes/c.me.md",
          views: [mine],
        },
      ],
    });
    expect(rootKeepTarget(views, { deviceId: "11111111" })).toEqual({
      choice: { deviceId: "11111111" },
      device: "studio",
      scopeCount: 3,
      files: [
        {
          rel: "notes/a.md",
          keep: "other",
          siblingRel: "notes/a.studio.md",
          views: [studio, laptopOnA],
        },
      ],
    });
    expect(rootKeepTarget(views, "unnamed")).toEqual({
      choice: "unnamed",
      device: "cloud",
      scopeCount: 3,
      files: [{ rel: "notes/c.md", keep: "live", views: [mine] }],
    });
    expect(rootKeepTarget([studio], "unnamed")).toBeNull();
  });
});

describe("rootKeepDisabled", () => {
  it("is disabled while locked, while a resolver is open, while a dialog is open, or while the fetch is pending", () => {
    expect(rootKeepDisabled(false, null, false, false)).toBe(false);
    expect(rootKeepDisabled(true, null, false, false)).toBe(true);
    expect(rootKeepDisabled(false, "notes/a.md", false, false)).toBe(true);
    expect(rootKeepDisabled(false, null, true, false)).toBe(true);
    expect(rootKeepDisabled(false, null, false, true)).toBe(true);
  });
});

describe("Sidebar keep-all", () => {
  const openFirstConflict = vi.fn();

  beforeEach(() => {
    openFirstConflict.mockReset();
    conflicts.mockReset();
    conflicts.mockResolvedValue([]);
  });

  it("keeps the conflict badge, leaves location and remove off the row, and fetches when that menu opens", () => {
    const { html } = renderSidebar({ roots: [linkedConflict], openFirstConflict });

    expect(html).toContain('aria-label="3 conflicts"');
    expect(html).not.toContain('aria-label="Go to location"');
    expect(html).not.toContain('aria-label="Remove"');
    expect(html).not.toContain("Keep all from this machine for");
    expect(menuOpenHandlers).toHaveLength(1);
    menuOpenHandlers[0]?.(true);
    expect(conflicts).toHaveBeenCalledTimes(1);
    expect(conflicts).toHaveBeenCalledWith("home-claude");
    expect(openFirstConflict).not.toHaveBeenCalled();
  });

  it("does not fetch conflicts for an unlinked row", () => {
    renderSidebar({ roots: [unlinkedConflict] });
    expect(menuOpenHandlers).toHaveLength(1);
    menuOpenHandlers[0]?.(true);
    expect(conflicts).not.toHaveBeenCalled();
  });

  it("does not throw when the conflicts fetch fails", async () => {
    conflicts.mockRejectedValueOnce(new Error("offline"));
    renderSidebar({ roots: [linkedConflict] });
    expect(menuOpenHandlers).toHaveLength(1);
    expect(() => menuOpenHandlers[0]?.(true)).not.toThrow();
    await Promise.resolve();
  });
});
