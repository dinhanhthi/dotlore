import type { ReactNode } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

import { TooltipProvider } from "@/components/ui/tooltip";
import { emptyRootsState, RootsContext, type RootsContextValue } from "@/lib/roots";
import type { FileContent, RootRow, TrashProject } from "@/lib/types";

import {
  confirmCopy,
  menuChoices,
  restoreIncomplete,
  restoreNotice,
  TrashView,
  type TrashViewProps,
} from "./TrashPanel";
import { TrashConfirmAlert } from "./TrashConfirmAlert";
import { TrashRestoreDialog } from "./TrashRestoreDialog";
import type { TrashSelection } from "./TrashTree";

vi.mock("@/lib/ipc", () => ({
  BLOCKED: Symbol("blocked"),
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
vi.mock("@/components/ui/tooltip", async () => {
  const actual = await vi.importActual<typeof import("@/components/ui/tooltip")>(
    "@/components/ui/tooltip",
  );
  return {
    ...actual,
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

const NOTICE = "Everything here is on this Mac only and never synced.";

const row: RootRow = {
  slug: "dotlore",
  path: "/Users/demo/git/dotlore",
  name: "Dotlore app",
  is_agent: false,
  linked: true,
  status: { kind: "Synced" },
};

const projects: TrashProject[] = [
  {
    slug: "dotlore",
    root: "/Users/demo/git/dotlore",
    passes: [
      {
        day: "2026-10-13",
        pass: "142501",
        path: "/home/trash/dotlore/2026-10-13/142501",
        files: [
          { rel: "CLAUDE.md", bytes: 12, status: "differs" },
          { rel: "docs/a.md", bytes: 3, status: "not_in_project" },
        ],
      },
    ],
  },
  {
    slug: "gone",
    root: null,
    passes: [
      {
        day: "2026-10-12",
        pass: "090000",
        path: "/home/trash/gone/2026-10-12/090000",
        files: [{ rel: "notes.md", bytes: 5, status: null }],
      },
    ],
  },
];

const text: FileContent = { text: "hello", binary: false, too_large: false, bytes_len: 5 };

function render(props: Partial<TrashViewProps> = {}): string {
  const value: RootsContextValue = {
    ...emptyRootsState,
    roots: [row],
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
    showHistory: () => {},
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
  const all: TrashViewProps = {
    projects,
    selection: null,
    onSelect: () => {},
    content: null,
    error: null,
    listError: null,
    onRetry: () => {},
    writeDisabled: false,
    onRestore: () => {},
    onDelete: () => {},
    onEmpty: () => {},
    capMb: 500,
    ...props,
  };
  return renderToStaticMarkup(
    <TooltipProvider>
      <RootsContext.Provider value={value}>
        <TrashView {...all} />
      </RootsContext.Provider>
    </TooltipProvider>,
  );
}

function button(html: string, label: string): string {
  const match = html.match(new RegExp(`<button[^>]*aria-label="${label}"[^>]*>`, "i"));
  if (!match) throw new Error(`no button with aria-label="${label}" in:\n${html}`);
  return match[0];
}

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

const fileSelection: TrashSelection = {
  slug: "dotlore",
  day: "2026-10-13",
  pass: "142501",
  rel: "CLAUDE.md",
  kind: "file",
};

describe("TrashView", () => {
  it("shows the local-only notice", () => {
    expect(render()).toContain(NOTICE);
    expect(render({ projects: [] })).toContain(NOTICE);
  });

  it("empty state when the trash is empty", () => {
    const html = render({ projects: [] });
    expect(html).toContain("The trash is empty");
    expect(html).toContain("when a sync deletes or overwrites a file");
    expect(button(html, "Clean up")).toMatch(/\sdisabled(=""|\s|>)/);
    expect(button(render(), "Clean up")).not.toMatch(/\sdisabled(=""|\s|>)/);
  });

  it("a failed listing shows an error with a retry, not the empty state", () => {
    const html = render({ projects: null, listError: "Permission denied" });
    expect(html).toContain("Could not read the trash.");
    expect(html).toContain("Permission denied");
    expect(html).toContain('aria-label="Try again"');
    expect(html).not.toContain("The trash is empty");
    expect(html).not.toContain("when a sync deletes or overwrites a file");
    expect(button(html, "Clean up")).toMatch(/\sdisabled(=""|\s|>)/);
  });

  it("removed project offers Delete and Reveal but no Restore", () => {
    const html = render({
      selection: { slug: "gone", day: "2026-10-12", pass: "090000", rel: null, kind: "pass" },
    });
    expect(html).toContain("Project removed");
    expect(html).toContain("This project was removed from Dotlore");
    expect(html).toContain('aria-label="Delete"');
    expect(html).toContain('aria-label="Go to location"');
    expect(html).not.toContain('aria-label="Restore"');
  });

  it("file selection renders FilePane without Star or Remove", () => {
    const html = render({ selection: fileSelection, content: text });
    expect(html).toContain('aria-label="Word wrap"');
    expect(html).toContain('aria-label="Restore"');
    expect(html).toContain('aria-label="Delete"');
    expect(html).toContain('aria-label="Go to location"');
    expect(html).toContain("Differs from project");
    expect(html).toContain("Dotlore app");
    expect(html).not.toContain('aria-label="Star"');
    expect(html).not.toContain('aria-label="Unstar"');
    expect(html).not.toContain('aria-label="Remove from Dotlore"');
  });

  it("shows a summary for a folder selection", () => {
    const html = render({
      selection: { ...fileSelection, rel: "docs", kind: "folder" },
    });
    expect(html).toContain("1 file");
    expect(html).toContain("Oct 13, 2026, 14:25:01");
    expect(html).toContain('aria-label="Restore"');
  });

  it("a search box sits above the tree, and not on an empty trash", () => {
    expect(render()).toContain('aria-label="Search trash"');
    expect(render()).toContain('placeholder="Search trash"');
    expect(render({ projects: [] })).not.toContain('aria-label="Search trash"');
  });

  it("header shows the trash size and the cap", () => {
    expect(render()).toContain("Uses 20 bytes of 500 MB");
    const unknown = render({ capMb: null });
    expect(unknown).toContain("Uses 20 bytes");
    expect(unknown).not.toContain("Uses 20 bytes of");
  });

  it("every button has a tooltip matching its aria-label", () => {
    const html =
      render({ selection: fileSelection, content: text }) +
      render({ projects: null, listError: "Permission denied" }) +
      renderToStaticMarkup(
        <TooltipProvider>
          <TrashRestoreDialog
            open
            rels={["CLAUDE.md"]}
            onCancel={() => {}}
            onReplace={() => {}}
            onKeepBoth={() => {}}
          />
          <TrashConfirmAlert
            open
            onOpenChange={() => {}}
            title="Empty the trash?"
            description="This cannot be undone."
            actionLabel="Empty trash"
            onConfirm={() => {}}
          />
        </TooltipProvider>,
      );
    expect(html).toContain("Empty the trash?");
    expect(ariaLabels(html).filter((label) => label === "Clean up")).toHaveLength(2);
    expect(ariaLabels(html).filter((label) => label === "Empty trash")).toHaveLength(1);
    const labels = ariaLabels(html);
    const tips = tooltipTexts(html);
    expect(labels).toEqual(
      expect.arrayContaining([
        "Clean up",
        "Empty trash",
        "Restore",
        "Delete",
        "Go to location",
        "Word wrap",
        "Cancel",
        "Keep both",
        "Replace",
        "Try again",
      ]),
    );
    for (const label of labels) expect(tips).toContain(label);
  });
});

function pass(day: string, bytes: number[]): TrashProject["passes"][number] {
  return {
    day,
    pass: "120000",
    path: `/home/trash/dotlore/${day}/120000`,
    files: bytes.map((size, i) => ({ rel: `f${i}.md`, bytes: size, status: "same" })),
  };
}

describe("clean up menu", () => {
  const now = new Date(2026, 9, 13, 15, 0, 0);
  const spread: TrashProject[] = [
    {
      slug: "dotlore",
      root: "/Users/demo/git/dotlore",
      passes: [pass("2026-10-13", [1]), pass("2026-08-01", [2, 3]), pass("undated", [4])],
    },
    { slug: "gone", root: null, passes: [pass("2026-06-01", [5])] },
  ];

  it("the empty trash menu lists the age choices and disables those with nothing to delete", () => {
    const choices = menuChoices(spread, now);
    expect(choices.map(({ label, before, disabled }) => ({ label, before, disabled }))).toEqual([
      { label: "Older than 1 month", before: "2026-09-13", disabled: false },
      { label: "Older than 3 months", before: "2026-07-13", disabled: false },
      { label: "Older than 6 months", before: "2026-04-13", disabled: true },
      { label: "Older than 1 year", before: "2025-10-13", disabled: true },
    ]);
    expect(choices[0]?.counts).toEqual({ passes: 2, files: 3, bytes: 10 });
    expect(choices[1]?.counts).toEqual({ passes: 1, files: 1, bytes: 5 });
    expect(menuChoices([], now).every((choice) => choice.disabled)).toBe(true);
  });

  it("an age confirm states how many syncs, files and bytes it deletes", () => {
    const older = confirmCopy(
      {
        kind: "older",
        label: "Older than 3 months",
        before: "2026-07-13",
        counts: { passes: 4, files: 12, bytes: 3355443 },
      },
      spread,
    );
    expect(older.title).toBe("Delete older copies?");
    expect(older.description).toBe(
      "Deletes 4 syncs (12 files, 3.2 MB) older than 3 months from the trash on this Mac. This cannot be undone.",
    );
    expect(older.actionLabel).toBe("Delete");
    const single = confirmCopy(
      {
        kind: "older",
        label: "Older than 1 year",
        before: "2025-10-13",
        counts: { passes: 1, files: 1, bytes: 5 },
      },
      spread,
    );
    expect(single.description).toBe(
      "Deletes 1 sync (1 file, 5 bytes) older than 1 year from the trash on this Mac. This cannot be undone.",
    );
    const everything = confirmCopy({ kind: "empty" }, spread);
    expect(everything.title).toBe("Empty the trash?");
    expect(everything.description).toBe(
      "Deletes all 4 syncs (5 files, 15 bytes) from the trash on this Mac. This cannot be undone.",
    );
    expect(everything.actionLabel).toBe("Empty trash");
  });
});

describe("restoreNotice", () => {
  const done = (
    restored: number,
    unchanged: string[],
    skipped: string[],
  ): Parameters<typeof restoreNotice>[0] => ({
    kind: "done",
    restored: Array.from({ length: restored }, (_, i) => ({ from: `f${i}`, to: `f${i}` })),
    unchanged,
    skipped,
    tracked: [],
    kept_local: [],
    track_error: null,
  });

  it("keep both says a copy it did not add stays on this Mac", () => {
    expect(
      restoreNotice({
        ...done(0, [], []),
        restored: [{ from: "credentials.json", to: "credentials.restored.json" }],
        kept_local: ["credentials.restored.json"],
      }),
    ).toBe(
      "Saved credentials.json as credentials.restored.json. " +
        "Kept credentials.restored.json on this Mac only; track it in the project to sync it.",
    );
  });

  it("restore notice is null for a clean success and mentions skipped files", () => {
    expect(restoreNotice(done(2, [], []))).toBeNull();
    expect(restoreNotice(done(0, ["CLAUDE.md"], []))).toBeNull();
    const notice = restoreNotice(done(1, [], ["docs/a.md", "docs/b.md"]));
    expect(notice).toContain("Restored 1 file.");
    expect(notice).toContain("Skipped 2: docs/a.md, docs/b.md");
    expect(restoreNotice(done(0, [], []))).toBe("Nothing to restore.");
  });

  it("keep both names each restored copy and the copies added to the project", () => {
    const copy = (from: string, to: string) => ({ from, to });
    expect(
      restoreNotice({
        ...done(0, [], []),
        restored: [copy("CLAUDE.md", "CLAUDE.restored.md")],
        tracked: ["CLAUDE.restored.md"],
      }),
    ).toBe(
      "Saved CLAUDE.md as CLAUDE.restored.md. Added CLAUDE.restored.md to the project so it syncs.",
    );
    expect(
      restoreNotice({ ...done(0, [], []), restored: [copy("docs/a.md", "docs/a.restored.md")] }),
    ).toBe("Saved docs/a.md as docs/a.restored.md.");
    const many = ["a", "b", "c", "d"].map((name) => copy(`${name}.md`, `${name}.restored.md`));
    expect(
      restoreNotice({
        ...done(0, [], ["x.md"]),
        restored: [copy("f.md", "f.md"), ...many],
        tracked: ["a.restored.md", "b.restored.md"],
      }),
    ).toBe(
      "Restored 5 files. Saved a.md as a.restored.md, b.md as b.restored.md, c.md as c.restored.md, … " +
        "Added a.restored.md, b.restored.md to the project so they sync. Skipped 1: x.md",
    );
  });

  it("a tracking failure names the written copies and never claims they sync", () => {
    const notice = restoreNotice({
      ...done(0, [], []),
      restored: [{ from: "CLAUDE.md", to: "CLAUDE.restored.md" }],
      track_error: "reading .dotloreproject",
    });
    expect(notice).toBe(
      "Saved CLAUDE.md as CLAUDE.restored.md. " +
        "Could not add CLAUDE.restored.md to the project: reading .dotloreproject",
    );
    expect(notice).not.toContain("so it syncs");
    expect(restoreNotice({ ...done(1, [], []), track_error: "no staging repo" })).toBe(
      "Could not update the project: no staging repo",
    );
  });
});

describe("restoreIncomplete", () => {
  const done = (restored: number, unchanged: string[], skipped: string[]) => ({
    kind: "done" as const,
    restored: Array.from({ length: restored }, (_, i) => ({ from: `f${i}`, to: `f${i}.restored` })),
    unchanged,
    skipped,
    tracked: [],
    kept_local: [],
    track_error: null,
  });

  it("a clean keep both is a success notice, skipped or nothing restored is not", () => {
    expect(restoreIncomplete(done(1, [], []))).toBe(false);
    expect(restoreIncomplete(done(0, ["a.md"], []))).toBe(false);
    expect(restoreIncomplete(done(1, [], ["b.md"]))).toBe(true);
    expect(restoreIncomplete(done(0, [], []))).toBe(true);
  });

  it("a tracking failure is incomplete", () => {
    expect(restoreIncomplete({ ...done(1, [], []), track_error: "boom" })).toBe(true);
  });
});

describe("TrashRestoreDialog", () => {
  it("restore dialog lists at most 10 files and says how many more", () => {
    const rels = Array.from({ length: 12 }, (_, i) => `docs/f${String(i + 1).padStart(2, "0")}.md`);
    const html = renderToStaticMarkup(
      <TooltipProvider>
        <TrashRestoreDialog
          open
          rels={rels}
          onCancel={() => {}}
          onReplace={() => {}}
          onKeepBoth={() => {}}
        />
      </TooltipProvider>,
    );
    expect(html).toContain("Some files already exist in the project");
    expect(html).toContain("docs/f01.md");
    expect(html).toContain("docs/f10.md");
    expect(html).not.toContain("docs/f11.md");
    expect(html).not.toContain("docs/f12.md");
    expect(html).toContain("and 2 more");
    expect(html).toContain(".restored.");
    expect(html).toContain("other Macs");
  });
});
