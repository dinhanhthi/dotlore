import {
  demoConflicts,
  demoResolution,
} from "../fixtures/conflicts";
import { demoFiles, type FileRecord } from "../fixtures/files";
import {
  GDRIVE_MOUNTS,
  ICLOUD_PROVIDER,
  LINKABLE_SLUGS,
  demoRoots,
} from "../fixtures/roots";
import type { ConflictView, ResolutionDto, RootRow } from "@/lib/types";

export type MockState = {
  providerDir: string | null;
  gitMissing: boolean;
  loginItem: boolean;
  roots: RootRow[];
  files: Record<string, Record<string, FileRecord>>;
  conflicts: Record<string, ConflictView[]>;
  linkable: string[];
};

const ICLOUD = ICLOUD_PROVIDER;
const MOUNTS = GDRIVE_MOUNTS;

function clone<T>(value: T): T {
  return structuredClone(value);
}

export const store: MockState = emptyPopulated();

export function emptyPopulated(): MockState {
  return {
    providerDir: ICLOUD,
    gitMissing: false,
    loginItem: true,
    roots: demoRoots(),
    files: demoFiles(),
    conflicts: demoConflicts(),
    linkable: [...LINKABLE_SLUGS],
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
  const sibling = (store.conflicts[slug] ?? []).find((view) => view.live === rel);
  return demoResolution(
    slug,
    rel,
    record?.text ?? null,
    record?.binary ?? false,
    sibling,
  );
}
