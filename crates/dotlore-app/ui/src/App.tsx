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
import { ConflictResolver } from "@/components/main/ConflictResolver";
import { EmptyState } from "@/components/main/EmptyState";
import { FileViewer } from "@/components/main/FileViewer";
import { GitMissingBanner } from "@/components/settings/SettingsPopover";
import { Sidebar } from "@/components/sidebar/Sidebar";
import { ProviderSetup } from "@/components/setup/ProviderSetup";
import { FileTree } from "@/components/tree/FileTree";
import { ErrorBanner } from "@/components/ui/ErrorBanner";
import { TooltipProvider } from "@/components/ui/tooltip";
import { uniqueConflictRels } from "@/lib/conflicts";
import { errorMessage } from "@/lib/errors";
import {
  conflicts as fetchConflicts,
  gitMissing as fetchGitMissing,
  getWorkSnapshot,
  listLinkable,
  listRoots,
  listenStatus,
  providerDir as fetchProviderDir,
  setBanner,
  subscribeWork,
  trackedFiles,
} from "@/lib/ipc";
import {
  applyRootDiscovery,
  emptyRootsState,
  RootsContext,
  useRoots,
  type CloudDiscovery,
  type LocalDiscovery,
  type RootsState,
  type SelectRootOptions,
} from "@/lib/roots";
import { readStarred, toggleStarred } from "@/lib/starred";
import type { ConflictView, RootRow } from "@/lib/types";

function overlayStatuses(rows: RootRow[], live: RootRow[]): RootRow[] {
  if (live.length === 0) return rows;
  const bySlug = new Map(live.map((row) => [row.slug, row.status]));
  return rows.map((row) => {
    const status = bySlug.get(row.slug);
    return status ? { ...row, status } : row;
  });
}

function isLinked(roots: RootRow[], slug: string | null): boolean {
  if (!slug) return false;
  return roots.find((row) => row.slug === slug)?.linked === true;
}

async function loadTrackedCounts(
  roots: RootRow[],
): Promise<Record<string, number>> {
  const entries = await Promise.all(
    roots.map(async (row) => {
      if (!row.linked) return [row.slug, 0] as const;
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
  const { view, selectedSlug, selectedRel, resolvingRel, closeResolver } =
    useRoots();
  if (view === "all") return <AllProjects />;
  if (view === "starred") return <AllProjects starredOnly />;
  if (!selectedSlug || !selectedRel) return <EmptyState />;
  if (resolvingRel === selectedRel) {
    return (
      <ConflictResolver
        slug={selectedSlug}
        rel={selectedRel}
        onClose={closeResolver}
      />
    );
  }
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
  const discoveryGen = useRef(0);
  const discoveryErrorRef = useRef<string | null>(null);
  const rootsRef = useRef(state.roots);
  rootsRef.current = state.roots;
  const work = useSyncExternalStore(
    subscribeWork,
    getWorkSnapshot,
    getWorkSnapshot,
  );

  const refreshCombined = useCallback(async () => {
    const generation = ++discoveryGen.current;
    const [local, cloud] = await Promise.all([
      listRoots().then(
        (rows): LocalDiscovery => ({ ok: true, rows }),
        (err): LocalDiscovery => ({
          ok: false,
          message: errorMessage(err, "Could not list configured projects"),
        }),
      ),
      listLinkable().then(
        (rows): CloudDiscovery => ({ ok: true, rows }),
        (err): CloudDiscovery => ({
          ok: false,
          message: errorMessage(err, "Could not list cloud projects"),
        }),
      ),
    ]);
    if (generation !== discoveryGen.current) return;

    let discovered: RootRow[] | null = null;
    setState((current) => {
      const applied = applyRootDiscovery({
        generation,
        latestGeneration: discoveryGen.current,
        local,
        cloud,
        previous: current.roots,
      });
      if (!applied) return current;
      discovered = applied.roots;
      const selected = applied.roots.find(
        (row) => row.slug === current.selectedSlug,
      );
      const stillSelected = selected !== undefined;
      const keepLive = selected?.linked === true;
      let error = current.error;
      if (applied.discoveryError !== null) {
        discoveryErrorRef.current = applied.discoveryError;
        error = applied.discoveryError;
      } else if (current.error === discoveryErrorRef.current) {
        error = null;
        discoveryErrorRef.current = null;
      } else {
        discoveryErrorRef.current = null;
      }
      return {
        ...current,
        roots: overlayStatuses(applied.roots, liveRef.current),
        error,
        selectedSlug: stillSelected ? current.selectedSlug : null,
        selectedRel: keepLive ? current.selectedRel : null,
        resolvingRel: keepLive ? current.resolvingRel : null,
      };
    });
    if (!discovered) return;
    const trackedBySlug = await loadTrackedCounts(discovered);
    if (generation !== discoveryGen.current) return;
    setState((current) => ({ ...current, trackedBySlug }));
  }, []);

  useEffect(() => {
    let cancelled = false;

    void (async () => {
      const [providerDir, gitMissing] = await Promise.all([
        fetchProviderDir().catch((): null => null),
        fetchGitMissing().catch((): boolean => false),
      ]);
      if (cancelled) return;
      setState((current) => ({
        ...current,
        providerDir,
        gitMissing,
      }));
      await refreshCombined();
      if (!cancelled) setReady(true);
    })();

    const unlisten = listenStatus((payload) => {
      liveRef.current = payload.roots;
      setState((current) => ({
        ...current,
        error: payload.error ?? discoveryErrorRef.current,
        roots: overlayStatuses(current.roots, payload.roots),
      }));
    });

    return () => {
      cancelled = true;
      void unlisten.then((stop) => stop());
    };
  }, [refreshCombined]);

  const selectRoot = useCallback((slug: string, options?: SelectRootOptions) => {
    setState((current) => ({
      ...current,
      view: "root",
      selectedSlug: slug,
      selectedRel: current.selectedSlug === slug ? current.selectedRel : null,
      resolvingRel: current.selectedSlug === slug ? current.resolvingRel : null,
      focusRequest: options?.focusSidebar
        ? { slug, seq: (current.focusRequest?.seq ?? 0) + 1 }
        : current.focusRequest,
    }));
  }, []);

  const selectFile = useCallback((rel: string) => {
    setState((current) => {
      if (!isLinked(current.roots, current.selectedSlug)) return current;
      return {
        ...current,
        selectedRel: rel,
        resolvingRel: null,
      };
    });
  }, []);

  const openResolver = useCallback((slug: string, rel: string) => {
    setState((current) => {
      if (!isLinked(current.roots, slug)) return current;
      return {
        ...current,
        view: "root",
        selectedSlug: slug,
        selectedRel: rel,
        resolvingRel: rel,
      };
    });
  }, []);

  const openFirstConflict = useCallback((slug: string) => {
    if (!isLinked(rootsRef.current, slug)) {
      selectRoot(slug);
      return;
    }
    void (async () => {
      try {
        const views = await fetchConflicts(slug).catch((): ConflictView[] => []);
        const first = uniqueConflictRels(views)[0];
        if (!first) {
          selectRoot(slug);
          return;
        }
        openResolver(slug, first);
      } catch {
        selectRoot(slug);
      }
    })();
  }, [openResolver, selectRoot]);

  const closeResolver = useCallback(() => {
    setState((current) => ({
      ...current,
      resolvingRel: null,
    }));
  }, []);

  const showAllProjects = useCallback(() => {
    setState((current) => ({
      ...current,
      view: "all",
      resolvingRel: null,
    }));
  }, []);

  const showStarred = useCallback(() => {
    setState((current) => ({
      ...current,
      view: "starred",
      resolvingRel: null,
    }));
  }, []);

  const toggleStar = useCallback((slug: string) => {
    const starredSlugs = toggleStarred(slug);
    setState((current) => ({ ...current, starredSlugs }));
  }, []);

  const applyProvider = useCallback((dir: string) => {
    discoveryErrorRef.current = null;
    setState((current) => ({ ...current, providerDir: dir, error: null }));
    void (async () => {
      const providerDir = await fetchProviderDir().catch(
        (): string | null => dir,
      );
      setState((current) => ({
        ...current,
        providerDir: providerDir ?? dir,
      }));
      await refreshCombined();
    })();
  }, [refreshCombined]);

  const refreshRoots = refreshCombined;

  const value = useMemo(
    () => ({
      ...state,
      selectRoot,
      selectFile,
      openResolver,
      openFirstConflict,
      closeResolver,
      showAllProjects,
      showStarred,
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
      openResolver,
      openFirstConflict,
      closeResolver,
      showAllProjects,
      showStarred,
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
          hideTree={onboarding || !ready || state.view !== "root"}
          sidebar={onboarding || !ready ? null : <Sidebar />}
          tree={onboarding || !ready ? undefined : <FileTree />}
          main={!ready ? null : <MainColumn />}
          footer={<Footer />}
        />
      </RootsContext.Provider>
    </TooltipProvider>
  );
}
