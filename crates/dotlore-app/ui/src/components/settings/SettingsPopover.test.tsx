import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

import { Dialog } from "@/components/ui/dialog";
import { emptyRootsState, RootsContext, type RootsContextValue } from "@/lib/roots";

import { SettingsDefaults, SettingsPanel } from "./SettingsPopover";

vi.mock("@/lib/ipc", () => ({
  defaultPatterns: () => Promise.resolve(["CLAUDE.md", "docs/"]),
  defaultIgnore: () => Promise.resolve("/chats/\n"),
  maxFileMb: () => Promise.resolve(50),
  maxSeedFolderMb: () => Promise.resolve(200),
  setDefaultPatterns: () => Promise.resolve(),
  setDefaultIgnore: () => Promise.resolve(),
  setMaxFileMb: () => Promise.resolve(),
  setMaxSeedFolderMb: () => Promise.resolve(),
  loginItemEnabled: () => Promise.resolve(false),
  setLoginItem: () => Promise.resolve(),
}));

function wrap(node: React.ReactNode): string {
  const value: RootsContextValue = {
    ...emptyRootsState,
    providerDir: "/Users/demo/Library/Mobile Documents/com~apple~CloudDocs",
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
    <RootsContext.Provider value={value}>{node}</RootsContext.Provider>,
  );
}

describe("SettingsPanel", () => {
  it("has General and Patterns tabs, not a popover", () => {
    const html = wrap(
      <Dialog open>
        <SettingsPanel />
      </Dialog>,
    );
    expect(html).toContain("Settings");
    expect(html).toContain('role="tablist"');
    expect(html).toMatch(/role="tab"[^>]*>General</);
    expect(html).toMatch(/role="tab"[^>]*>Patterns</);
    expect(html).not.toContain('data-slot="popover-content"');
  });

  it("shows general config on the General tab", () => {
    const html = wrap(
      <Dialog open>
        <SettingsPanel />
      </Dialog>,
    );
    expect(html).toContain("Cloud folder");
    expect(html).toContain("Appearance");
    expect(html).toContain("Start at login");
    expect(html).toContain("Max file size");
    expect(html).toContain("Max folder size when adding");
    expect(html).toContain("every sync");
    expect(html).toContain("first added");
  });
});

describe("SettingsDefaults", () => {
  it("explains new-project scope, agent folders, and the never-list", () => {
    const html = wrap(<SettingsDefaults />);
    expect(html).toMatch(/textarea/i);
    expect(html.match(/<textarea/gi)?.length).toBe(2);
    expect(html).toContain("projects added from now on");
    expect(html).toContain("~/.claude");
    expect(html).toContain("~/.cursor");
    expect(html).toContain("never-list");
  });
});
