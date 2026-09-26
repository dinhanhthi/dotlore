import type { ComponentProps } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { emptyRootsState, RootsContext, type RootsContextValue } from "@/lib/roots";

import { Sidebar } from "./Sidebar";

const { importInstalledAgents, reportError, refreshClicks } = vi.hoisted(() => ({
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
}));

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
