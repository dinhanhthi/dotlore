import type { ComponentProps } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { emptyRootsState, RootsContext, type RootsContextValue } from "@/lib/roots";
import type { RootRow } from "@/lib/types";

import { Footer } from "./Footer";
import { TitleBarActions } from "./TitleBarActions";

const { clicks } = vi.hoisted(() => ({
  clicks: new Map<string, () => void>(),
}));

vi.mock("@/components/ui/button", async () => {
  const actual = await vi.importActual<typeof import("@/components/ui/button")>(
    "@/components/ui/button",
  );
  return {
    ...actual,
    Button: (props: ComponentProps<typeof actual.Button>) => {
      const label = props["aria-label"];
      if (typeof label === "string" && props.onClick) {
        clicks.set(label, props.onClick as () => void);
      }
      return actual.Button(props);
    },
  };
});

vi.mock("@/components/settings/SettingsPopover", () => ({
  SettingsPopover: () => null,
}));

function row(slug: string, status: RootRow["status"]): RootRow {
  return { slug, path: `/p/${slug}`, name: slug, is_agent: false, linked: true, status };
}

function wrap(node: React.ReactNode, overrides: Partial<RootsContextValue> = {}) {
  const value: RootsContextValue = {
    ...emptyRootsState,
    providerDir: "/cloud",
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
    ...overrides,
  };
  return renderToStaticMarkup(
    <RootsContext.Provider value={value}>{node}</RootsContext.Provider>,
  );
}

describe("conflict entry point", () => {
  beforeEach(() => {
    clicks.clear();
  });

  it("makes the footer conflict status open the Conflicts view", () => {
    const showConflicts = vi.fn();
    const html = wrap(<Footer />, {
      roots: [row("a", { kind: "Conflicts", detail: 2 })],
      showConflicts,
    });
    expect(html).toContain("2 conflicts");
    const open = clicks.get("2 conflicts — show them");
    expect(open).toBeTypeOf("function");
    open?.();
    expect(showConflicts).toHaveBeenCalledOnce();
  });

  it("keeps the footer status plain text when nothing conflicts", () => {
    const html = wrap(<Footer />, { roots: [row("a", { kind: "Synced" })] });
    expect(html).toContain("Synced");
    expect([...clicks.keys()].some((k) => k.includes("show them"))).toBe(false);
  });

  it("no longer puts a conflict button in the title bar", () => {
    wrap(<TitleBarActions />, {
      roots: [row("a", { kind: "Conflicts", detail: 1 })],
    });
    expect([...clicks.keys()].some((k) => k.includes("conflict"))).toBe(false);
  });
});
