import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

import { emptyRootsState, RootsContext, type RootsContextValue } from "@/lib/roots";

import { ProjectIgnoreDialog } from "./ProjectIgnoreDialog";

const { projectIgnore, setProjectIgnore } = vi.hoisted(() => ({
  projectIgnore: vi.fn((): Promise<string> => Promise.resolve("/chats/\n")),
  setProjectIgnore: vi.fn(async (): Promise<void> => {}),
}));

// The real content renders through a portal, which is empty on the server.
vi.mock("@/components/ui/dialog", async () => {
  const actual = await vi.importActual<typeof import("@/components/ui/dialog")>(
    "@/components/ui/dialog",
  );
  return {
    ...actual,
    DialogContent: ({ children }: { children?: React.ReactNode }) => (
      <div data-slot="dialog-content">{children}</div>
    ),
  };
});

vi.mock("@/lib/ipc", async () => {
  const actual = await vi.importActual<typeof import("@/lib/ipc")>("@/lib/ipc");
  return { ...actual, projectIgnore, setProjectIgnore };
});

function addInput(html: string): string {
  const tag = html.match(/<input[^>]*id="project-ignore-demo"[^>]*>/)?.[0];
  expect(tag).toBeDefined();
  return tag ?? "";
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
    setResolverDirty: () => {},
    showAllProjects: () => {},
    showStarred: () => {},
    showConflicts: () => {},
    showErrors: () => {},
    showTrash: () => {},
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
    ...overrides,
  };
  return renderToStaticMarkup(
    <RootsContext.Provider value={value}>{node}</RootsContext.Provider>,
  );
}

describe("ProjectIgnoreDialog", () => {
  it("renders the title and hint when open", () => {
    const html = wrap(
      <ProjectIgnoreDialog slug="demo" name="Demo" open onOpenChange={() => {}} />,
    );
    expect(html).toContain("Ignore list — Demo");
    expect(html).toContain(
      "Only for this project, on top of the global never-list. Syncs to your other Macs. Matching files are untracked, not deleted. Start a line with ! to sync a path the never-list ignores (name the ignored entry itself, e.g. !/plans/); sensitive files always stay out.",
    );
    expect(html).toContain('placeholder="Add an entry"');
    expect(addInput(html)).not.toContain('disabled=""');
  });

  it("disables the list while locked", () => {
    const html = wrap(
      <ProjectIgnoreDialog slug="demo" name="Demo" open onOpenChange={() => {}} />,
      { locked: true },
    );
    expect(addInput(html)).toContain('disabled=""');
  });
});
