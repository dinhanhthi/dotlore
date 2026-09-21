import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { Info, Loader2, Plus, RefreshCw } from "lucide-react";

import { SearchBar } from "@/components/sidebar/SearchBar";
import {
  EntryPickerDialog,
  UntrackEntryDialog,
} from "@/components/tree/EntryPickerDialog";
import { formatBytes } from "@/components/tree/entries";
import { TreeNode } from "@/components/tree/TreeNode";
import { Button } from "@/components/ui/button";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { ScrollArea } from "@/components/ui/scroll-area";
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
import { useRoots, useSyncing } from "@/lib/roots";
import { buildTree, filterTree } from "@/lib/tree";
import type { ConflictView, EntryView, TrackedFile } from "@/lib/types";
import { cn } from "@/lib/utils";

const DEFAULT_MAX_FILE_BYTES = 50 * 1024 * 1024;

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

function conflictPathSet(views: ConflictView[]): Set<string> {
  return new Set(views.map((view) => String(view.live).replace(/\\/g, "/")));
}

function TrackedSummary({ files }: { files: TrackedFile[] }) {
  const [open, setOpen] = useState(false);
  const totalBytes = files.reduce((sum, file) => sum + file.bytes, 0);
  const fileLabel = `${files.length} ${files.length === 1 ? "file" : "files"}`;

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <Tooltip disabled={open}>
        <TooltipTrigger
          render={
            <PopoverTrigger
              render={
                <Button
                  variant="ghost"
                  size="icon-xs"
                  className="text-muted-foreground"
                  aria-label="Tracked files"
                />
              }
            />
          }
        >
          <Info aria-hidden />
        </TooltipTrigger>
        <TooltipContent>Tracked files</TooltipContent>
      </Tooltip>
      <PopoverContent align="end" sideOffset={6} className="w-52 gap-2 p-3">
        <div className="flex items-baseline justify-between gap-4">
          <span className="text-muted-foreground">Files</span>
          <span className="tabular-nums">{fileLabel}</span>
        </div>
        <div className="flex items-baseline justify-between gap-4">
          <span className="text-muted-foreground">Size</span>
          <span className="tabular-nums">{formatBytes(totalBytes)}</span>
        </div>
      </PopoverContent>
    </Popover>
  );
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
  const root = roots.find((row) => row.slug === selectedSlug) ?? null;
  const seedingItem = seeding.find((item) => item.slug === selectedSlug) ?? null;

  const [files, setFiles] = useState<TrackedFile[]>([]);
  const [entries, setEntries] = useState<EntryView[]>([]);
  const [conflictSet, setConflictSet] = useState<Set<string>>(() => new Set());
  const [maxFileBytes, setMaxFileBytes] = useState(DEFAULT_MAX_FILE_BYTES);
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

  const applyTree = useCallback(
    (next: {
      files: TrackedFile[];
      listed: EntryView[];
      conflicts: Set<string>;
      maxFileBytes: number;
    }) => {
      setFiles(next.files);
      setEntries(next.listed);
      setConflictSet(next.conflicts);
      setMaxFileBytes(next.maxFileBytes);
    },
    [],
  );

  const refetch = useCallback(() => {
    const started = { slug: selectedSlug, linked: !!root?.linked };
    void loadTree().then((next) => {
      if (!treeLoadMatches(loadId.current, started)) return;
      applyTree(next);
    });
  }, [loadTree, applyTree, selectedSlug, root?.linked]);

  useEffect(() => {
    if (seedingItem) return;
    const started = { slug: selectedSlug, linked: !!root?.linked };
    void (async () => {
      const next = await loadTree();
      if (!treeLoadMatches(loadId.current, started)) return;
      applyTree(next);
    })();
  }, [loadTree, applyTree, statusKey, selectedSlug, root?.linked, seedingItem]);

  useEffect(() => {
    loadId.current = { slug: selectedSlug, linked: !!root?.linked };
    const next = treeDialogsAfterRootChange();
    setPickerOpen(next.pickerOpen);
    setUntrackTarget(next.untrackTarget);
    setFiles(next.files);
    setEntries(next.listed);
    setConflictSet(new Set());
    setQuery("");
  }, [selectedSlug, root?.linked]);

  const tree = useMemo(() => buildTree(files), [files]);
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

  return (
    <div className="flex h-full min-h-0 min-w-0 flex-col">
      <header className="flex h-row min-w-0 shrink-0 items-center gap-2 border-b border-border px-3">
        <span className="min-w-0 flex-1 truncate text-foreground" title={root.name}>
          {root.name}
        </span>
        <div className="flex shrink-0 items-center gap-1">
          <TrackedSummary files={files} />
          <div className="flex items-center gap-0.5">
            <Tooltip>
              <TooltipTrigger
                render={
                  <Button
                    variant="ghost"
                    size="icon-xs"
                    className="text-muted-foreground"
                    disabled={busy || !root.linked}
                    aria-label="Add to track"
                    onClick={() => setPickerOpen(true)}
                  />
                }
              >
                <Plus aria-hidden />
              </TooltipTrigger>
              <TooltipContent>Add to track</TooltipContent>
            </Tooltip>
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
          </div>
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
      <ScrollArea className="min-h-0 flex-1">
        <div className="flex flex-col gap-0.5 px-1.5 py-1">
        {filtering && visibleTree.length === 0 ? (
          <p className="px-2 py-1.5 text-sm text-muted-foreground">No matches</p>
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
      </ScrollArea>
      {root.linked ? (
        <>
          <EntryPickerDialog
            key={root.slug}
            slug={root.slug}
            entries={entries}
            open={pickerOpen}
            onOpenChange={setPickerOpen}
            onMutated={refetch}
          />
          <UntrackEntryDialog
            key={`untrack-${root.slug}`}
            slug={root.slug}
            entry={untrackTarget}
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
