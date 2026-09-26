import type { ComponentProps } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { emptyRootsState, RootsContext, type RootsContextValue } from "@/lib/roots";
import type { RootRow, UploadState } from "@/lib/types";

import { aggregateStatus, Footer, pollUpload } from "./Footer";
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

vi.mock("@/lib/ipc", async () => {
  const actual = await vi.importActual<typeof import("@/lib/ipc")>("@/lib/ipc");
  return { ...actual, cloudUpload: vi.fn(async () => ({ kind: "Unknown" })) };
});

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
    commandErrors: [],
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

  it("makes the footer error status open the Errors view", () => {
    const showErrors = vi.fn();
    const html = wrap(<Footer />, {
      roots: [row("a", { kind: "Error", detail: "boom" }), row("b", { kind: "Error", detail: "x" })],
      showErrors,
    });
    expect(html).toContain("2 errors");
    const open = clicks.get("2 errors — show details");
    expect(open).toBeTypeOf("function");
    open?.();
    expect(showErrors).toHaveBeenCalledOnce();
  });

  it("counts a command error with no root in error", () => {
    const showErrors = vi.fn();
    const html = wrap(<Footer />, {
      roots: [row("a", { kind: "Synced" })],
      commandErrors: [{ id: 1, message: "wipe failed", at: 0 }],
      showErrors,
    });
    expect(html).toContain("1 error");
    expect(html).not.toContain(">Synced<");
    const open = clicks.get("1 error — show details");
    expect(open).toBeTypeOf("function");
    open?.();
    expect(showErrors).toHaveBeenCalledOnce();
  });

  it("shows a command error when nothing is tracked yet", () => {
    const showErrors = vi.fn();
    const html = wrap(<Footer />, {
      roots: [],
      commandErrors: [{ id: 1, message: "add failed", at: 0 }],
      showErrors,
    });
    expect(html).toContain("1 error");
    expect(html).not.toContain("Nothing tracked");
    clicks.get("1 error — show details")?.();
    expect(showErrors).toHaveBeenCalledOnce();
  });

  it("shows a command error before a cloud folder is set", () => {
    const showErrors = vi.fn();
    const html = wrap(<Footer />, {
      providerDir: null,
      commandErrors: [{ id: 1, message: "provider failed", at: 0 }],
      showErrors,
    });
    expect(html).toContain("1 error");
    expect(html).not.toContain("No cloud folder set");
    clicks.get("1 error — show details")?.();
    expect(showErrors).toHaveBeenCalledOnce();
  });

  it("keeps the footer status plain text when nothing conflicts", () => {
    const html = wrap(<Footer />, { roots: [row("a", { kind: "Synced" })] });
    expect(html).toContain("Synced");
    expect([...clicks.keys()].some((k) => k.includes("show them"))).toBe(false);
  });

  it("ignores cloud-only projects this device has not linked", () => {
    const unlinked: RootRow = { ...row("b", { kind: "Pending" }), path: "", linked: false };
    const html = wrap(<Footer />, { roots: [row("a", { kind: "Synced" }), unlinked] });
    expect(html).toContain("Synced");
    expect(html).not.toContain("Pending");
    expect(html).toContain("1 of 2 roots linked");
  });

  it("shows a plain root count when every root is linked", () => {
    const html = wrap(<Footer />, { roots: [row("a", { kind: "Synced" }), row("b", { kind: "Synced" })] });
    expect(html).toContain("2 roots ·");
    expect(html).not.toContain("linked");
  });

  it("keeps the missing-runtime note off the onboarding footer", () => {
    const html = wrap(<Footer />, {
      providerDir: null,
      error: "no sync runtime is running — pick a provider folder first",
    });
    expect(html).toContain("No cloud folder set");
    expect(html).not.toContain("no sync runtime is running");
  });

  it("still shows a real footer error once a cloud folder is set", () => {
    const html = wrap(<Footer />, {
      providerDir: "/cloud",
      error: "cycle failed",
    });
    expect(html).toContain("cycle failed");
  });

  it("shows a loading state instead of the status while roots are loading", () => {
    const html = wrap(<Footer />, { loadingRoots: true });
    expect(html).toContain("Loading projects…");
    expect(html).not.toContain("Nothing tracked");
    expect(html).toMatch(/<button(?=[^>]*aria-label="Sync now")[^>]*\sdisabled=""/);
  });

  it("shows a spinner while the first cycle has not reported a root yet", () => {
    const html = wrap(<Footer />, { roots: [row("a", { kind: "Checking" })] });
    expect(html).toContain("Syncing…");
    expect(html).toContain("animate-spin");
    expect(html).not.toContain("Pending");
  });

  it("stops the spinner when the cycle failed before reporting", () => {
    const html = wrap(<Footer />, {
      roots: [row("a", { kind: "Checking" })],
      error: "cycle failed",
    });
    expect(html).toContain("Not synced");
    expect(html).not.toContain("Syncing…");
  });

  it("says a root is waiting for the cloud instead of a bare Pending", () => {
    const html = wrap(<Footer />, { roots: [row("a", { kind: "Pending" })] });
    expect(html).toContain("Waiting for cloud files");
    expect(html).not.toContain(">Pending<");
  });

  it("says a root will retry after a live edit", () => {
    const html = wrap(<Footer />, { roots: [row("a", { kind: "Retrying" })] });
    expect(html).toContain("Files changed during sync — retrying");
  });

  it("no longer puts a conflict button in the title bar", () => {
    wrap(<TitleBarActions />, {
      roots: [row("a", { kind: "Conflicts", detail: 1 })],
    });
    expect([...clicks.keys()].some((k) => k.includes("conflict"))).toBe(false);
  });
});

describe("cloud upload status", () => {
  const synced = [row("a", { kind: "Synced" })];

  it("says the provider is still uploading", () => {
    expect(aggregateStatus("/cloud", synced, 0, null, { kind: "Uploading", detail: 3 })).toEqual({
      glyph: "dot",
      color: "bg-status-pending",
      text: "Uploading to cloud…",
    });
  });

  it("says the files reached the cloud", () => {
    expect(aggregateStatus("/cloud", synced, 0, null, { kind: "Uploaded" })).toEqual({
      glyph: "dot",
      color: "bg-status-synced",
      text: "Synced to cloud",
    });
  });

  it("keeps plain Synced when the provider does not say", () => {
    expect(aggregateStatus("/cloud", synced, 0, null, { kind: "Unknown" }).text).toBe("Synced");
  });

  it("does not let an upload state override an earlier status", () => {
    const pending = [row("a", { kind: "Pending" })];
    expect(aggregateStatus("/cloud", pending, 0, null, { kind: "Uploaded" }).text).toBe(
      "Waiting for cloud files",
    );
  });

  it("renders plain Synced before the first upload poll answers", () => {
    const html = wrap(<Footer />, { roots: synced });
    expect(html).toContain(">Synced<");
    expect(html).not.toContain("cloud…");
  });
});

describe("upload poll", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("asks again while the provider is still uploading", async () => {
    const fetch = vi.fn(async (): Promise<UploadState> => ({ kind: "Uploading", detail: 1 }));
    pollUpload(fetch, () => {});
    await vi.advanceTimersByTimeAsync(5000);
    expect(fetch).toHaveBeenCalledTimes(2);
  });

  it("does not start a second probe while the first is still running", async () => {
    const fetch = vi.fn(() => new Promise<UploadState>(() => {}));
    pollUpload(fetch, () => {});
    await vi.advanceTimersByTimeAsync(15000);
    expect(fetch).toHaveBeenCalledOnce();
  });

  it("stops once the files reached the cloud", async () => {
    const onState = vi.fn();
    const fetch = vi.fn(async (): Promise<UploadState> => ({ kind: "Uploaded" }));
    pollUpload(fetch, onState);
    await vi.advanceTimersByTimeAsync(15000);
    expect(fetch).toHaveBeenCalledOnce();
    expect(onState).toHaveBeenCalledWith({ kind: "Uploaded" });
  });

  it("stops when the probe fails", async () => {
    const onState = vi.fn();
    const fetch = vi.fn(async (): Promise<UploadState> => {
      throw new Error("no runtime");
    });
    pollUpload(fetch, onState);
    await vi.advanceTimersByTimeAsync(15000);
    expect(fetch).toHaveBeenCalledOnce();
    expect(onState).toHaveBeenCalledWith({ kind: "Unknown" });
  });

  it("stops after cancel", async () => {
    const onState = vi.fn();
    const fetch = vi.fn(async (): Promise<UploadState> => ({ kind: "Uploading", detail: 1 }));
    const cancel = pollUpload(fetch, onState);
    await vi.advanceTimersByTimeAsync(0);
    cancel();
    await vi.advanceTimersByTimeAsync(15000);
    expect(fetch).toHaveBeenCalledOnce();
    expect(onState).toHaveBeenCalledOnce();
  });
});
