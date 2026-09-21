import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { Plus, RefreshCw } from "lucide-react";

import {
  EntryPickerDialog,
  UntrackEntryDialog,
} from "@/components/tree/EntryPickerDialog";
import { TreeNode } from "@/components/tree/TreeNode";
import { Button } from "@/components/ui/button";
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
import { useRoots } from "@/lib/roots";
import { buildTree } from "@/lib/tree";
import type { ConflictView, EntryView, TrackedFile } from "@/lib/types";

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

export function FileTree() {
  const { roots, selectedSlug, selectedRel, selectFile, openResolver, busy } =
    useRoots();
  const root = roots.find((row) => row.slug === selectedSlug) ?? null;

  const [files, setFiles] = useState<TrackedFile[]>([]);
  const [entries, setEntries] = useState<EntryView[]>([]);
  const [conflictSet, setConflictSet] = useState<Set<string>>(() => new Set());
  const [maxFileBytes, setMaxFileBytes] = useState(DEFAULT_MAX_FILE_BYTES);
  const [openBySlug, setOpenBySlug] = useState<Record<string, Record<string, boolean>>>(
    {},
  );
  const [pickerOpen, setPickerOpen] = useState(false);
  const [untrackTarget, setUntrackTarget] = useState<EntryView | null>(null);
  const loadId = useRef({ slug: selectedSlug, linked: !!root?.linked });

  const statusKey = root ? JSON.stringify(root.status) : "";

  const loadTree = useCallback(async () => {
    if (!selectedSlug || !root?.linked) {
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
  }, [selectedSlug, root?.linked]);

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
    const started = { slug: selectedSlug, linked: !!root?.linked };
    void (async () => {
      const next = await loadTree();
      if (!treeLoadMatches(loadId.current, started)) return;
      applyTree(next);
    })();
  }, [loadTree, applyTree, statusKey, selectedSlug, root?.linked]);

  useEffect(() => {
    loadId.current = { slug: selectedSlug, linked: !!root?.linked };
    const next = treeDialogsAfterRootChange();
    setPickerOpen(next.pickerOpen);
    setUntrackTarget(next.untrackTarget);
    setFiles(next.files);
    setEntries(next.listed);
    setConflictSet(new Set());
  }, [selectedSlug, root?.linked]);

  const tree = useMemo(() => buildTree(files), [files]);

  const isOpen = useCallback(
    (path: string, depth: number): boolean => {
      if (!selectedSlug) return false;
      const explicit = openBySlug[selectedSlug]?.[path];
      if (explicit !== undefined) return explicit;
      return depth === 0;
    },
    [openBySlug, selectedSlug],
  );

  const onToggle = useCallback(
    (path: string, depth: number) => {
      if (!selectedSlug) return;
      const next = !isOpen(path, depth);
      setOpenBySlug((current) => ({
        ...current,
        [selectedSlug]: { ...current[selectedSlug], [path]: next },
      }));
    },
    [isOpen, selectedSlug],
  );

  if (!root) {
    return <div className="h-full" />;
  }

  return (
    <div className="flex h-full min-h-0 flex-col">
      <header className="flex h-row shrink-0 items-center gap-3 border-b border-border px-3">
        <span className="min-w-0 flex-1 truncate text-foreground">{root.name}</span>
        <span className="shrink-0 tabular-nums text-xs text-muted-foreground">
          {files.length} {files.length === 1 ? "file" : "files"}
        </span>
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
                disabled={busy || !root.linked}
                aria-label="Sync now"
                onClick={() => {
                  void syncNow().catch(() => {
                    // Banner is set by `run()`.
                  });
                }}
              />
            }
          >
            <RefreshCw aria-hidden />
          </TooltipTrigger>
          <TooltipContent>Sync now</TooltipContent>
        </Tooltip>
      </header>
      <ScrollArea className="min-h-0 flex-1">
        <div className="flex flex-col gap-0.5 px-1.5 py-1">
        {tree.map((node) => (
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
