import type { ComponentProps } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { ErrorLogEntry } from "@/lib/ipc";
import { emptyRootsState, RootsContext, type RootsContextValue } from "@/lib/roots";
import type { RootRow } from "@/lib/types";

import { ErrorsPanel } from "./ErrorsPanel";

const { clicks, dismissError, clearErrors } = vi.hoisted(() => ({
  clicks: new Map<string, Array<() => void>>(),
  dismissError: vi.fn(),
  clearErrors: vi.fn(),
}));

vi.mock("@/lib/ipc", () => ({
  BLOCKED: Symbol("blocked"),
  recoverRoot: vi.fn(),
  syncNow: vi.fn(),
  dismissError,
  clearErrors,
  subscribeWork: () => () => {},
  getWorkSnapshot: () => ({
    inflight: 0,
    syncing: 0,
    banner: null,
    taskLabel: null,
    trackConfirm: null,
    errors: [],
  }),
}));

vi.mock("@/components/ui/button", async () => {
  const actual = await vi.importActual<typeof import("@/components/ui/button")>(
    "@/components/ui/button",
  );
  return {
    ...actual,
    Button: (props: ComponentProps<typeof actual.Button>) => {
      const key =
        props["aria-label"] ?? (typeof props.children === "string" ? props.children : null);
      if (typeof key === "string" && props.onClick) {
        const list = clicks.get(key) ?? [];
        list.push(props.onClick as () => void);
        clicks.set(key, list);
      }
      return actual.Button(props);
    },
  };
});

function row(slug: string, status: RootRow["status"], linked = true): RootRow {
  return { slug, path: `/p/${slug}`, name: slug, is_agent: false, linked, status };
}

function render(roots: RootRow[], commandErrors: ErrorLogEntry[] = []): string {
  clicks.clear();
  const value: RootsContextValue = {
    ...emptyRootsState,
    providerDir: "/cloud",
    roots,
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
    refreshRoots: async () => {},
    inflight: 0,
    busy: false,
    locked: false,
    banner: null,
    commandErrors,
    setBanner: () => {},
    addProject: async () => {},
  };
  return renderToStaticMarkup(
    <RootsContext.Provider value={value}>
      <ErrorsPanel />
    </RootsContext.Provider>,
  );
}

const failures: ErrorLogEntry[] = [
  { id: 7, message: "could not add /p/x: already tracked", at: 0 },
  { id: 3, message: "wipe failed", at: 0 },
];

describe("errors panel", () => {
  beforeEach(() => {
    dismissError.mockReset();
    clearErrors.mockReset();
  });

  it("shows the engine message and Recover for an Error root", () => {
    const html = render([row("a", { kind: "Error", detail: "invalid protection MAC for a" })]);
    expect(html).toContain(">Projects<");
    expect(html).not.toContain("Recent errors");
    expect(html).toContain("invalid protection MAC for a");
    expect(html).toContain(">Recover<");
    expect(html).toContain(">Go to location<");
  });

  it("offers neither Recover nor Go to location for a missing folder", () => {
    const html = render([row("a", { kind: "RootMissing" })]);
    expect(html).toContain("Folder missing");
    expect(html).not.toContain(">Recover<");
    expect(html).not.toContain(">Go to location<");
  });

  it("skips healthy and cloud-only roots", () => {
    const html = render([
      row("a", { kind: "Synced" }),
      row("b", { kind: "Error", detail: "boom" }, false),
    ]);
    expect(html).toContain("No errors.");
  });

  it("lists command errors alone without a Projects section", () => {
    const html = render([row("a", { kind: "Synced" })], failures);
    expect(html).toContain("Recent errors");
    expect(html).toContain("could not add /p/x: already tracked");
    expect(html).toContain("wipe failed");
    expect(html).toContain("font-path");
    expect(html).toContain("select-text");
    expect(html).toContain(">Clear all<");
    expect(html).not.toContain(">Projects<");
    expect(html).not.toContain("No errors.");
  });

  it("shows both sections when both kinds exist", () => {
    const html = render([row("a", { kind: "Error", detail: "boom" })], failures);
    expect(html).toContain(">Projects<");
    expect(html).toContain("Recent errors");
    expect(html.indexOf(">Projects<")).toBeLessThan(html.indexOf("Recent errors"));
  });

  it("says No errors. when both are empty", () => {
    const html = render([], []);
    expect(html).toContain("No errors.");
    expect(html).not.toContain("Recent errors");
    expect(html).not.toContain(">Projects<");
  });

  it("dismisses one entry by id", () => {
    render([], failures);
    const dismiss = clicks.get("Dismiss");
    expect(dismiss).toHaveLength(2);
    dismiss?.[1]?.();
    expect(dismissError).toHaveBeenCalledExactlyOnceWith(3);
  });

  it("clears every entry from the section header", () => {
    render([], failures);
    const clear = clicks.get("Clear all");
    expect(clear).toHaveLength(1);
    clear?.[0]?.();
    expect(clearErrors).toHaveBeenCalledOnce();
  });
});
