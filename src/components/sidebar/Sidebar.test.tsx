import type { ComponentProps } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { emptyRootsState, RootsContext, type RootsContextValue } from "@/lib/roots";

import { Sidebar } from "./Sidebar";

const { importInstalledAgents, setBanner, refreshClicks } = vi.hoisted(() => ({
  importInstalledAgents: vi.fn(
    async (): Promise<{
      added: string[];
      failed: { path: string; message: string }[];
    }> => ({ added: [], failed: [] }),
  ),
  setBanner: vi.fn(),
  refreshClicks: [] as Array<
    (event: { nativeEvent: Event }) => void | Promise<void>
  >,
}));

vi.mock("@/lib/ipc", () => ({
  importInstalledAgents,
  setBanner,
  subscribeWork: () => () => {},
  getWorkSnapshot: () => ({
    inflight: 0,
    syncing: 0,
    banner: null,
    taskLabel: null,
    trackConfirm: null,
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
    showAllProjects: () => {},
    showStarred: () => {},
    showConflicts: () => {},
    toggleStar: () => {},
    applyProvider: () => {},
    refreshRoots,
    addProject: async () => {},
    inflight: 0,
    busy: false,
    locked: false,
    banner: null,
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

describe("Sidebar agents refresh", () => {
  beforeEach(() => {
    importInstalledAgents.mockReset();
    importInstalledAgents.mockResolvedValue({ added: [], failed: [] });
    setBanner.mockReset();
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

    expect(setBanner).toHaveBeenCalledWith("already tracked");
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
