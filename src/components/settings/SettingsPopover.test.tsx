import type { ComponentProps } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { Dialog } from "@/components/ui/dialog";
import { emptyRootsState, RootsContext, type RootsContextValue } from "@/lib/roots";

import { SettingsSeedList } from "./SettingsSeedList";
import {
  SettingsNeverList,
  SettingsPanel,
  SettingsPatterns,
  SettingsSync,
} from "./SettingsPopover";
import { WipeCloudDataAlert } from "./WipeCloudDataAlert";

const { BLOCKED, wipeCloudData, setBanner, clicks } = vi.hoisted(() => ({
  BLOCKED: Symbol("blocked"),
  wipeCloudData: vi.fn(
    async (): Promise<{
      readded: string[];
      failed: { slug: string; error: string }[];
    }> => ({ readded: [], failed: [] }),
  ),
  setBanner: vi.fn(),
  clicks: new Map<
    string,
    (event: { preventDefault: () => void }) => void | Promise<void>
  >(),
}));

vi.mock("@/components/ui/button", async () => {
  const actual = await vi.importActual<typeof import("@/components/ui/button")>(
    "@/components/ui/button",
  );
  return {
    ...actual,
    Button: (props: ComponentProps<typeof actual.Button>) => {
      const key =
        typeof props.children === "string"
          ? props.children
          : props["aria-label"];
      if (typeof key === "string" && props.onClick) {
        clicks.set(
          key,
          props.onClick as (event: {
            preventDefault: () => void;
          }) => void | Promise<void>,
        );
      }
      return actual.Button(props);
    },
  };
});

// The real content renders through a portal, which is empty on the server.
vi.mock("@/components/ui/alert-dialog", async () => {
  const actual = await vi.importActual<
    typeof import("@/components/ui/alert-dialog")
  >("@/components/ui/alert-dialog");
  return {
    ...actual,
    AlertDialogContent: ({ children }: { children?: React.ReactNode }) => (
      <div data-slot="alert-dialog-content">{children}</div>
    ),
  };
});

vi.mock("@/lib/ipc", () => ({
  BLOCKED,
  wipeCloudData,
  setBanner,
  WIPE_LABEL: "Wiping cloud data…",
  subscribeWork: () => () => {},
  getWorkSnapshot: () => ({
    inflight: 0,
    syncing: 0,
    banner: null,
    taskLabel: null,
    trackConfirm: null,
  }),
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

function wrap(
  node: React.ReactNode,
  overrides: Partial<RootsContextValue> = {},
): string {
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

describe("SettingsPanel", () => {
  it("has General, Sync, Patterns, and Never-list tabs, not a popover", () => {
    const html = wrap(
      <Dialog open>
        <SettingsPanel />
      </Dialog>,
    );
    expect(html).toContain("Settings");
    expect(html).toContain("what new folders track");
    expect(html).toContain('role="tablist"');
    expect(html).toMatch(/role="tab"[^>]*>General</);
    expect(html).toMatch(/role="tab"[^>]*>Sync</);
    expect(html).toMatch(/role="tab"[^>]*>Patterns</);
    expect(html).toMatch(/role="tab"[^>]*>Never-list</);
    expect(html).not.toContain('data-slot="popover-content"');
  });

  it("shows only appearance and login on the General tab", () => {
    const html = wrap(
      <Dialog open>
        <SettingsPanel />
      </Dialog>,
    );
    expect(html).toContain("Appearance");
    expect(html).toContain("Start at login");
    expect(html).not.toContain('aria-label="Change cloud folder"');
    expect(html).not.toContain("Max file size");
    expect(html).not.toContain("Preferences");
    expect(html).not.toContain("Size limits");
  });
});

describe("SettingsSync", () => {
  it("holds the cloud folder and the size limits", () => {
    const html = wrap(<SettingsSync onChangeFolder={() => {}} />);
    expect(html).toContain("Cloud folder");
    expect(html).toContain("Wipe cloud data");
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
    expect(html).not.toContain("Default never-list");
    expect(html).toContain("agent folders");
  });
});

function toolbarOf(html: string): string {
  const start = html.indexOf('data-slot="seed-list-toolbar"');
  const end = html.indexOf('data-slot="seed-list-items"');
  expect(start).toBeGreaterThan(-1);
  expect(end).toBeGreaterThan(start);
  return html.slice(start, end);
}

describe("seed list toolbar", () => {
  it("puts the catalog, search, add input and Add on one compact row in Patterns", () => {
    const bar = toolbarOf(wrap(<SettingsPatterns />));
    expect(bar).toContain("Projects");
    expect(bar).toContain('placeholder="Search"');
    expect(bar).toContain('placeholder="Add a pattern"');
    expect(bar).toContain(">Add<");
    expect(bar).not.toContain("h-9");
  });

  it("puts search, add input and Add on one compact row in Never-list", () => {
    const bar = toolbarOf(wrap(<SettingsNeverList />));
    expect(bar).toContain('placeholder="Search"');
    expect(bar).toContain('placeholder="Add an entry"');
    expect(bar).toContain(">Add<");
    expect(bar).not.toContain("h-9");
  });
});

describe("seed list hint", () => {
  function hintBetween(html: string, hint: string) {
    const bar = html.indexOf('data-slot="seed-list-toolbar"');
    const at = html.indexOf(hint);
    const items = html.indexOf('data-slot="seed-list-items"');
    expect(at).toBeGreaterThan(bar);
    expect(at).toBeLessThan(items);
  }

  it("shows the short Patterns hint under the toolbar", () => {
    const html = wrap(<SettingsPatterns />);
    hintBetween(
      html,
      "Applies to folders added from now on. Folders already added keep their list.",
    );
  });

  it("shows the short Never-list hint under the toolbar", () => {
    const html = wrap(<SettingsNeverList />);
    hintBetween(html, "Applies to projects and agent folders added from now on.");
    expect(html).not.toContain("One list for projects and agent folders.");
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

describe("Wipe cloud data", () => {
  beforeEach(() => {
    clicks.clear();
    wipeCloudData.mockReset();
    wipeCloudData.mockResolvedValue({ readded: [], failed: [] });
    setBanner.mockReset();
  });

  async function clickWipe() {
    await clicks.get("Wipe")?.({ preventDefault: () => {} });
    // The action fires `void confirm()`; let it settle.
    await new Promise((resolve) => setTimeout(resolve, 0));
  }

  function renderAlert(overrides: Partial<RootsContextValue> = {}) {
    const onOpenChange = vi.fn();
    const html = wrap(
      <WipeCloudDataAlert open onOpenChange={onOpenChange} />,
      overrides,
    );
    return { html, onOpenChange };
  }

  it("renders an icon-only Wipe button left of the folder button, and the dialog copy", () => {
    const html = wrap(<SettingsSync onChangeFolder={() => {}} />);
    expect(html).not.toContain("Wipe cloud data…");
    expect(clicks.get("Wipe cloud data")).toBeTypeOf("function");
    const wipeAt = html.indexOf('aria-label="Wipe cloud data"');
    const folderAt = html.indexOf('aria-label="Go to location"');
    expect(wipeAt).toBeGreaterThan(-1);
    expect(wipeAt).toBeLessThan(folderAt);

    const { html: alert } = renderAlert();
    expect(alert).toContain("Wipe all synced data?");
    expect(alert).toContain("Files in your projects are not touched.");
    expect(alert).toContain("Cancel");
    expect(alert).toContain("Wipe");
  });

  it("does not wipe just by rendering the dialog", () => {
    const { html } = renderAlert();
    expect(html).toContain('data-slot="alert-dialog-cancel"');
    expect(wipeCloudData).not.toHaveBeenCalled();
  });

  it("invokes wipe_cloud_data once on Wipe, then refreshes and closes", async () => {
    const refreshRoots = vi.fn(async () => {});
    const { onOpenChange } = renderAlert({ refreshRoots });

    await clickWipe();

    expect(wipeCloudData).toHaveBeenCalledOnce();
    expect(refreshRoots).toHaveBeenCalledOnce();
    expect(onOpenChange).toHaveBeenCalledWith(false);
    expect(setBanner).not.toHaveBeenCalled();
  });

  it("closes before the wipe finishes, so the window stays usable", async () => {
    let finish = (_: { readded: string[]; failed: { slug: string; error: string }[] }) => {};
    wipeCloudData.mockReturnValueOnce(
      new Promise((resolve) => {
        finish = resolve;
      }),
    );
    const { onOpenChange } = renderAlert();

    void clicks.get("Wipe")?.({ preventDefault: () => {} });

    expect(onOpenChange).toHaveBeenCalledWith(false);
    expect(wipeCloudData).toHaveBeenCalledOnce();
    finish({ readded: [], failed: [] });
  });

  it("does not refresh when the task slot is taken", async () => {
    wipeCloudData.mockResolvedValueOnce(
      BLOCKED as unknown as {
        readded: string[];
        failed: { slug: string; error: string }[];
      },
    );
    const refreshRoots = vi.fn(async () => {});
    renderAlert({ refreshRoots });

    await clickWipe();

    expect(refreshRoots).not.toHaveBeenCalled();
    expect(setBanner).not.toHaveBeenCalled();
  });

  it("banners the projects that could not be rebuilt", async () => {
    wipeCloudData.mockResolvedValueOnce({
      readded: ["alpha"],
      failed: [
        { slug: "beta", error: "x" },
        { slug: "gamma", error: "y" },
      ],
    });
    renderAlert();

    await clickWipe();

    expect(setBanner).toHaveBeenCalledWith(
      "Could not rebuild: beta, gamma. Add them again from the sidebar.",
    );
  });

  it("closes without crashing when the wipe throws", async () => {
    wipeCloudData.mockRejectedValueOnce(new Error("boom"));
    const refreshRoots = vi.fn(async () => {});
    const { onOpenChange } = renderAlert({ refreshRoots });

    await clickWipe();

    expect(refreshRoots).not.toHaveBeenCalled();
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("hides the button when there is no provider dir", () => {
    const html = wrap(<SettingsSync onChangeFolder={() => {}} />, {
      providerDir: null,
    });
    expect(html).not.toContain('aria-label="Wipe cloud data"');
  });

  it("makes Change an icon-only button right of the folder button", () => {
    const html = wrap(<SettingsSync onChangeFolder={() => {}} />);
    expect(html).not.toContain("Change…");
    expect(clicks.get("Change cloud folder")).toBeTypeOf("function");
    expect(html.indexOf('aria-label="Change cloud folder"')).toBeGreaterThan(
      html.indexOf('aria-label="Go to location"'),
    );
  });
});
