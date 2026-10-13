import {
  demoConflicts,
  demoResolution,
} from "../fixtures/conflicts";
import { SECRET_PATTERNS, demoFiles, type FileRecord } from "../fixtures/files";
import {
  GDRIVE_MOUNTS,
  ICLOUD_PROVIDER,
  LINKABLE_ROWS,
  demoRoots,
} from "../fixtures/roots";
import type {
  ConflictView,
  EntryView,
  FileContent,
  LinkableRow,
  PickerRow,
  ResolutionDto,
  RootRow,
  TrashProject,
  TrashStatus,
} from "@/lib/types";

export const DEFAULT_MAX_FILE_MB = 50;
export const DEFAULT_MAX_SEED_FOLDER_MB = 200;

/** Built-in project seed patterns shown in Settings until the user edits them. */
export const DEFAULT_PATTERNS = [
  "CLAUDE.md",
  "AGENTS.md",
  "docs/",
  ".claude/",
  ".cursor/",
  ".agents/",
];

/** Built-in never-list (gitignore) applied when a project or agent is first added. */
export const DEFAULT_IGNORE = `\
/.credentials.json
/auth.json
/projects/
/sessions/
/chats/
.DS_Store
node_modules/
`;

export type MockState = {
  providerDir: string | null;
  gitMissing: boolean;
  loginItem: boolean;
  roots: RootRow[];
  files: Record<string, Record<string, FileRecord>>;
  conflicts: Record<string, ConflictView[]>;
  linkable: LinkableRow[];
  entries: Record<string, EntryView[]>;
  excludes: Record<string, string[]>;
  pickerExtra: Record<string, Record<string, PickerRow[]>>;
  defaultPatterns: string[];
  /** Per-catalog overrides. A missing key means the mock builtin; `[]` is an override. */
  agentPatterns: Record<string, string[]>;
  defaultIgnore: string;
  /** Secret file patterns (gitignore syntax), seeded from the builtin list. */
  sensitivePatterns: string[];
  maxFileMb: number;
  maxSeedFolderMb: number;
  trash: TrashProject[];
  /** Trash file previews keyed by `trashKey(slug, day, pass, rel)`. */
  trashContents: Record<string, FileContent>;
};

/** The mock state dir; the trash lives under `<APP_HOME>/trash`. */
export const APP_HOME = "/Users/demo/Library/Application Support/dotlore";

export function trashKey(slug: string, day: string, pass: string, rel: string): string {
  return `${slug}/${day}/${pass}/${rel}`;
}

function textContent(text: string): FileContent {
  return {
    text,
    binary: false,
    too_large: false,
    bytes_len: new TextEncoder().encode(text).length,
  };
}

type TrashSeedFile = { rel: string; status: TrashStatus | null; content: FileContent };

type TrashSeed = {
  slug: string;
  root: string | null;
  passes: { day: string; pass: string; files: TrashSeedFile[] }[];
};

/** One live project (`dotlore`, with a `-1` sibling pass) and one removed project. */
const TRASH_SEED: TrashSeed[] = [
  {
    slug: "dotlore",
    root: "/Users/demo/git/dotlore",
    passes: [
      {
        day: "2026-10-13",
        pass: "142501-1",
        files: [
          {
            rel: "assets/logo.png",
            status: "not_in_project",
            content: { text: null, binary: true, too_large: false, bytes_len: 18_432 },
          },
          {
            rel: "docs/notes.md",
            status: "differs",
            content: textContent("# Notes\n\n- Ship the trash view\n- Older wording\n"),
          },
        ],
      },
      {
        day: "2026-10-13",
        pass: "142501",
        files: [
          {
            rel: ".claude/settings.json",
            status: "unknown",
            content: textContent('{\n  "model": "opus"\n}\n'),
          },
          {
            rel: "CLAUDE.md",
            status: "differs",
            content: textContent("# CLAUDE.md\n\nOlder guidance kept by a sync overwrite.\n"),
          },
          {
            rel: "docs/plans/trash-manager/README.md",
            status: "same",
            content: textContent("# Trash manager\n\nBrowse and restore trash copies.\n"),
          },
          {
            rel: "docs/plans/trash-manager/phase-1.md",
            status: "not_in_project",
            content: textContent("# Phase 1\n\nA plan doc another Mac deleted.\n"),
          },
        ],
      },
    ],
  },
  {
    slug: "old-project",
    root: null,
    passes: [
      {
        day: "2026-10-02",
        pass: "091530",
        files: [
          {
            rel: "CLAUDE.md",
            status: null,
            content: textContent("# old-project\n\nA project removed from Dotlore.\n"),
          },
          {
            rel: "data/dump.bin",
            status: null,
            content: { text: null, binary: true, too_large: true, bytes_len: 64_000_000 },
          },
        ],
      },
    ],
  },
];

function demoTrash(): Pick<MockState, "trash" | "trashContents"> {
  const trashContents: Record<string, FileContent> = {};
  const trash = TRASH_SEED.map(({ slug, root, passes }) => ({
    slug,
    root,
    passes: passes.map(({ day, pass, files }) => ({
      day,
      pass,
      path: `${APP_HOME}/trash/${slug}/${day}/${pass}`,
      files: files.map(({ rel, status, content }) => {
        trashContents[trashKey(slug, day, pass, rel)] = { ...content };
        return { rel, bytes: content.bytes_len, status };
      }),
    })),
  }));
  return { trash, trashContents };
}

export function entriesFromFiles(
  files: Record<string, Record<string, FileRecord>>,
): Record<string, EntryView[]> {
  const out: Record<string, EntryView[]> = {};
  for (const [slug, recs] of Object.entries(files)) {
    out[slug] = Object.keys(recs)
      .sort()
      .map((key) => ({ key, kind: "file" as const, covering: [] }));
  }
  return out;
}

/** First-level includes: a file stays a file; nested paths become a folder entry. */
function folderStyleEntries(
  files: Record<string, Record<string, FileRecord>>,
): Record<string, EntryView[]> {
  const out: Record<string, EntryView[]> = {};
  for (const [slug, recs] of Object.entries(files)) {
    const seen = new Map<string, EntryView["kind"]>();
    for (const rel of Object.keys(recs)) {
      const slash = rel.indexOf("/");
      if (slash === -1) seen.set(rel, "file");
      else seen.set(`${rel.slice(0, slash)}/`, "directory");
    }
    out[slug] = [...seen.entries()]
      .sort(([a], [b]) => a.localeCompare(b))
      .map(([key, kind]) => ({ key, kind, covering: [] }));
  }
  return out;
}

const ICLOUD = ICLOUD_PROVIDER;
const MOUNTS = GDRIVE_MOUNTS;

function clone<T>(value: T): T {
  return structuredClone(value);
}

export const store: MockState = emptyPopulated();

export function emptyPopulated(): MockState {
  const files = demoFiles();
  return {
    providerDir: ICLOUD,
    gitMissing: false,
    loginItem: true,
    roots: demoRoots(),
    files,
    conflicts: demoConflicts(),
    linkable: [...LINKABLE_ROWS],
    entries: folderStyleEntries(files),
    excludes: {},
    pickerExtra: {},
    defaultPatterns: [...DEFAULT_PATTERNS],
    agentPatterns: {},
    defaultIgnore: DEFAULT_IGNORE,
    sensitivePatterns: [...SECRET_PATTERNS],
    maxFileMb: DEFAULT_MAX_FILE_MB,
    maxSeedFolderMb: DEFAULT_MAX_SEED_FOLDER_MB,
    ...demoTrash(),
  };
}

export function resetStore(next?: Partial<MockState>): void {
  const seed = emptyPopulated();
  store.providerDir = next && "providerDir" in next ? next.providerDir ?? null : seed.providerDir;
  store.gitMissing = next?.gitMissing ?? seed.gitMissing;
  store.loginItem = next?.loginItem ?? seed.loginItem;
  store.roots = next?.roots ? clone(next.roots) : seed.roots;
  store.files = next?.files ? clone(next.files) : seed.files;
  store.conflicts = next?.conflicts ? clone(next.conflicts) : seed.conflicts;
  store.linkable = next?.linkable ? clone(next.linkable) : seed.linkable;
  store.entries =
    next && "entries" in next
      ? clone(next.entries ?? {})
      : next?.files
        ? entriesFromFiles(next.files)
        : seed.entries;
  store.excludes =
    next && "excludes" in next
      ? clone(next.excludes ?? {})
      : seed.excludes;
  store.pickerExtra =
    next && "pickerExtra" in next
      ? clone(next.pickerExtra ?? {})
      : seed.pickerExtra;
  store.defaultPatterns =
    next && "defaultPatterns" in next
      ? clone(next.defaultPatterns ?? [])
      : seed.defaultPatterns;
  store.agentPatterns =
    next && "agentPatterns" in next
      ? clone(next.agentPatterns ?? {})
      : seed.agentPatterns;
  store.defaultIgnore =
    next && "defaultIgnore" in next
      ? (next.defaultIgnore ?? "")
      : seed.defaultIgnore;
  store.sensitivePatterns =
    next && "sensitivePatterns" in next
      ? clone(next.sensitivePatterns ?? [])
      : seed.sensitivePatterns;
  store.maxFileMb =
    next && "maxFileMb" in next
      ? (next.maxFileMb ?? DEFAULT_MAX_FILE_MB)
      : seed.maxFileMb;
  store.maxSeedFolderMb =
    next && "maxSeedFolderMb" in next
      ? (next.maxSeedFolderMb ?? DEFAULT_MAX_SEED_FOLDER_MB)
      : seed.maxSeedFolderMb;
  store.trash = next?.trash ? clone(next.trash) : seed.trash;
  store.trashContents = next?.trashContents
    ? clone(next.trashContents)
    : seed.trashContents;
}

export function icloudPath(): string {
  return ICLOUD;
}

export function gdriveMounts(): string[] {
  return [...MOUNTS];
}

export function fileRecord(slug: string, rel: string): FileRecord | undefined {
  return store.files[slug]?.[rel];
}

export function syncConflictCount(slug: string): void {
  const row = store.roots.find((item) => item.slug === slug);
  if (!row) return;
  const count = store.conflicts[slug]?.length ?? 0;
  if (row.status.kind === "RootMissing" || row.status.kind === "GitMissing") return;
  row.status = count > 0 ? { kind: "Conflicts", detail: count } : { kind: "Synced" };
}

export function resolutionFor(slug: string, rel: string): ResolutionDto {
  const record = fileRecord(slug, rel);
  const siblings = (store.conflicts[slug] ?? []).filter((view) => view.live === rel);
  return demoResolution(
    slug,
    rel,
    record?.text ?? null,
    record?.binary ?? false,
    siblings,
  );
}
