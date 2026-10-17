import {
  demoConflicts,
  demoResolution,
} from "../fixtures/conflicts";
import {
  SECRET_PATTERNS,
  demoFiles,
  toFileContent,
  type FileRecord,
} from "../fixtures/files";
import {
  GDRIVE_MOUNTS,
  ICLOUD_PROVIDER,
  LINKABLE_ROWS,
  demoRoots,
} from "../fixtures/roots";
import type {
  ConflictView,
  DeletedFile,
  EntryView,
  FileContent,
  HistoryVersion,
  LinkableRow,
  PickerRow,
  ResolutionDto,
  RootRow,
  TrashProject,
  TrashStatus,
} from "@/lib/types";

export const DEFAULT_MAX_FILE_MB = 50;
export const DEFAULT_MAX_SEED_FOLDER_MB = 200;
export const DEFAULT_TRASH_MAX_MB = 500;

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
  trashMaxMb: number;
  trash: TrashProject[];
  /** Trash file previews keyed by `trashKey(slug, day, pass, rel)`. */
  trashContents: Record<string, FileContent>;
  /** Staging history per slug and rel, newest first. */
  history: Record<string, Record<string, HistoryVersion[]>>;
  /** History previews keyed by `historyKey(slug, rel, commit)`; a deleting version has none. */
  historyContents: Record<string, FileContent>;
  deletedFiles: Record<string, DeletedFile[]>;
};

/** The mock state dir; the trash lives under `<APP_HOME>/trash`. */
export const APP_HOME = "/Users/demo/Library/Application Support/dotlore";

export function trashKey(slug: string, day: string, pass: string, rel: string): string {
  return `${slug}/${day}/${pass}/${rel}`;
}

export function historyKey(slug: string, rel: string, commit: string): string {
  return `${slug}/${rel}@${commit}`;
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

const MACBOOK = "MacBook Pro";
const MAC_MINI = "Mac mini";
/** Fixed clock for history times (2026-10-16 14:30 UTC), so the demo stays deterministic. */
const HISTORY_NOW = Date.UTC(2026, 9, 16, 14, 30) / 1000;
const HOUR = 3600;
const DAY = 24 * HOUR;

/** A deterministic 40-hex fake commit id (FNV-1a over `seed`, five rounds, then a final mix). */
function fakeCommit(seed: string): string {
  let out = "";
  for (let round = 0; round < 5; round += 1) {
    let hash = 0x811c9dc5;
    for (const ch of `${round}:${seed}`) {
      hash = Math.imul(hash ^ ch.charCodeAt(0), 0x01000193);
    }
    // Final mix, so seeds that differ in one character look unrelated.
    hash = Math.imul(hash ^ (hash >>> 16), 0x85ebca6b);
    hash = Math.imul(hash ^ (hash >>> 13), 0xc2b2ae35);
    hash ^= hash >>> 16;
    out += (hash >>> 0).toString(16).padStart(8, "0");
  }
  return out;
}

/** An older text: the first half of the lines; a binary keeps its kind with another size. */
function olderContent(content: FileContent, step: number): FileContent {
  if (content.text === null) {
    return { ...content, bytes_len: Math.max(1, content.bytes_len - step * 512) };
  }
  const lines = content.text.split("\n");
  const keep = Math.max(1, Math.ceil(lines.length / (step + 1)));
  return textContent(`${lines.slice(0, keep).join("\n")}\n`);
}

/** One step back in a file's history; `content` null is the version that deleted it. */
type HistorySeed = { ago: number; device: string; content: FileContent | null };

/** Tracked files no longer in the project, each with its versions newest first. */
const DELETED_SEED: Record<string, Record<string, HistorySeed[]>> = {
  dotlore: {
    ".claude/agents/reviewer.md": [
      { ago: 5 * HOUR, device: MAC_MINI, content: null },
      {
        ago: 2 * DAY,
        device: MACBOOK,
        content: textContent("# Reviewer\n\nReview each diff for bugs and missing tests.\n"),
      },
      {
        ago: 6 * DAY,
        device: MAC_MINI,
        content: textContent("# Reviewer\n\nReview each diff.\n"),
      },
    ],
    "docs/plans/old-idea.md": [
      { ago: 3 * DAY, device: MACBOOK, content: null },
      {
        ago: 8 * DAY,
        device: MACBOOK,
        content: textContent("# Old idea\n\nA plan that was dropped.\n"),
      },
    ],
    ".claude/commands/release.md": [
      { ago: 9 * DAY, device: MAC_MINI, content: null },
      {
        ago: 12 * DAY,
        device: MAC_MINI,
        content: textContent("Bump the version, tag it and push.\n"),
      },
    ],
  },
  memlore: {
    ".claude/agents/planner.md": [
      { ago: 1 * DAY, device: MACBOOK, content: null },
      {
        ago: 4 * DAY,
        device: MAC_MINI,
        content: textContent("# Planner\n\nSplit work into phases.\n"),
      },
    ],
    "docs/notes.md": [
      { ago: 7 * DAY, device: MAC_MINI, content: null },
      {
        ago: 10 * DAY,
        device: MACBOOK,
        content: textContent("# Notes\n\n- Try the new index\n"),
      },
    ],
  },
  notes: {
    "drafts/ideas.md": [
      { ago: 2 * HOUR, device: MACBOOK, content: null },
      {
        ago: 1 * DAY,
        device: MACBOOK,
        content: textContent("# Ideas\n\n- A reading list\n- A garden plan\n"),
      },
      {
        ago: 5 * DAY,
        device: MAC_MINI,
        content: textContent("# Ideas\n\n- A reading list\n"),
      },
    ],
    "assets/old-cover.png": [
      { ago: 4 * DAY, device: MAC_MINI, content: null },
      {
        ago: 20 * DAY,
        device: MACBOOK,
        content: { text: null, binary: true, too_large: false, bytes_len: 9_216 },
      },
    ],
  },
};

/** Four versions of a live file: the current content, then older ones; one file was deleted and re-added. */
function liveHistory(slug: string, rel: string, record: FileRecord): HistorySeed[] {
  const current = toFileContent(record);
  const deletedOnce = slug === "dotlore" && rel === "CLAUDE.md";
  return [
    { ago: 3 * HOUR, device: MACBOOK, content: current },
    deletedOnce
      ? { ago: 1 * DAY, device: MAC_MINI, content: null }
      : { ago: 1 * DAY, device: MAC_MINI, content: olderContent(current, 1) },
    { ago: 4 * DAY, device: MAC_MINI, content: olderContent(current, 2) },
    { ago: 11 * DAY, device: MACBOOK, content: olderContent(current, 3) },
  ];
}

function demoHistory(
  files: Record<string, Record<string, FileRecord>>,
): Pick<MockState, "history" | "historyContents" | "deletedFiles"> {
  const history: MockState["history"] = {};
  const historyContents: Record<string, FileContent> = {};
  const deletedFiles: MockState["deletedFiles"] = {};
  const add = (slug: string, rel: string, seeds: HistorySeed[]): HistoryVersion[] => {
    const versions = seeds.map(({ ago, device, content }, index) => {
      const commit = fakeCommit(`${slug}/${rel}/${index}`);
      if (content) historyContents[historyKey(slug, rel, commit)] = { ...content };
      return {
        commit,
        time: HISTORY_NOW - ago,
        device_name: device,
        deleted: content === null,
        bytes_len: null,
      };
    });
    (history[slug] ??= {})[rel] = versions;
    return versions;
  };
  for (const [slug, recs] of Object.entries(files)) {
    for (const [rel, record] of Object.entries(recs)) {
      add(slug, rel, liveHistory(slug, rel, record));
    }
  }
  for (const [slug, rels] of Object.entries(DELETED_SEED)) {
    deletedFiles[slug] = Object.entries(rels)
      .map(([rel, seeds]) => {
        const newest = add(slug, rel, seeds)[0]!;
        return {
          rel,
          commit: newest.commit,
          deleted_at: newest.time,
          device_name: newest.device_name,
        };
      })
      .sort((a, b) => b.deleted_at - a.deleted_at);
  }
  return { history, historyContents, deletedFiles };
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
    trashMaxMb: DEFAULT_TRASH_MAX_MB,
    ...demoTrash(),
    ...demoHistory(files),
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
  store.trashMaxMb =
    next && "trashMaxMb" in next
      ? (next.trashMaxMb ?? DEFAULT_TRASH_MAX_MB)
      : seed.trashMaxMb;
  store.trash = next?.trash ? clone(next.trash) : seed.trash;
  store.trashContents = next?.trashContents
    ? clone(next.trashContents)
    : seed.trashContents;
  store.history = next?.history ? clone(next.history) : seed.history;
  store.historyContents = next?.historyContents
    ? clone(next.historyContents)
    : seed.historyContents;
  store.deletedFiles = next?.deletedFiles ? clone(next.deletedFiles) : seed.deletedFiles;
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
