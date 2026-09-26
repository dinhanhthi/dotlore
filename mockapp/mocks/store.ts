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
  LinkableRow,
  PickerRow,
  ResolutionDto,
  RootRow,
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
};

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
