import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { Loader2, Plus, RefreshCw } from "lucide-react";

import { TreeSkeleton } from "@/components/layout/AppSkeleton";
import { RootOverflowMenu, StarRootButton } from "@/components/layout/RootActions";
import { SearchBar } from "@/components/sidebar/SearchBar";
import {
  EntryPickerDialog,
  UntrackEntryDialog,
} from "@/components/tree/EntryPickerDialog";
import { formatBytes } from "@/components/tree/entries";
import { TreeNode } from "@/components/tree/TreeNode";
import { Button } from "@/components/ui/button";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import {
  conflicts as fetchConflicts,
  listEntries,
  maxFileMb,
  syncNow,
  trackedFiles,
} from "@/lib/ipc";
import { useRoots, useSyncing, useTaskLabel } from "@/lib/roots";
import { buildTree, filterTree } from "@/lib/tree";
import type { ConflictView, EntryView, RootRow, TrackedFile } from "@/lib/types";
import { cn } from "@/lib/utils";

const DEFAULT_MAX_FILE_BYTES = 50 * 1024 * 1024;
const EMPTY_CONFLICTS = new Set<string>();

type TreeSnapshot = {
  slug: string | null;
  linked: boolean;
  files: TrackedFile[];
  listed: EntryView[];
  conflicts: Set<string>;
  maxFileBytes: number;
};

/** FileTree stays mounted across sidebar selection — drop the previous project. */
export function treeDialogsAfterRootChange(): {
  pickerOpen: false;
  untrackTarget: null;
  files: TrackedFile[];
  listed: EntryView[];
} {
  return { pickerOpen: false, untrackTarget: null, files: [], listed: [] };
}

export function treeLoadMatches(
  current: { slug: string | null; linked: boolean },
  started: { slug: string | null; linked: boolean },
): boolean {
  return current.slug === started.slug && current.linked === started.linked;
}

/** Linked trees fetch before they can paint. Anything already shown for another
 * project must not stay on screen while that fetch runs. */
export function treeAwaitingLoad(
  current: { slug: string | null; linked: boolean },
  shown: { slug: string | null; linked: boolean } | null,
): boolean {
  if (!current.linked || current.slug === null) return false;
  if (shown === null) return true;
  return shown.slug !== current.slug || shown.linked !== current.linked;
}

/** A detected agent folder whose patterns matched nothing would otherwise show a blank tree. */
export function showAgentEmptyHint(
  root: RootRow,
  fileCount: number,
  filtering: boolean,
): boolean {
  return root.is_agent && root.linked && fileCount === 0 && !filtering;
}

function conflictPathSet(views: ConflictView[]): Set<string> {
  return new Set(views.map((view) => String(view.live).replace(/\\/g, "/")));
}

/**
 * The footer line. An unlinked root never loads a file list, so counting its
 * files would read as "this project is empty" rather than "nothing is being
 * tracked here yet".
 */
export function projectSizeLabel(files: TrackedFile[], linked: boolean): string {
  if (!linked) return "Not linked";
  const totalBytes = files.reduce((sum, file) => sum + file.bytes, 0);
  const fileLabel = `${files.length} ${files.length === 1 ? "file" : "files"}`;
  return `${fileLabel} · ${formatBytes(totalBytes)}`;
}

function TreeSeeding({ name }: { name: string }) {
  return (
    <div
      className="flex h-full min-h-0 min-w-0 flex-col"
      aria-busy="true"
      aria-live="polite"
    >
      <header className="flex h-row min-w-0 shrink-0 items-center border-b border-border px-3">
        <span className="min-w-0 flex-1 truncate text-foreground" title={name}>
          {name}
        </span>
      </header>
      <div className="flex min-h-0 flex-1 flex-col items-center justify-center gap-3 px-6 text-muted-foreground">
        <Loader2 className="size-5 animate-spin" aria-hidden />
        <p className="text-center text-sm">Adding files…</p>
      </div>
    </div>
  );
}

export function FileTree() {
  const { roots, selectedSlug, selectedRel, selectFile, openResolver, busy, seeding } =
    useRoots();
  const syncing = useSyncing();
  const taskLabel = useTaskLabel();
  const tracking = taskLabel !== null;
  const root = roots.find((row) => row.slug === selectedSlug) ?? null;
  const seedingItem = seeding.find((item) => item.slug === selectedSlug) ?? null;

  const [snapshot, setSnapshot] = useState<TreeSnapshot | null>(null);
  const [openBySlug, setOpenBySlug] = useState<Record<string, Record<string, boolean>>>(
    {},
  );
  const [pickerOpen, setPickerOpen] = useState(false);
  const [untrackTarget, setUntrackTarget] = useState<EntryView | null>(null);
  const [query, setQuery] = useState("");
  const loadId = useRef({ slug: selectedSlug, linked: !!root?.linked });

  const statusKey = root ? JSON.stringify(root.status) : "";

  const loadTree = useCallback(async () => {
    if (seedingItem || !selectedSlug || !root?.linked) {
      return {
        files: [] as TrackedFile[],
        listed: [] as EntryView[],
        conflicts: new Set<string>(),
        maxFileBytes: DEFAULT_MAX_FILE_BYTES,
      };
    }
    const [listedFiles, views, listed, maxMb] = await Promise.all([
      trackedFiles(selectedSlug).catch((): TrackedFile[] => []),
      fetchConflicts(selectedSlug).catch((): ConflictView[] => []),
      listEntries(selectedSlug).catch((): EntryView[] => []),
      maxFileMb().catch(() => 50),
    ]);
    return {
      files: listedFiles,
      listed,
      conflicts: conflictPathSet(views),
      maxFileBytes: maxMb * 1024 * 1024,
    };
  }, [selectedSlug, root?.linked, seedingItem]);

  const commitTree = useCallback(
    (
      started: { slug: string | null; linked: boolean },
      next: {
        files: TrackedFile[];
        listed: EntryView[];
        conflicts: Set<string>;
        maxFileBytes: number;
      },
    ) => {
      if (!treeLoadMatches(loadId.current, started)) return;
      setSnapshot({
        slug: started.slug,
        linked: started.linked,
        files: next.files,
        listed: next.listed,
        conflicts: next.conflicts,
        maxFileBytes: next.maxFileBytes,
      });
    },
    [],
  );

  const refetch = useCallback(() => {
    const started = { slug: selectedSlug, linked: !!root?.linked };
    void loadTree().then((next) => {
      commitTree(started, next);
    });
  }, [loadTree, commitTree, selectedSlug, root?.linked]);

  useEffect(() => {
    loadId.current = { slug: selectedSlug, linked: !!root?.linked };
    const next = treeDialogsAfterRootChange();
    setPickerOpen(next.pickerOpen);
    setUntrackTarget(next.untrackTarget);
    setQuery("");
  }, [selectedSlug, root?.linked]);

  useEffect(() => {
    if (seedingItem) return;
    const started = { slug: selectedSlug, linked: !!root?.linked };
    void (async () => {
      const next = await loadTree();
      commitTree(started, next);
    })();
  }, [loadTree, commitTree, statusKey, selectedSlug, root?.linked, seedingItem]);

  const currentTree = { slug: selectedSlug, linked: !!root?.linked };
  const treeReady =
    snapshot !== null &&
    snapshot.slug === currentTree.slug &&
    snapshot.linked === currentTree.linked;
  const awaiting = !seedingItem && treeAwaitingLoad(currentTree, snapshot);
  const files = treeReady && snapshot ? snapshot.files : [];
  const entries = treeReady && snapshot ? snapshot.listed : [];
  const conflictSet = treeReady && snapshot ? snapshot.conflicts : EMPTY_CONFLICTS;
  const maxFileBytes = treeReady && snapshot ? snapshot.maxFileBytes : DEFAULT_MAX_FILE_BYTES;

  const tree = useMemo(() => (awaiting ? [] : buildTree(files)), [awaiting, files]);
  const filtering = query.trim().length > 0;
  const visibleTree = useMemo(() => filterTree(tree, query), [tree, query]);

  const isOpen = useCallback(
    (path: string, depth: number): boolean => {
      if (filtering) return true;
      if (!selectedSlug) return false;
      const explicit = openBySlug[selectedSlug]?.[path];
      if (explicit !== undefined) return explicit;
      return depth === 0;
    },
    [filtering, openBySlug, selectedSlug],
  );

  const onToggle = useCallback(
    (path: string, depth: number) => {
      if (!selectedSlug || filtering) return;
      const next = !isOpen(path, depth);
      setOpenBySlug((current) => ({
        ...current,
        [selectedSlug]: { ...current[selectedSlug], [path]: next },
      }));
    },
    [filtering, isOpen, selectedSlug],
  );

  if (seedingItem) {
    return <TreeSeeding name={seedingItem.name} />;
  }

  if (!root) {
    return <div className="h-full" />;
  }

  if (awaiting) {
    return <TreeSkeleton title={root.name} />;
  }

  return (
    <div className="flex h-full min-h-0 min-w-0 flex-col">
      <header className="flex h-row min-w-0 shrink-0 items-center gap-2 border-b border-border px-3">
        <span className="min-w-0 flex-1 truncate text-foreground" title={root.name}>
          {root.name}
        </span>
        <div className="flex shrink-0 items-center gap-0.5">
          <StarRootButton />
          <Tooltip>
            <TooltipTrigger
              render={
                <Button
                  variant="ghost"
                  size="icon-xs"
                  className="text-muted-foreground"
                  disabled={busy || syncing || !root.linked}
                  aria-busy={syncing || undefined}
                  aria-label="Sync now"
                  onClick={() => {
                    void syncNow().catch(() => {
                      // Banner is set by `run()`.
                    });
                  }}
                />
              }
            >
              <RefreshCw
                aria-hidden
                className={cn(syncing && "animate-spin")}
              />
            </TooltipTrigger>
            <TooltipContent>Sync now</TooltipContent>
          </Tooltip>
          <Tooltip>
            <TooltipTrigger render={<span className="inline-flex" />}>
              <Button
                variant="ghost"
                size="icon-xs"
                className="text-muted-foreground"
                disabled={busy || tracking || !root.linked}
                aria-label="Add to track"
                onClick={() => setPickerOpen(true)}
              >
                <Plus aria-hidden />
              </Button>
            </TooltipTrigger>
            <TooltipContent>
              {tracking ? "A process is running" : "Add to track"}
            </TooltipContent>
          </Tooltip>
          <RootOverflowMenu />
        </div>
      </header>
      <div className="flex shrink-0 border-b border-border px-3 py-2">
        <SearchBar
          value={query}
          onChange={setQuery}
          placeholder="Search files"
          label="Search files and folders"
          onKeyDown={(event) => {
            if (event.key === "Escape" && query) {
              event.preventDefault();
              setQuery("");
            }
          }}
        />
      </div>
      <div className="panel-scroll min-h-0 min-w-0 flex-1 overflow-auto">
        <div className="flex flex-col gap-0.5 px-1.5 py-1">
        {filtering && visibleTree.length === 0 ? (
          <p className="px-2 py-1.5 text-sm text-muted-foreground">No matches</p>
        ) : null}
        {!root.linked ? (
          <p className="px-2 py-1.5 text-sm text-muted-foreground">
            This folder is in your cloud but not linked on this Mac. Use the
            link button on its sidebar row to pick a local folder.
          </p>
        ) : null}
        {showAgentEmptyHint(root, files.length, filtering) ? (
          <p className="px-2 py-1.5 text-sm text-muted-foreground">
            Agent found on this Mac, but no files match its patterns. Use + to
            add or edit patterns.
          </p>
        ) : null}
        {visibleTree.map((node) => (
          <TreeNode
            key={node.path}
            node={node}
            depth={0}
            selectedRel={selectedRel}
            conflictSet={conflictSet}
            isOpen={isOpen}
            onToggle={onToggle}
            onSelect={(path) => {
              if (conflictSet.has(path) && selectedSlug) {
                openResolver(selectedSlug, path);
              } else {
                selectFile(path);
              }
            }}
            entries={entries}
            onUntrack={setUntrackTarget}
            rootPath={root.path}
            maxFileBytes={maxFileBytes}
          />
        ))}
        </div>
      </div>
      <footer className="flex h-row shrink-0 items-center border-t border-border px-3 text-xs tabular-nums text-muted-foreground">
        {projectSizeLabel(files, root.linked)}
      </footer>
      {root.linked ? (
        <>
          <EntryPickerDialog
            key={root.slug}
            slug={root.slug}
            entries={entries}
            files={files}
            open={pickerOpen}
            onOpenChange={setPickerOpen}
            onMutated={refetch}
          />
          <UntrackEntryDialog
            key={`untrack-${root.slug}`}
            slug={root.slug}
            entry={untrackTarget}
            entries={entries}
            open={untrackTarget !== null}
            onOpenChange={(next) => {
              if (!next) setUntrackTarget(null);
            }}
            onMutated={refetch}
          />
        </>
      ) : null}
    </div>
  );
}
