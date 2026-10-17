import type { ReactElement, ReactNode } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { TooltipProvider } from "@/components/ui/tooltip";
import { emptyRootsState, RootsContext, type RootsContextValue } from "@/lib/roots";
import type { DeletedFile, FileContent, HistoryVersion, RestoreResult } from "@/lib/types";

import {
  currentRel,
  HistoryPanel,
  HistoryView,
  relativeTime,
  restoreVersion,
  type HistoryViewProps,
} from "./HistoryPanel";

const { clicks, restoreHistory, BLOCKED } = vi.hoisted(() => ({
  clicks: new Map<string, (event?: unknown) => void>(),
  restoreHistory: vi.fn(),
  BLOCKED: Symbol("blocked"),
}));

vi.mock("@/lib/ipc", () => ({
  BLOCKED,
  fileHistory: vi.fn(),
  deletedFiles: vi.fn(),
  readHistoryFile: vi.fn(),
  restoreHistory,
  listTrash: vi.fn(),
  readTrashFile: vi.fn(),
  restoreTrash: vi.fn(),
  deleteTrash: vi.fn(),
  emptyTrash: vi.fn(),
  deleteTrashOlderThan: vi.fn(),
  trashMaxMb: vi.fn(),
  readFile: vi.fn(),
  reportError: vi.fn(),
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

// Tooltip and dialog content render through portals, which are empty on the server.
// The trigger records its button's click handler by aria-label.
vi.mock("@/components/ui/tooltip", async () => {
  const actual = await vi.importActual<typeof import("@/components/ui/tooltip")>(
    "@/components/ui/tooltip",
  );
  return {
    ...actual,
    TooltipTrigger: (props: Parameters<typeof actual.TooltipTrigger>[0]) => {
      const render = props.render as
        | ReactElement<{ "aria-label"?: string; onClick?: (event?: unknown) => void }>
        | undefined;
      const label = render?.props["aria-label"];
      const onClick = render?.props.onClick;
      if (label !== undefined && onClick) clicks.set(label, onClick);
      return actual.TooltipTrigger(props);
    },
    TooltipContent: ({ children }: { children?: ReactNode }) => (
      <span data-slot="tooltip-content">{children}</span>
    ),
  };
});

vi.mock("@/components/ui/alert-dialog", async () => {
  const actual = await vi.importActual<typeof import("@/components/ui/alert-dialog")>(
    "@/components/ui/alert-dialog",
  );
  return {
    ...actual,
    AlertDialogContent: ({ children }: { children?: ReactNode }) => (
      <div data-slot="alert-dialog-content">{children}</div>
    ),
  };
});

const NOW = new Date(2026, 9, 17, 12, 0, 0);
const nowSecs = Math.floor(NOW.getTime() / 1000);

const versions: HistoryVersion[] = [
  { commit: "c3", time: nowSecs - 120, device_name: "Studio", deleted: true, bytes_len: null },
  {
    commit: "c2",
    time: nowSecs - 3 * 3600,
    device_name: "MacBook Air",
    deleted: false,
    bytes_len: null,
  },
];

const files: DeletedFile[] = [
  { rel: "docs/a.md", commit: "d1", deleted_at: nowSecs - 300, device_name: "Studio" },
  { rel: "notes.md", commit: "d2", deleted_at: nowSecs - 7200, device_name: "MacBook Air" },
];

const text: FileContent = { text: "hello history", binary: false, too_large: false, bytes_len: 13 };

function render(props: Partial<HistoryViewProps> = {}): string {
  clicks.clear();
  const all: HistoryViewProps = {
    rel: "CLAUDE.md",
    versions,
    files: null,
    listError: null,
    onRetry: () => {},
    selected: null,
    onSelect: () => {},
    onOpenHistory: () => {},
    content: null,
    error: null,
    writeDisabled: false,
    onRestore: () => {},
    ask: null,
    onAnswer: () => {},
    onBack: () => {},
    now: NOW,
    ...props,
  };
  return renderToStaticMarkup(
    <TooltipProvider>
      <HistoryView {...all} />
    </TooltipProvider>,
  );
}

function button(html: string, label: string): string {
  const match = html.match(new RegExp(`<button[^>]*aria-label="${label}"[^>]*>`, "i"));
  if (!match) throw new Error(`no button with aria-label="${label}" in:\n${html}`);
  return match[0];
}

const DISABLED = /\sdisabled(=""|\s|>)/;

function ariaLabels(html: string): string[] {
  return [...html.matchAll(/<button[^>]*>/g)].flatMap((tag) => {
    const label = /aria-label="([^"]*)"/.exec(tag[0]);
    return label?.[1] !== undefined ? [label[1]] : [];
  });
}

function tooltipTexts(html: string): Set<string> {
  return new Set(
    [...html.matchAll(/<span data-slot="tooltip-content">([^<]*)<\/span>/g)].map(
      (match) => match[1] ?? "",
    ),
  );
}

function click(label: string) {
  const handler = clicks.get(label);
  if (!handler) throw new Error(`no click handler for "${label}"`);
  handler({ preventDefault: () => {} });
}

describe("relativeTime", () => {
  it("counts minutes and hours back from now", () => {
    expect(relativeTime(nowSecs - 10, NOW)).toBe("Just now");
    expect(relativeTime(nowSecs - 120, NOW)).toBe("2 min ago");
    expect(relativeTime(nowSecs - 3 * 3600, NOW)).toBe("3 h ago");
    expect(relativeTime(nowSecs - 2 * 86400, NOW)).toBe("2 d ago");
  });
});

describe("currentRel", () => {
  it("shows the rel prop, or a deleted file opened in the same project", () => {
    expect(currentRel("A", "CLAUDE.md", { slug: "A", file: "x.md" })).toBe("CLAUDE.md");
    expect(currentRel("A", null, { slug: "A", file: "x.md" })).toBe("x.md");
    expect(currentRel("B", null, { slug: "A", file: "x.md" })).toBeNull();
    expect(currentRel("A", null, null)).toBeNull();
  });
});

describe("HistoryView, one file", () => {
  it("lists versions newest first with device names and a Deleted badge", () => {
    const html = render();
    expect(html).toContain("CLAUDE.md");
    expect(html).toContain("2 min ago");
    expect(html).toContain("3 h ago");
    expect(html).toContain('title="Studio"');
    expect(html).toContain('title="MacBook Air"');
    expect(html).toContain(">Deleted<");
    expect(html.indexOf("Studio")).toBeLessThan(html.indexOf("MacBook Air"));
  });

  it("notes that history is local to this Mac", () => {
    expect(render()).toContain("History is kept on this Mac only.");
    expect(render({ rel: null, versions: null, files })).toContain(
      "History is kept on this Mac only.",
    );
  });

  it("selecting a version calls onSelect with its commit", () => {
    const onSelect = vi.fn();
    render({ onSelect });
    click("3 h ago · MacBook Air");
    expect(onSelect).toHaveBeenCalledWith("c2");
  });

  it("a selected version shows its text preview and an enabled Restore", () => {
    const html = render({ selected: "c2", content: text });
    expect(html).toContain("hello history");
    expect(button(html, "Restore")).not.toMatch(DISABLED);
  });

  it("binary and too-large versions show a notice instead of text", () => {
    expect(
      render({
        selected: "c2",
        content: { text: null, binary: true, too_large: false, bytes_len: 9 },
      }),
    ).toContain("Binary file");
    expect(
      render({
        selected: "c2",
        content: { text: null, binary: false, too_large: true, bytes_len: 9 },
      }),
    ).toContain("File too large to preview");
  });

  it("a deleted version has nothing to preview and Restore disabled", () => {
    const html = render({ selected: "c3" });
    expect(html).toContain("This version deleted the file.");
    expect(button(html, "Restore")).toMatch(DISABLED);
  });

  it("Restore is disabled while another task runs", () => {
    expect(button(render({ selected: "c2", content: text, writeDisabled: true }), "Restore")).toMatch(
      DISABLED,
    );
  });

  it("Restore calls onRestore", () => {
    const onRestore = vi.fn();
    render({ selected: "c2", content: text, onRestore });
    click("Restore");
    expect(onRestore).toHaveBeenCalled();
  });

  it("an existing file opens the dialog, and Replace answers replace", () => {
    const onAnswer = vi.fn();
    const html = render({ selected: "c2", content: text, ask: ["CLAUDE.md"], onAnswer });
    expect(html).toContain("This file already exists in the project");
    click("Replace");
    expect(onAnswer).toHaveBeenCalledWith("replace");
    click("Keep both");
    expect(onAnswer).toHaveBeenCalledWith("keep_both");
  });

  it("empty, loading and error states", () => {
    expect(render({ versions: [] })).toContain("No earlier versions");
    expect(render({ versions: null })).toContain("Loading");
    const failed = render({ versions: null, listError: "boom" });
    expect(failed).toContain("Could not read the history.");
    expect(failed).toContain("boom");
    expect(failed).toContain('aria-label="Try again"');
  });

  it("Back calls onBack", () => {
    const onBack = vi.fn();
    render({ onBack });
    click("Back");
    expect(onBack).toHaveBeenCalled();
  });

  it("renders peer strings as text", () => {
    const html = render({
      versions: [{ ...versions[1]!, device_name: "<b>evil</b>" }],
    });
    expect(html).toContain("&lt;b&gt;evil&lt;/b&gt;");
    expect(html).not.toContain("<b>evil</b>");
  });
});

describe("HistoryView, deleted files", () => {
  it("lists deleted files with time and device", () => {
    const html = render({ rel: null, versions: null, files });
    expect(html).toContain("Deleted files");
    expect(html).toContain('title="docs/a.md"');
    expect(html).toContain('title="notes.md"');
    expect(html).toContain("5 min ago");
    expect(html).toContain("2 h ago");
    expect(html).toContain('title="MacBook Air"');
  });

  it("clicking a row drills into that file", () => {
    const onOpenHistory = vi.fn();
    render({ rel: null, versions: null, files, onOpenHistory });
    click("History of docs/a.md");
    expect(onOpenHistory).toHaveBeenCalledWith("docs/a.md");
  });

  it("empty state", () => {
    expect(render({ rel: null, versions: null, files: [] })).toContain("No deleted files");
  });
});

describe("restoreVersion", () => {
  const target = { slug: "proj", rel: "CLAUDE.md", commit: "c2" };
  const done: RestoreResult = {
    kind: "done",
    restored: [{ from: "CLAUDE.md", to: "CLAUDE.md" }],
    unchanged: [],
    skipped: [],
    tracked: [],
    kept_local: [],
    track_error: null,
  };

  beforeEach(() => {
    restoreHistory.mockReset();
  });

  it("asks on existing, then restores with the chosen answer", async () => {
    const onAsk = vi.fn();
    const setBanner = vi.fn();
    restoreHistory.mockResolvedValueOnce({ kind: "existing", rels: ["CLAUDE.md"] });
    await expect(restoreVersion(target, "ask", onAsk, setBanner)).resolves.toBe(false);
    expect(restoreHistory).toHaveBeenLastCalledWith("proj", "CLAUDE.md", "c2", "ask");
    expect(onAsk).toHaveBeenCalledWith(["CLAUDE.md"]);

    restoreHistory.mockResolvedValueOnce(done);
    await expect(restoreVersion(target, "replace", onAsk, setBanner)).resolves.toBe(true);
    expect(restoreHistory).toHaveBeenLastCalledWith("proj", "CLAUDE.md", "c2", "replace");
    expect(onAsk).toHaveBeenCalledTimes(1);
    expect(setBanner).not.toHaveBeenCalled();
  });

  it("does nothing when blocked", async () => {
    const onAsk = vi.fn();
    restoreHistory.mockResolvedValueOnce(BLOCKED);
    await expect(restoreVersion(target, "ask", onAsk, vi.fn())).resolves.toBe(false);
    expect(onAsk).not.toHaveBeenCalled();
  });

  it("an incomplete restore sets the banner", async () => {
    const setBanner = vi.fn();
    restoreHistory.mockResolvedValueOnce({
      ...done,
      restored: [],
      skipped: ["CLAUDE.md"],
    });
    await restoreVersion(target, "ask", vi.fn(), setBanner);
    expect(setBanner).toHaveBeenCalledWith("Skipped 1: CLAUDE.md");
  });
});

describe("HistoryPanel", () => {
  it("starts loading with a Back button", () => {
    const value: RootsContextValue = {
      ...emptyRootsState,
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
    };
    const html = renderToStaticMarkup(
      <TooltipProvider>
        <RootsContext.Provider value={value}>
          <HistoryPanel slug="proj" rel="CLAUDE.md" onBack={() => {}} />
        </RootsContext.Provider>
      </TooltipProvider>,
    );
    expect(html).toContain("Loading");
    expect(html).toContain('aria-label="Back"');
  });
});

describe("tooltips", () => {
  it("every button has a tooltip matching its aria-label", () => {
    const html =
      render({ selected: "c2", content: text, ask: ["CLAUDE.md"] }) +
      render({ versions: null, listError: "boom" }) +
      render({ rel: null, versions: null, files });
    const labels = ariaLabels(html);
    expect(labels).toEqual(
      expect.arrayContaining([
        "Back",
        "Restore",
        "Try again",
        "Replace",
        "Keep both",
        "Cancel",
        "3 h ago · MacBook Air",
        "History of docs/a.md",
      ]),
    );
    const tips = tooltipTexts(html);
    for (const label of labels) expect(tips).toContain(label);
  });
});
