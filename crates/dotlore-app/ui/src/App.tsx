import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";

import { Footer } from "@/components/layout/Footer";
import { Shell } from "@/components/layout/Shell";
import { AllProjects } from "@/components/main/AllProjects";
import { EmptyState } from "@/components/main/EmptyState";
import { FileViewer } from "@/components/main/FileViewer";
import { GitMissingBanner } from "@/components/settings/SettingsPopover";
import { Sidebar } from "@/components/sidebar/Sidebar";
import { ProviderSetup } from "@/components/setup/ProviderSetup";
import { FileTree } from "@/components/tree/FileTree";
import { ErrorBanner } from "@/components/ui/ErrorBanner";
import { TooltipProvider } from "@/components/ui/tooltip";
import {
  gitMissing as fetchGitMissing,
  getWorkSnapshot,
  listRoots,
  listenStatus,
  providerDir as fetchProviderDir,
  setBanner,
  subscribeWork,
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

function MainColumn() {
  const { gitMissing, providerDir, banner, setBanner } = useRoots();
  return (
    <div className="flex h-full min-h-0 flex-col">
      {banner !== null ? (
        <ErrorBanner message={banner} onDismiss={() => setBanner(null)} />
      ) : null}
      {gitMissing ? <GitMissingBanner /> : null}
      <div className="min-h-0 flex-1 overflow-hidden">
        {providerDir === null ? <ProviderSetup /> : <MainPanel />}
      </div>
    </div>
  );
}

export function App() {
  const [state, setState] = useState<RootsState>(() => ({
    ...emptyRootsState,
    starredSlugs: readStarred(),
  }));
  const [ready, setReady] = useState(false);
  const liveRef = useRef<RootRow[]>([]);
  const work = useSyncExternalStore(
    subscribeWork,
    getWorkSnapshot,
    getWorkSnapshot,
  );

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
      setReady(true);
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

  const applyProvider = useCallback((dir: string) => {
    setState((current) => ({ ...current, providerDir: dir, error: null }));
    void (async () => {
      const [roots, providerDir] = await Promise.all([
        listRoots().catch((): RootRow[] => []),
        fetchProviderDir().catch((): string | null => dir),
      ]);
      setState((current) => ({
        ...current,
        roots: overlayStatuses(roots, liveRef.current),
        providerDir: providerDir ?? dir,
      }));
      const trackedBySlug = await loadTrackedCounts(roots);
      setState((current) => ({ ...current, trackedBySlug }));
    })();
  }, []);

  const refreshRoots = useCallback(async () => {
    const roots = await listRoots().catch((): RootRow[] => []);
    setState((current) => {
      const stillSelected =
        current.selectedSlug !== null &&
        roots.some((row) => row.slug === current.selectedSlug);
      return {
        ...current,
        roots: overlayStatuses(roots, liveRef.current),
        selectedSlug: stillSelected ? current.selectedSlug : null,
        selectedRel: stillSelected ? current.selectedRel : null,
      };
    });
    const trackedBySlug = await loadTrackedCounts(roots);
    setState((current) => ({ ...current, trackedBySlug }));
  }, []);

  const value = useMemo(
    () => ({
      ...state,
      selectRoot,
      selectFile,
      showAllProjects,
      toggleStar,
      applyProvider,
      refreshRoots,
      inflight: work.inflight,
      busy: work.inflight > 0,
      banner: work.banner,
      setBanner,
    }),
    [
      state,
      selectRoot,
      selectFile,
      showAllProjects,
      toggleStar,
      applyProvider,
      refreshRoots,
      work,
    ],
  );

  const onboarding = ready && state.providerDir === null;

  return (
    <TooltipProvider delay={400}>
      <RootsContext.Provider value={value}>
        <Shell
          hideTree={onboarding || !ready || state.view === "all"}
          sidebar={onboarding || !ready ? null : <Sidebar />}
          tree={onboarding || !ready ? undefined : <FileTree />}
          main={!ready ? null : <MainColumn />}
          footer={<Footer />}
        />
      </RootsContext.Provider>
    </TooltipProvider>
  );
}
