import { createContext, useContext, useSyncExternalStore } from "react";

import { getWorkSnapshot, subscribeWork } from "./ipc";

import type { LinkableRow, RootRow } from "./types";

export type AppView = "root" | "all" | "starred" | "conflicts";

export type FocusRequest = {
  slug: string;
  seq: number;
};

export type SelectRootOptions = {
  /** Scroll the matching sidebar row into view and expand its section. */
  focusSidebar?: boolean;
};

/** A project whose pattern files are still being copied in. */
export type SeedingRoot = {
  slug: string;
  path: string;
  name: string;
};

/** File count and byte total for one project, from its tracked-file list. */
export type TrackedStats = {
  files: number;
  bytes: number;
};

export const emptyTrackedStats: TrackedStats = { files: 0, bytes: 0 };

export type RootsState = {
  roots: RootRow[];
  providerDir: string | null;
  error: string | null;
  gitMissing: boolean;
  trackedBySlug: Record<string, TrackedStats>;
  starredSlugs: string[];
  selectedSlug: string | null;
  selectedRel: string | null;
  /** When set, the main panel shows the conflict resolver for this path. */
  resolvingRel: string | null;
  view: AppView;
  focusRequest: FocusRequest | null;
  /** Adds in flight. Does not disable the rest of the app. */
  seeding: SeedingRoot[];
};

export type RootsContextValue = RootsState & {
  selectRoot: (slug: string, options?: SelectRootOptions) => void;
  selectFile: (rel: string) => void;
  /** Select a conflicted file and open the inline resolver. */
  openResolver: (slug: string, rel: string) => void;
  /** Open the first conflict of a root, or select the root if it has none. */
  openFirstConflict: (slug: string) => void;
  /** Leave the resolver; `close_resolution` runs on unmount. */
  closeResolver: () => void;
  showAllProjects: () => void;
  showStarred: () => void;
  showConflicts: () => void;
  toggleStar: (slug: string) => void;
  /** Set `providerDir` immediately so onboarding unmounts, then refresh roots. */
  applyProvider: (dir: string) => void;
  /** Re-fetch configured roots, cloud-only rows, and file counts. */
  refreshRoots: () => Promise<void>;
  /** Register `path` without locking the rest of the window. */
  addProject: (path: string, slug: string) => Promise<void>;
  inflight: number;
  busy: boolean;
  /** A write is in flight: the global lock, or a background footer task. */
  locked: boolean;
  banner: string | null;
  setBanner: (message: string | null) => void;
};

export const emptyRootsState: RootsState = {
  roots: [],
  providerDir: null,
  error: null,
  gitMissing: false,
  trackedBySlug: {},
  starredSlugs: [],
  selectedSlug: null,
  selectedRel: null,
  resolvingRel: null,
  view: "root",
  focusRequest: null,
  seeding: [],
};

/** Unresolved conflicts in one root. */
export function conflictCount(row: RootRow): number {
  return row.status.kind === "Conflicts" ? row.status.detail : 0;
}

/** Unresolved conflicts across every root. */
export function conflictTotal(rows: RootRow[]): number {
  return rows.reduce((total, row) => total + conflictCount(row), 0);
}

/** Last path segment, or the slug when the path has none. */
export function folderName(path: string, slug: string): string {
  const name = path.split("/").filter((part) => part.length > 0).at(-1);
  return name && name.length > 0 ? name : slug;
}

/**
 * First component under `/Users/<name>` or `/home/<name>` starts with `.`.
 * Matches `Root::is_agent` closely enough to place an optimistic row.
 */
export function looksLikeAgent(path: string): boolean {
  const parts = path.split("/").filter((part) => part.length > 0);
  const atHome = parts[0] === "Users" || parts[0] === "home";
  const first = atHome ? parts[2] : undefined;
  return first !== undefined && first.startsWith(".");
}

/** Show a linked placeholder for each add that has not landed in `roots` yet. */
export function rootsWithSeeding(roots: RootRow[], seeding: SeedingRoot[]): RootRow[] {
  const pending = seeding.filter(
    (item) => !roots.some((row) => row.slug === item.slug && row.linked),
  );
  if (pending.length === 0) return roots;
  const slugs = new Set(pending.map((item) => item.slug));
  return [
    ...roots.filter((row) => !slugs.has(row.slug)),
    ...pending.map(
      (item): RootRow => ({
        slug: item.slug,
        path: item.path,
        name: item.name,
        is_agent: looksLikeAgent(item.path),
        linked: true,
        status: { kind: "Pending" },
      }),
    ),
  ];
}

/** Cloud-only rows are built so `is_agent` still groups `~/.claude` under Agents. */
export function mergeRootsBySlug(
  local: RootRow[],
  cloud: LinkableRow[],
): RootRow[] {
  const seen = new Set(local.map((row) => row.slug));
  const extras: RootRow[] = [];
  for (const item of cloud) {
    if (seen.has(item.slug)) continue;
    seen.add(item.slug);
    extras.push({
      slug: item.slug,
      name: item.display_name,
      is_agent: item.is_agent,
      path: "",
      linked: false,
      status: { kind: "Pending" },
    });
  }
  return [...local, ...extras];
}

/**
 * Overlay the live cycle statuses onto the discovered rows.
 *
 * An unlinked row keeps its own `Pending`: the last status event for a slug
 * outlives the root it described, so a root that was removed would otherwise
 * carry that staging repo's conflict count on a cloud-only row with no files
 * behind it.
 */
export function overlayStatuses(rows: RootRow[], live: RootRow[]): RootRow[] {
  if (live.length === 0) return rows;
  const bySlug = new Map(live.map((row) => [row.slug, row.status]));
  return rows.map((row) => {
    const status = row.linked ? bySlug.get(row.slug) : undefined;
    return status ? { ...row, status } : row;
  });
}

export type CloudDiscovery =
  | { ok: true; rows: LinkableRow[] }
  | { ok: false; message: string };

export type LocalDiscovery =
  | { ok: true; rows: RootRow[] }
  | { ok: false; message: string };

export type RootDiscovery = {
  roots: RootRow[];
  discoveryError: string | null;
};

function discoveryErrorOf(
  local: LocalDiscovery,
  cloud: CloudDiscovery,
): string | null {
  const messages = [
    local.ok ? null : local.message,
    cloud.ok ? null : cloud.message,
  ].filter((message): message is string => message !== null);
  return messages.length === 0 ? null : messages.join("\n");
}

/** Ignore a list that finished after a newer provider change or refresh started. */
export function applyRootDiscovery(input: {
  generation: number;
  latestGeneration: number;
  local: LocalDiscovery;
  cloud: CloudDiscovery;
  previous: RootRow[];
}): RootDiscovery | null {
  if (input.generation !== input.latestGeneration) return null;
  const localRows = input.local.ok
    ? input.local.rows
    : input.previous.filter((row) => row.linked);
  const cloudRows: LinkableRow[] = input.cloud.ok
    ? input.cloud.rows
    : input.previous
        .filter((row) => !row.linked)
        .map((row) => ({
          slug: row.slug,
          display_name: row.name,
          is_agent: row.is_agent,
        }));
  return {
    roots: mergeRootsBySlug(localRows, cloudRows),
    discoveryError: discoveryErrorOf(input.local, input.cloud),
  };
}

export const RootsContext = createContext<RootsContextValue | null>(null);

export function useRoots(): RootsContextValue {
  const value = useContext(RootsContext);
  if (!value) {
    throw new Error("useRoots must be used within RootsContext");
  }
  return value;
}

/** True while a user-started `syncNow` is still running. */
export function useSyncing(): boolean {
  const count = useSyncExternalStore(
    subscribeWork,
    () => getWorkSnapshot().syncing,
    () => getWorkSnapshot().syncing,
  );
  return count > 0;
}

/** Footer copy while a background task runs. Null when idle. */
export function useTaskLabel(): string | null {
  return useSyncExternalStore(
    subscribeWork,
    () => getWorkSnapshot().taskLabel,
    () => getWorkSnapshot().taskLabel,
  );
}

/** Folder confirmation paused inside a track batch. Null when idle. */
export function useTrackConfirm() {
  return useSyncExternalStore(
    subscribeWork,
    () => getWorkSnapshot().trackConfirm,
    () => getWorkSnapshot().trackConfirm,
  );
}
