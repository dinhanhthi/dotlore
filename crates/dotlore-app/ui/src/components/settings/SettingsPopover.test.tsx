import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

import { emptyRootsState, RootsContext, type RootsContextValue } from "@/lib/roots";

import { SettingsDefaults } from "./SettingsPopover";

vi.mock("@/lib/ipc", () => ({
  defaultPatterns: () => Promise.resolve(["CLAUDE.md", "docs/"]),
  defaultIgnore: () => Promise.resolve("/chats/\n"),
  maxFileMb: () => Promise.resolve(50),
  maxSeedFolderMb: () => Promise.resolve(200),
  setDefaultPatterns: () => Promise.resolve(),
  setDefaultIgnore: () => Promise.resolve(),
  setMaxFileMb: () => Promise.resolve(),
  setMaxSeedFolderMb: () => Promise.resolve(),
}));

function renderDefaults(): string {
  const value: RootsContextValue = {
    ...emptyRootsState,
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
    <RootsContext.Provider value={value}>
      <SettingsDefaults />
    </RootsContext.Provider>,
  );
}

describe("SettingsDefaults", () => {
  it("explains new-project scope, agent folders, and the two size limits", () => {
    const html = renderDefaults();
    expect(html).toMatch(/textarea/i);
    expect(html.match(/<textarea/gi)?.length).toBe(2);
    expect(html).toContain("projects added from now on");
    expect(html).toContain("~/.claude");
    expect(html).toContain("~/.cursor");
    expect(html).toContain("never-list");
    expect(html).toContain("Max file size");
    expect(html).toContain("Max folder size when adding");
    expect(html).toContain("every sync");
    expect(html).toContain("never tracked");
    expect(html).toContain("first added");
    expect(html).toContain("many small files");
  });
});
