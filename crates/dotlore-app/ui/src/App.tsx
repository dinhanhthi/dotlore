import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { Footer } from "@/components/layout/Footer";
import { Shell } from "@/components/layout/Shell";
import { AllProjects } from "@/components/main/AllProjects";
import { EmptyState } from "@/components/main/EmptyState";
import { FileViewer } from "@/components/main/FileViewer";
import { Sidebar } from "@/components/sidebar/Sidebar";
import { FileTree } from "@/components/tree/FileTree";
import { TooltipProvider } from "@/components/ui/tooltip";
import {
  gitMissing as fetchGitMissing,
  listRoots,
  listenStatus,
  providerDir as fetchProviderDir,
  trackedFiles,
} from "@/lib/ipc";
import {
  emptyRootsState,
  RootsContext,
  useRoots,
  type RootsState,
  type SelectRootOptions,
} from "@/lib/roots";
import { readStarred, toggleStarred } from "@/lib/starred";
import type { RootRow } from "@/lib/types";

function overlayStatuses(rows: RootRow[], live: RootRow[]): RootRow[] {
  if (live.length === 0) return rows;
  if (rows.length === 0) return live;
  const bySlug = new Map(live.map((row) => [row.slug, row.status]));
  return rows.map((row) => {
    const status = bySlug.get(row.slug);
    return status ? { ...row, status } : row;
  });
}

async function loadTrackedCounts(
  roots: RootRow[],
): Promise<Record<string, number>> {
  const entries = await Promise.all(
    roots.map(async (row) => {
      try {
        const files = await trackedFiles(row.slug);
        return [row.slug, files.length] as const;
      } catch {
        return [row.slug, 0] as const;
      }
    }),
  );
  return Object.fromEntries(entries);
}

function MainPanel() {
  const { view, selectedSlug, selectedRel } = useRoots();
  if (view === "all") return <AllProjects />;
  if (!selectedSlug || !selectedRel) return <EmptyState />;
  return <FileViewer slug={selectedSlug} rel={selectedRel} />;
}

export function App() {
  const [state, setState] = useState<RootsState>(() => ({
    ...emptyRootsState,
    starredSlugs: readStarred(),
  }));
  const liveRef = useRef<RootRow[]>([]);

  useEffect(() => {
    let cancelled = false;

    void (async () => {
      const [roots, providerDir, gitMissing] = await Promise.all([
        listRoots().catch((): RootRow[] => []),
        fetchProviderDir().catch((): null => null),
        fetchGitMissing().catch((): boolean => false),
      ]);
      if (cancelled) return;
      setState((current) => ({
        ...current,
        roots: overlayStatuses(roots, liveRef.current),
        providerDir,
        gitMissing,
      }));
      const trackedBySlug = await loadTrackedCounts(roots);
      if (!cancelled) {
        setState((current) => ({ ...current, trackedBySlug }));
      }
    })();

    const unlisten = listenStatus((payload) => {
      liveRef.current = payload.roots;
      setState((current) => ({
        ...current,
        error: payload.error,
        roots: overlayStatuses(current.roots, payload.roots),
      }));
    });

    return () => {
      cancelled = true;
      void unlisten.then((stop) => stop());
    };
  }, []);

  const selectRoot = useCallback((slug: string, options?: SelectRootOptions) => {
    setState((current) => ({
      ...current,
      view: "root",
      selectedSlug: slug,
      selectedRel: current.selectedSlug === slug ? current.selectedRel : null,
      focusRequest: options?.focusSidebar
        ? { slug, seq: (current.focusRequest?.seq ?? 0) + 1 }
        : current.focusRequest,
    }));
  }, []);

  const selectFile = useCallback((rel: string) => {
    setState((current) => ({
      ...current,
      selectedRel: rel,
    }));
  }, []);

  const showAllProjects = useCallback(() => {
    setState((current) => ({
      ...current,
      view: "all",
    }));
  }, []);

  const toggleStar = useCallback((slug: string) => {
    const starredSlugs = toggleStarred(slug);
    setState((current) => ({ ...current, starredSlugs }));
  }, []);

  const value = useMemo(
    () => ({ ...state, selectRoot, selectFile, showAllProjects, toggleStar }),
    [state, selectRoot, selectFile, showAllProjects, toggleStar],
  );

  return (
    <TooltipProvider delay={400}>
      <RootsContext.Provider value={value}>
        <Shell
          hideTree={state.view === "all"}
          sidebar={<Sidebar />}
          tree={<FileTree />}
          main={<MainPanel />}
          footer={<Footer />}
        />
      </RootsContext.Provider>
    </TooltipProvider>
  );
}
