import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";

import {
  MainSkeleton,
  SidebarSkeleton,
  TreeSkeleton,
} from "@/components/layout/AppSkeleton";
import { Footer } from "@/components/layout/Footer";
import { Shell } from "@/components/layout/Shell";
import { AllProjects } from "@/components/main/AllProjects";
import { ConflictResolver } from "@/components/main/ConflictResolver";
import { DiscardChangesAlert } from "@/components/main/DiscardChangesAlert";
import { EmptyState } from "@/components/main/EmptyState";
import { FileViewer } from "@/components/main/FileViewer";
import { NoticeToasts } from "@/components/notices/NoticeToasts";
import { Sidebar } from "@/components/sidebar/Sidebar";
import { ProviderSetup } from "@/components/setup/ProviderSetup";
import { TrackConfirmDialog } from "@/components/tree/EntryPickerDialog";
import { FileTree } from "@/components/tree/FileTree";
import { Toaster } from "@/components/ui/toast";
import { TooltipProvider } from "@/components/ui/tooltip";
import { uniqueConflictRels } from "@/lib/conflicts";
import { errorMessage } from "@/lib/errors";
import {
  addRoot,
  BLOCKED,
  conflicts as fetchConflicts,
  gitMissing as fetchGitMissing,
  getWorkSnapshot,
  importInstalledAgents,
  listLinkable,
  listRoots,
  listenStatus,
  providerDir as fetchProviderDir,
  setBanner,
  subscribeWork,
  trackedFiles,
} from "@/lib/ipc";
import { type LeaveTarget, shouldPromptLeave } from "@/lib/leave-guard";
import {
  applyRootDiscovery,
  emptyRootsState,
  emptyTrackedStats,
  folderName,
  overlayStatuses,
  RootsContext,
  rootsWithSeeding,
  useRoots,
  type CloudDiscovery,
  type LocalDiscovery,
  type RootsState,
  type SelectRootOptions,
  type TrackedStats,
} from "@/lib/roots";
import { SidebarQueryProvider } from "@/lib/sidebar-query";
import { readStarred, toggleStarred } from "@/lib/starred";
import type { ConflictView, RootRow } from "@/lib/types";

/** Import catalog agents when a provider is set. Failures show as a toast; refresh still runs. */
async function importAgentsWhenReady(dir: string | null): Promise<void> {
  if (dir === null) return;
  try {
    const report = await importInstalledAgents();
    if (report === BLOCKED) return;
    const first = report.failed[0];
    if (first) setBanner(first.message);
  } catch {
    // `runTask` already stored the command error for the toast.
  }
}

function isLinked(roots: RootRow[], slug: string | null): boolean {
  if (!slug) return false;
  return roots.find((row) => row.slug === slug)?.linked === true;
}

function statsOf(files: { bytes: number }[]): TrackedStats {
  return {
    files: files.length,
    bytes: files.reduce((sum, file) => sum + file.bytes, 0),
  };
}

async function loadTrackedCounts(
  roots: RootRow[],
): Promise<Record<string, TrackedStats>> {
  const entries = await Promise.all(
    roots.map(async (row) => {
      if (!row.linked) return [row.slug, emptyTrackedStats] as const;
      try {
        const files = await trackedFiles(row.slug);
        return [row.slug, statsOf(files)] as const;
      } catch {
        return [row.slug, emptyTrackedStats] as const;
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
  if (view === "conflicts") return <AllProjects conflictsOnly />;
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
  const { providerDir } = useRoots();
  return (
    <div className="h-full min-h-0 overflow-hidden">
      {providerDir === null ? <ProviderSetup /> : <MainPanel />}
    </div>
  );
}

export function App() {
  const [state, setState] = useState<RootsState>(() => ({
    ...emptyRootsState,
    starredSlugs: readStarred(),
    loadingRoots: true,
  }));
  const [sidebarQuery, setSidebarQuery] = useState("");
  const [ready, setReady] = useState(false);
  const liveRef = useRef<RootRow[]>([]);
  const discoveryGen = useRef(0);
  const discoveryErrorRef = useRef<string | null>(null);
  const rootsRef = useRef(state.roots);
  rootsRef.current = state.roots;
  const stateRef = useRef(state);
  stateRef.current = state;
  const dirtyRef = useRef(false);
  const [pendingLeave, setPendingLeave] = useState<{
    rel: string;
    run: () => void;
  } | null>(null);
  const leaveNameRef = useRef("");
  if (pendingLeave) {
    leaveNameRef.current = pendingLeave.rel.split("/").pop() ?? pendingLeave.rel;
  }
  const seedingSlugs = useRef(new Set<string>());
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

    const applied = applyRootDiscovery({
      generation,
      latestGeneration: discoveryGen.current,
      local,
      cloud,
      previous: rootsRef.current,
    });
    if (!applied) return;
    const discovered = applied.roots;
    setState((current) => {
      if (generation !== discoveryGen.current) return current;
      const selected = discovered.find(
        (row) => row.slug === current.selectedSlug,
      );
      const stillSelected = selected !== undefined;
      const seedingSelected = current.seeding.some(
        (item) => item.slug === current.selectedSlug,
      );
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
        roots: overlayStatuses(discovered, liveRef.current),
        error,
        selectedSlug:
          stillSelected || seedingSelected ? current.selectedSlug : null,
        selectedRel: keepLive ? current.selectedRel : null,
        resolvingRel: keepLive ? current.resolvingRel : null,
      };
    });
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
      await importAgentsWhenReady(providerDir);
      if (cancelled) return;
      await refreshCombined();
      if (cancelled) return;
      setState((current) => ({ ...current, loadingRoots: false }));
      setReady(true);
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

  const setResolverDirty = useCallback((dirty: boolean) => {
    dirtyRef.current = dirty;
  }, []);

  /** Ask before a user action closes a resolver with unsaved Result changes. */
  const guardLeave = useCallback(
    (run: () => void, target: LeaveTarget = { kind: "other" }) => {
      const current = stateRef.current;
      const rel = current.resolvingRel;
      if (rel !== null && shouldPromptLeave(current, dirtyRef.current, target)) {
        setPendingLeave({ rel, run });
        return;
      }
      run();
    },
    [],
  );

  const selectRoot = useCallback((slug: string, options?: SelectRootOptions) => {
    const apply = () =>
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
    guardLeave(apply, { kind: "selectRoot", slug });
  }, [guardLeave]);

  const selectFile = useCallback((rel: string) => {
    guardLeave(() =>
      setState((current) => {
        if (!isLinked(current.roots, current.selectedSlug)) return current;
        return {
          ...current,
          selectedRel: rel,
          resolvingRel: null,
        };
      }),
    );
  }, [guardLeave]);

  const openResolver = useCallback((slug: string, rel: string) => {
    const apply = () =>
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
    guardLeave(apply, { kind: "openResolver", slug, rel });
  }, [guardLeave]);

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
    guardLeave(() =>
      setState((current) => ({
        ...current,
        resolvingRel: null,
      })),
    );
  }, [guardLeave]);

  const showAllProjects = useCallback(() => {
    guardLeave(() =>
      setState((current) => ({
        ...current,
        view: "all",
        resolvingRel: null,
      })),
    );
  }, [guardLeave]);

  const showStarred = useCallback(() => {
    guardLeave(() =>
      setState((current) => ({
        ...current,
        view: "starred",
        resolvingRel: null,
      })),
    );
  }, [guardLeave]);

  const showConflicts = useCallback(() => {
    guardLeave(() =>
      setState((current) => ({
        ...current,
        view: "conflicts",
        resolvingRel: null,
      })),
    );
  }, [guardLeave]);

  const toggleStar = useCallback((slug: string) => {
    const starredSlugs = toggleStarred(slug);
    setState((current) => ({ ...current, starredSlugs }));
  }, []);

  const addProject = useCallback(
    async (path: string, slug: string) => {
      const name = folderName(path, slug);
      if (seedingSlugs.current.has(slug)) return;
      seedingSlugs.current.add(slug);
      setState((current) => ({
        ...current,
        seeding: current.seeding.some((item) => item.slug === slug)
          ? current.seeding
          : [...current.seeding, { slug, path, name }],
      }));
      try {
        const created = await addRoot(path, slug);
        await refreshCombined();
        const isHere = (selected: string | null) =>
          selected === slug || selected === created;
        if (isHere(stateRef.current.selectedSlug)) {
          guardLeave(() =>
            setState((current) => {
              if (!isHere(current.selectedSlug)) return current;
              return {
                ...current,
                view: "root",
                selectedSlug: created,
                selectedRel: null,
                resolvingRel: null,
              };
            }),
          );
        }
      } catch (err) {
        setBanner(errorMessage(err, "Could not add project"));
        setState((current) => {
          if (current.selectedSlug !== slug) return current;
          return {
            ...current,
            selectedSlug: null,
            selectedRel: null,
            resolvingRel: null,
          };
        });
      } finally {
        seedingSlugs.current.delete(slug);
        setState((current) => ({
          ...current,
          seeding: current.seeding.filter((item) => item.slug !== slug),
        }));
      }
    },
    [guardLeave, refreshCombined],
  );

  const applyProvider = useCallback((dir: string) => {
    discoveryErrorRef.current = null;
    setState((current) => ({
      ...current,
      providerDir: dir,
      error: null,
      loadingRoots: true,
    }));
    void (async () => {
      const providerDir = await fetchProviderDir().catch(
        (): string | null => dir,
      );
      const resolved = providerDir ?? dir;
      setState((current) => ({
        ...current,
        providerDir: resolved,
      }));
      await importAgentsWhenReady(resolved);
      await refreshCombined();
      setState((current) => ({ ...current, loadingRoots: false }));
    })();
  }, [refreshCombined]);

  const refreshRoots = refreshCombined;

  const value = useMemo(
    () => ({
      ...state,
      roots: rootsWithSeeding(state.roots, state.seeding),
      selectRoot,
      selectFile,
      openResolver,
      openFirstConflict,
      closeResolver,
      setResolverDirty,
      showAllProjects,
      showStarred,
      showConflicts,
      toggleStar,
      applyProvider,
      refreshRoots,
      addProject,
      inflight: work.inflight,
      busy: work.inflight > 0,
      locked: work.inflight > 0 || work.taskLabel !== null,
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
      setResolverDirty,
      showAllProjects,
      showStarred,
      showConflicts,
      toggleStar,
      applyProvider,
      refreshRoots,
      addProject,
      work,
    ],
  );

  const onboarding = ready && state.providerDir === null;

  return (
    <TooltipProvider delay={400}>
      <SidebarQueryProvider query={sidebarQuery} setQuery={setSidebarQuery}>
        <RootsContext.Provider value={value}>
          <TrackConfirmDialog />
          <DiscardChangesAlert
            open={pendingLeave !== null}
            fileName={leaveNameRef.current}
            onCancel={() => setPendingLeave(null)}
            onDiscard={() => {
              if (!pendingLeave) return;
              dirtyRef.current = false;
              const run = pendingLeave.run;
              setPendingLeave(null);
              run();
            }}
          />
          <Toaster>
            <NoticeToasts banner={work.banner} gitMissing={state.gitMissing} />
            <Shell
              hideTree={ready && (onboarding || state.view !== "root")}
              sidebar={!ready ? <SidebarSkeleton /> : onboarding ? null : <Sidebar />}
              tree={!ready ? <TreeSkeleton /> : <FileTree />}
              main={!ready ? <MainSkeleton /> : <MainColumn />}
              footer={<Footer />}
            />
          </Toaster>
        </RootsContext.Provider>
      </SidebarQueryProvider>
    </TooltipProvider>
  );
}
