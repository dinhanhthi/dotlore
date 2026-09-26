import { useEffect, useMemo, useRef, useState } from "react";
import { ChevronDown, ChevronRight, Loader2 } from "lucide-react";

import { SearchBar } from "@/components/sidebar/SearchBar";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { errorMessage } from "@/lib/errors";
import {
  answerTrackConfirm,
  applyTrackBatch,
  BLOCKED,
  listEntryChildren,
  untrackEntry,
} from "@/lib/ipc";
import { useRoots, useTrackConfirm } from "@/lib/roots";
import type { EntryView, PickerRow, TrackedFile } from "@/lib/types";
import { cn } from "@/lib/utils";

import { formatBytes, untrackCopy } from "./entries";
import {
  hasTrackedInside,
  isShownTracked,
  orderedPendingOps,
  pickerRowMatchesQuery,
  pickerStateAfterIdentityChange,
  sortPickerRows,
  stagePending,
  type PendingMap,
  type PickerKind,
} from "./picker";

/** Left inset shared with the middle-panel tree. */
const TREE_INSET = "10px";
/**
 * One level: the `size-4` chevron plus the `gap-1.5` before the folder label.
 * A child then starts where its parent's name starts.
 */
const TREE_LEVEL = "calc(1rem + 0.375rem)";

type EntryPickerDialogProps = {
  slug: string;
  entries: EntryView[];
  files: TrackedFile[];
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onMutated: () => void;
};

function asKind(kind: string): PickerKind {
  return kind === "directory" ? "directory" : "file";
}

export function EntryPickerDialog({
  slug,
  entries,
  files,
  open,
  onOpenChange,
  onMutated,
}: EntryPickerDialogProps) {
  const { setBanner } = useRoots();
  const trackedRels = useMemo(
    () => new Set(files.map((file) => file.rel)),
    [files],
  );
  const [pending, setPending] = useState<PendingMap>({});
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});
  const [childrenByRel, setChildrenByRel] = useState<Record<string, PickerRow[]>>(
    {},
  );
  const [loadingRel, setLoadingRel] = useState<Record<string, boolean>>({});
  const [query, setQuery] = useState("");
  const requestGen = useRef(0);
  const applying = useRef(false);

  useEffect(() => {
    const next = pickerStateAfterIdentityChange();
    setPending(next.pending);
    setExpanded(next.expanded);
    setChildrenByRel({});
    setLoadingRel({});
    setQuery("");
    applying.current = false;
    if (!open) {
      requestGen.current += 1;
      return;
    }
    const gen = ++requestGen.current;
    let cancelled = false;
    void listEntryChildren(slug, "")
      .then((rows) => {
        if (cancelled || gen !== requestGen.current) return;
        setChildrenByRel({ "": sortPickerRows(rows) });
      })
      .catch((err) => {
        if (cancelled || gen !== requestGen.current) return;
        setChildrenByRel({ "": [] });
        setBanner(errorMessage(err, "Something went wrong"));
      });
    return () => {
      cancelled = true;
    };
  }, [open, slug, setBanner]);

  function handleOpenChange(next: boolean) {
    if (!next) requestGen.current += 1;
    onOpenChange(next);
  }

  function stage(rel: string, kind: PickerKind, action: "track" | "untrack") {
    setPending((current) =>
      stagePending(current, rel, kind, action, entries, trackedRels),
    );
  }

  function toggle(rel: string) {
    const opening = !expanded[rel];
    setExpanded((current) => ({ ...current, [rel]: opening }));
    if (!opening || childrenByRel[rel]) return;
    const gen = requestGen.current;
    setLoadingRel((current) => ({ ...current, [rel]: true }));
    void listEntryChildren(slug, rel)
      .then((rows) => {
        if (gen !== requestGen.current) return;
        setChildrenByRel((current) => ({
          ...current,
          [rel]: sortPickerRows(rows),
        }));
      })
      .catch((err) => {
        if (gen !== requestGen.current) return;
        setBanner(errorMessage(err, "Something went wrong"));
        setExpanded((current) => ({ ...current, [rel]: false }));
      })
      .finally(() => {
        if (gen !== requestGen.current) return;
        setLoadingRel((current) => ({ ...current, [rel]: false }));
      });
  }

  function apply() {
    if (applying.current) return;
    const ops = orderedPendingOps(pending);
    if (ops.length === 0) return;
    applying.current = true;
    onOpenChange(false);
    void applyTrackBatch(
      slug,
      ops.map((op) => ({ rel: op.rel, action: op.action })),
    ).finally(() => {
      onMutated();
    });
  }

  const rootRows = childrenByRel[""];
  const needle = query.trim().toLowerCase();
  const visibleRows =
    rootRows === undefined
      ? undefined
      : needle
        ? rootRows.filter((row) =>
            pickerRowMatchesQuery(row, needle, expanded, childrenByRel),
          )
        : rootRows;
  const changeCount = Object.keys(pending).length;

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent className="flex h-[min(42rem,calc(100dvh-2rem))] w-full flex-col overflow-hidden sm:max-w-2xl">
        <DialogHeader className="shrink-0 pr-8">
          <DialogTitle>Add to track</DialogTitle>
          <DialogDescription>
            Mark files and folders to track or untrack. Nothing changes until
            you apply.
          </DialogDescription>
        </DialogHeader>
        <div className="flex min-h-0 min-w-0 flex-1 flex-col gap-2">
          <div className="shrink-0">
            <SearchBar
              value={query}
              onChange={setQuery}
              placeholder="Search files"
              label="Search files and folders"
              onKeyDown={(event) => {
                if (event.key === "Escape" && query) {
                  event.preventDefault();
                  event.stopPropagation();
                  setQuery("");
                }
              }}
            />
          </div>
          <div className="min-h-0 w-full min-w-0 flex-1 overflow-x-hidden overflow-y-auto rounded-2xl border border-border">
            {visibleRows === undefined ? (
              <div className="flex h-full items-center justify-center text-muted-foreground">
                <Loader2 className="size-4 animate-spin" aria-hidden />
                <span className="sr-only">Loading files</span>
              </div>
            ) : visibleRows.length === 0 ? (
              <p className="px-3 py-2 text-sm text-muted-foreground">
                {rootRows && rootRows.length > 0
                  ? "No matches"
                  : "This folder is empty."}
              </p>
            ) : (
              <div className="flex w-full min-w-0 flex-col gap-0.5 px-1.5 py-1">
                {visibleRows.map((row) => (
                  <PickerNode
                    key={row.rel}
                    row={row}
                    entries={entries}
                    pending={pending}
                    trackedRels={trackedRels}
                    expanded={expanded}
                    childrenByRel={childrenByRel}
                    loadingRel={loadingRel}
                    needle={needle}
                    onToggle={toggle}
                    onStage={stage}
                  />
                ))}
              </div>
            )}
          </div>
        </div>
        <DialogFooter className="shrink-0">
          <Button
            type="button"
            variant="outline"
            onClick={() => handleOpenChange(false)}
          >
            Cancel
          </Button>
          <Button
            type="button"
            disabled={changeCount === 0}
            onClick={apply}
          >
            Apply
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

type PickerNodeProps = {
  row: PickerRow;
  entries: EntryView[];
  pending: PendingMap;
  trackedRels: ReadonlySet<string>;
  expanded: Record<string, boolean>;
  childrenByRel: Record<string, PickerRow[]>;
  loadingRel: Record<string, boolean>;
  needle: string;
  onToggle: (rel: string) => void;
  onStage: (rel: string, kind: PickerKind, action: "track" | "untrack") => void;
};

function PickerNode({
  row,
  entries,
  pending,
  trackedRels,
  expanded,
  childrenByRel,
  loadingRel,
  needle,
  onToggle,
  onStage,
}: PickerNodeProps) {
  const kind = asKind(row.kind);
  const tracked = isShownTracked(row.rel, kind, entries, pending, trackedRels);
  const partial =
    !tracked &&
    kind === "directory" &&
    hasTrackedInside(row.rel, entries, pending, trackedRels);
  const open = kind === "directory" && expanded[row.rel] === true;
  const loading = loadingRel[row.rel] === true;
  const nested = childrenByRel[row.rel];
  const shownNested =
    nested === undefined
      ? undefined
      : needle
        ? nested.filter((child) =>
            pickerRowMatchesQuery(child, needle, expanded, childrenByRel),
          )
        : nested;

  return (
    <>
      <div
        className={cn(
          "group relative flex h-8 w-full min-w-0 items-center gap-2 rounded-2xl pr-2",
          "transition-colors duration-[var(--dur-short)] ease-[var(--ease-out)]",
          "hover:bg-muted/70",
        )}
        style={{ paddingLeft: TREE_INSET }}
      >
        {kind === "directory" ? (
          <button
            type="button"
            aria-expanded={open}
            aria-label={open ? `Collapse ${row.name}` : `Expand ${row.name}`}
            onClick={() => onToggle(row.rel)}
            className="flex min-w-0 flex-1 items-center gap-1.5 text-left text-foreground"
          >
            {loading ? (
              <Loader2
                aria-hidden
                className="size-4 shrink-0 animate-spin text-muted-foreground"
              />
            ) : open ? (
              <ChevronDown
                aria-hidden
                className="size-4 shrink-0 text-muted-foreground"
              />
            ) : (
              <ChevronRight
                aria-hidden
                className="size-4 shrink-0 text-muted-foreground"
              />
            )}
            <span className="min-w-0 truncate text-sm">{row.name}</span>
          </button>
        ) : (
          <span className="min-w-0 flex-1 truncate text-sm text-foreground">
            {row.name}
          </span>
        )}
        <TrackMark
          name={row.name}
          tracked={tracked}
          partial={partial}
          onClick={() => onStage(row.rel, kind, tracked ? "untrack" : "track")}
        />
      </div>
      {open &&
      (shownNested === undefined || shownNested.length > 0 || !needle) ? (
        <div
          className="relative flex w-full min-w-0 flex-col gap-0.5"
          style={{ paddingLeft: TREE_LEVEL }}
        >
          <span
            aria-hidden
            className="pointer-events-none absolute -top-0.5 bottom-0 z-10 w-[0.5px] -translate-x-1/2 bg-foreground/15"
            style={{ left: `calc(${TREE_INSET} + 0.5rem)` }}
          />
          {shownNested === undefined ? null : shownNested.length === 0 ? (
            <p className="px-2 py-1 text-xs text-muted-foreground">Empty</p>
          ) : (
            shownNested.map((child) => (
              <PickerNode
                key={child.rel}
                row={child}
                entries={entries}
                pending={pending}
                trackedRels={trackedRels}
                expanded={expanded}
                childrenByRel={childrenByRel}
                loadingRel={loadingRel}
                needle={needle}
                onToggle={onToggle}
                onStage={onStage}
              />
            ))
          )}
        </div>
      ) : null}
    </>
  );
}

function TrackMark({
  name,
  tracked,
  partial,
  onClick,
}: {
  name: string;
  tracked: boolean;
  partial: boolean;
  onClick: () => void;
}) {
  return (
    <span className="group/mark ml-auto inline-grid w-[5.5rem] shrink-0 items-center justify-items-end">
      <Badge
        aria-hidden
        variant={partial ? "outline" : "secondary"}
        className={cn(
          "pointer-events-none col-start-1 row-start-1 font-normal transition-opacity",
          tracked || partial
            ? "opacity-100 group-hover:opacity-0 group-focus-within/mark:opacity-0"
            : "opacity-0",
        )}
      >
        {partial ? "has tracked" : "tracked"}
      </Badge>
      <Button
        type="button"
        variant={tracked ? "destructive" : "outline"}
        size="xs"
        aria-label={tracked ? `Untrack ${name}` : `Track ${name}`}
        className={cn(
          "relative z-10 col-start-1 row-start-1 transition-opacity",
          "pointer-events-none opacity-0",
          "group-hover:pointer-events-auto group-hover:opacity-100",
          "group-focus-within/mark:pointer-events-auto group-focus-within/mark:opacity-100",
          "focus-visible:pointer-events-auto focus-visible:opacity-100",
        )}
        onClick={onClick}
      >
        {tracked ? "untrack" : "track"}
      </Button>
    </span>
  );
}

export function SensitivePathList({
  paths,
  more,
}: {
  paths: string[];
  more: boolean;
}) {
  return (
    <>
      <ul className="min-h-0 max-h-40 space-y-1 overflow-y-auto rounded-lg border border-border px-3 py-2 text-xs text-muted-foreground">
        {paths.map((path) => (
          <li key={path} className="break-all">
            {path}
          </li>
        ))}
      </ul>
      {more && (
        <p className="shrink-0 text-xs font-medium text-foreground">
          Plus additional secret files not shown.
        </p>
      )}
    </>
  );
}

/** Paused inside a background track batch. Does not take the global busy lock. */
export function TrackConfirmDialog() {
  const confirm = useTrackConfirm();

  return (
    <AlertDialog
      open={confirm !== null}
      onOpenChange={(next) => {
        if (!next) answerTrackConfirm(false);
      }}
    >
      <AlertDialogContent className="flex max-h-[calc(100dvh-2rem)] flex-col overflow-hidden">
        <AlertDialogHeader className="shrink-0">
          <AlertDialogTitle className="line-clamp-2 break-all">
            Track {confirm?.rel ?? "folder"}?
          </AlertDialogTitle>
          <AlertDialogDescription>
            {confirm?.kind === "sensitive"
              ? "These files may contain secrets. Tracking syncs their contents to your cloud folder."
              : confirm?.kind === "folder_limit"
                ? `${confirm.rel} is ${formatBytes(confirm.bytes)} (limit ${formatBytes(confirm.folderLimit)}). This folder is over the add limit. Confirm to track it.`
                : ""}
          </AlertDialogDescription>
        </AlertDialogHeader>
        {confirm?.kind === "sensitive" && (
          <SensitivePathList paths={confirm.paths} more={confirm.more} />
        )}
        <AlertDialogFooter className="shrink-0">
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <AlertDialogAction
            onClick={(event) => {
              event.preventDefault();
              answerTrackConfirm(true);
            }}
          >
            {confirm?.kind === "sensitive" ? "Track anyway" : "Track"}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}

type UntrackEntryDialogProps = {
  slug: string;
  entry: EntryView | null;
  entries: EntryView[];
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onMutated: () => void;
};

export function UntrackEntryDialog({
  slug,
  entry,
  entries,
  open,
  onOpenChange,
  onMutated,
}: UntrackEntryDialogProps) {
  const { locked } = useRoots();

  async function confirm() {
    if (entry === null || locked) return;
    try {
      onOpenChange(false);
      if ((await untrackEntry(slug, entry.key.replace(/\/$/, ""))) === BLOCKED) {
        return;
      }
      onMutated();
    } catch {
      // Banner is set by `runTask()`.
    }
  }

  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>
            Untrack {entry?.key ?? "entry"}?
          </AlertDialogTitle>
          <AlertDialogDescription>
            {entry ? untrackCopy(entry, entries) : ""}
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel disabled={locked}>Cancel</AlertDialogCancel>
          <AlertDialogAction
            variant="destructive"
            disabled={locked || entry === null}
            onClick={(event) => {
              event.preventDefault();
              void confirm();
            }}
          >
            Untrack
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
