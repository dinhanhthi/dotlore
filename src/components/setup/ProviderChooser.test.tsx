import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

const { setProvider, icloudDir, listGdriveMounts, pickLocalPath } = vi.hoisted(
  () => ({
    setProvider: vi.fn(),
    icloudDir: vi.fn(),
    listGdriveMounts: vi.fn(),
    pickLocalPath: vi.fn(),
  }),
);

vi.mock("@/lib/ipc", () => ({
  BLOCKED: Symbol("blocked"),
  setProvider,
  icloudDir,
  listGdriveMounts,
}));

vi.mock("@/lib/pick", () => ({ pickLocalPath }));

import { emptyRootsState, RootsContext, type RootsContextValue } from "@/lib/roots";

import { accountDir, APPLY_LABEL, ProviderChooser } from "./ProviderChooser";

function wrap(overrides: Partial<RootsContextValue> = {}): string {
  const value = {
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
    refreshRoots: async () => {},
    inflight: 0,
    busy: false,
    locked: false,
    banner: null,
    setBanner: () => {},
    addProject: async () => {},
    ...overrides,
  } as RootsContextValue;
  return renderToStaticMarkup(
    <RootsContext.Provider value={value}>
      <ProviderChooser />
    </RootsContext.Provider>,
  );
}

describe("ProviderChooser", () => {
  it("offers an apply button that is disabled until a folder is chosen", () => {
    const html = wrap();
    expect(html).toContain(APPLY_LABEL);
    const at = html.indexOf(APPLY_LABEL);
    const button = html.lastIndexOf("<button", at);
    expect(html.slice(button, at)).toContain("disabled");
  });

  /** Picking a provider must not reach the backend; only apply writes. */
  it("writes nothing while the user is only choosing", () => {
    wrap();
    expect(setProvider).not.toHaveBeenCalled();
    expect(icloudDir).not.toHaveBeenCalled();
    expect(pickLocalPath).not.toHaveBeenCalled();
  });
});

describe("accountDir", () => {
  it("syncs through the account's own My Drive root", () => {
    expect(accountDir("/Users/me/Library/CloudStorage/GoogleDrive-a@b.com")).toBe(
      "/Users/me/Library/CloudStorage/GoogleDrive-a@b.com/My Drive",
    );
  });
});
