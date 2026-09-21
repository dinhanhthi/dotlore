import { createContext, useContext } from "react";

import type { LinkableRow, RootRow } from "./types";

export type AppView = "root" | "all" | "starred";

export type FocusRequest = {
  slug: string;
  seq: number;
};

export type SelectRootOptions = {
  /** Scroll the matching sidebar row into view and expand its section. */
  focusSidebar?: boolean;
};

export type RootsState = {
  roots: RootRow[];
  providerDir: string | null;
  error: string | null;
  gitMissing: boolean;
  trackedBySlug: Record<string, number>;
  starredSlugs: string[];
  selectedSlug: string | null;
  selectedRel: string | null;
  /** When set, the main panel shows the conflict resolver for this path. */
  resolvingRel: string | null;
  view: AppView;
  focusRequest: FocusRequest | null;
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
  toggleStar: (slug: string) => void;
  /** Set `providerDir` immediately so onboarding unmounts, then refresh roots. */
  applyProvider: (dir: string) => void;
  /** Re-fetch configured roots, cloud-only rows, and file counts. */
  refreshRoots: () => Promise<void>;
  inflight: number;
  busy: boolean;
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
};

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
