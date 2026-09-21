import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

import { Dialog } from "@/components/ui/dialog";
import { emptyRootsState, RootsContext, type RootsContextValue } from "@/lib/roots";

import { SettingsSeedList } from "./SettingsSeedList";
import {
  SettingsNeverList,
  SettingsPanel,
  SettingsPatterns,
} from "./SettingsPopover";

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
  patternCatalogs: () => Promise.resolve([]),
  setPatternCatalog: () => Promise.resolve(),
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
  it("has General, Patterns, and Never-list tabs, not a popover", () => {
    const html = wrap(
      <Dialog open>
        <SettingsPanel />
      </Dialog>,
    );
    expect(html).toContain("Settings");
    expect(html).toContain("what new folders track");
    expect(html).toContain('role="tablist"');
    expect(html).toMatch(/role="tab"[^>]*>General</);
    expect(html).toMatch(/role="tab"[^>]*>Patterns</);
    expect(html).toMatch(/role="tab"[^>]*>Never-list</);
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

describe("SettingsPatterns", () => {
  it("renders a searchable catalog list instead of a textarea", () => {
    const html = wrap(<SettingsPatterns />);
    expect(html).not.toMatch(/textarea/i);
    expect(html).toContain("Search");
    expect(html).toContain("Add a pattern");
    expect(html).toContain("Projects");
  });
});

describe("SettingsNeverList", () => {
  it("renders a searchable list instead of a textarea", () => {
    const html = wrap(<SettingsNeverList />);
    expect(html).not.toMatch(/textarea/i);
    expect(html).toContain("Search");
    expect(html).toContain("Add an entry");
    expect(html).not.toContain("DropdownMenuRadioGroup");
    expect(html).toContain("never-list");
    expect(html).toContain("agent folders");
  });
});

describe("SettingsSeedList", () => {
  it("renders each line with a remove button", () => {
    const html = wrap(
      <SettingsSeedList
        id="seed-lines"
        label="Default patterns"
        hint="Applies the next time a project or agent folder is added. Folders already added stay as they are."
        lines={["CLAUDE.md", "docs/"]}
        disabled={false}
        addPlaceholder="Add a pattern"
        onCommit={() => {}}
      />,
    );
    expect(html).toContain("CLAUDE.md");
    expect(html).toContain("docs/");
    expect(html).toContain('aria-label="Remove CLAUDE.md"');
    expect(html).toContain('aria-label="Remove docs/"');
  });
});
