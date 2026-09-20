import { createContext, useContext } from "react";

import type { RootRow } from "./types";

export type AppView = "root" | "all";

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
  toggleStar: (slug: string) => void;
  /** Set `providerDir` immediately so onboarding unmounts, then refresh roots. */
  applyProvider: (dir: string) => void;
  /** Re-fetch `list_roots` and file counts after add / link / remove / recover. */
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

export const RootsContext = createContext<RootsContextValue | null>(null);

export function useRoots(): RootsContextValue {
  const value = useContext(RootsContext);
  if (!value) {
    throw new Error("useRoots must be used within RootsContext");
  }
  return value;
}
