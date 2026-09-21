import { useEffect, useState } from "react";
import { ChevronRight } from "lucide-react";

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
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { ScrollArea } from "@/components/ui/scroll-area";
import { errorMessage } from "@/lib/errors";
import {
  inspectEntry,
  listEntryChildren,
  trackEntry,
  untrackEntry,
} from "@/lib/ipc";
import { useRoots } from "@/lib/roots";
import type { EntryView, InspectedEntryDto, PickerRow } from "@/lib/types";
import { cn } from "@/lib/utils";

import {
  fileTooLarge,
  formatBytes,
  parentRel,
  untrackCopy,
} from "./entries";

type EntryPickerDialogProps = {
  slug: string;
  entries: EntryView[];
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onMutated: () => void;
};

/** Reset browse + nested untrack whenever the dialog closes or the slug changes. */
export function pickerStateAfterIdentityChange(): {
  rel: string;
  children: PickerRow[];
  selected: PickerRow | null;
  preview: InspectedEntryDto | null;
  untrackTarget: EntryView | null;
} {
  return {
    rel: "",
    children: [],
    selected: null,
    preview: null,
    untrackTarget: null,
  };
}

export function EntryPickerDialog({
  slug,
  entries,
  open,
  onOpenChange,
  onMutated,
}: EntryPickerDialogProps) {
  const { busy, setBanner } = useRoots();
  const [rel, setRel] = useState("");
  const [children, setChildren] = useState<PickerRow[]>([]);
  const [selected, setSelected] = useState<PickerRow | null>(null);
  const [preview, setPreview] = useState<InspectedEntryDto | null>(null);
  const [untrackTarget, setUntrackTarget] = useState<EntryView | null>(null);

  useEffect(() => {
    const next = pickerStateAfterIdentityChange();
    setRel(next.rel);
    setChildren(next.children);
    setSelected(next.selected);
    setPreview(next.preview);
    setUntrackTarget(next.untrackTarget);
  }, [open, slug]);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    void listEntryChildren(slug, rel)
      .then((rows) => {
        if (cancelled) return;
        setChildren(
          rows.filter((row) => row.kind === "file" || row.kind === "directory"),
        );
      })
      .catch((err) => {
        if (cancelled) return;
        setBanner(errorMessage(err, "Something went wrong"));
      });
    return () => {
      cancelled = true;
    };
  }, [open, slug, rel, setBanner]);

  const parent = parentRel(rel);
  const oversizedFile = preview !== null && fileTooLarge(preview);

  async function selectRow(row: PickerRow) {
    setSelected(row);
    try {
      setPreview(await inspectEntry(slug, row.rel));
    } catch (err) {
      setPreview(null);
      setBanner(errorMessage(err, "Something went wrong"));
    }
  }

  async function submit() {
    if (!selected || preview === null || oversizedFile || busy) return;
    const confirmed =
      preview.kind === "directory" && preview.confirmation_required
        ? preview.bytes
        : null;
    try {
      const result = await trackEntry(slug, selected.rel, confirmed);
      if (result.outcome === "needs_confirmation") {
        setPreview({
          kind: "directory",
          bytes: result.bytes,
          folder_limit: result.folder_limit,
          confirmation_required: true,
          skipped_too_large: result.skipped_too_large,
        });
        return;
      }
      setSelected(null);
      setPreview(null);
      onMutated();
    } catch {
      // Banner is set by `run()`.
    }
  }

  return (
    <>
      <Dialog open={open} onOpenChange={onOpenChange}>
        <DialogContent className="sm:max-w-lg">
          <DialogHeader>
            <DialogTitle>Add to track</DialogTitle>
            <DialogDescription>
              Choose a file or folder inside this project. The project root is
              fixed.
            </DialogDescription>
          </DialogHeader>
          <div className="flex flex-col gap-3">
            <div className="flex items-center gap-2">
              {parent !== null ? (
                <Button
                  type="button"
                  variant="outline"
                  size="xs"
                  onClick={() => {
                    setRel(parent);
                    setSelected(null);
                    setPreview(null);
                  }}
                >
                  Parent
                </Button>
              ) : null}
              <span className="min-w-0 truncate font-mono text-xs text-muted-foreground">
                {rel === "" ? "/" : rel}
              </span>
            </div>
            <ScrollArea className="h-44 rounded-2xl border border-border">
              <ul className="flex flex-col p-1">
                {children.map((row) => (
                  <li key={row.rel} className="flex items-center">
                    <button
                      type="button"
                      onClick={() => {
                        void selectRow(row);
                      }}
                      className={cn(
                        "flex min-w-0 flex-1 items-center rounded-xl px-2 py-1.5 text-left text-sm",
                        selected?.rel === row.rel
                          ? "bg-muted"
                          : "hover:bg-muted/70",
                      )}
                    >
                      <span className="min-w-0 truncate">{row.name}</span>
                      <span className="ml-auto pl-2 text-xs text-muted-foreground">
                        {row.kind === "directory" ? "folder" : "file"}
                      </span>
                    </button>
                    {row.kind === "directory" ? (
                      <Button
                        type="button"
                        variant="ghost"
                        size="icon-xs"
                        aria-label={`Open ${row.name}`}
                        className="text-muted-foreground"
                        onClick={() => {
                          setRel(row.rel);
                          setSelected(null);
                          setPreview(null);
                        }}
                      >
                        <ChevronRight aria-hidden />
                      </Button>
                    ) : null}
                  </li>
                ))}
              </ul>
            </ScrollArea>
            {preview && selected ? (
              <PreviewBlock
                selected={selected}
                preview={preview}
                oversizedFile={oversizedFile}
              />
            ) : null}
            {entries.length > 0 ? (
              <div className="flex flex-col gap-1.5">
                <p className="text-label text-muted-foreground">
                  Tracked entries
                </p>
                <ul className="flex flex-col gap-1">
                  {entries.map((entry) => (
                    <li
                      key={entry.key}
                      className="flex items-center gap-2 rounded-xl px-2 py-1"
                    >
                      <span className="min-w-0 flex-1 truncate font-mono text-xs">
                        {entry.key}
                      </span>
                      <Button
                        type="button"
                        variant="ghost"
                        size="xs"
                        className="text-destructive"
                        disabled={busy}
                        onClick={() => setUntrackTarget(entry)}
                      >
                        Untrack
                      </Button>
                    </li>
                  ))}
                </ul>
              </div>
            ) : null}
          </div>
          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              disabled={busy}
              onClick={() => onOpenChange(false)}
            >
              Cancel
            </Button>
            <Button
              type="button"
              disabled={busy || selected === null || preview === null || oversizedFile}
              onClick={() => {
                void submit();
              }}
            >
              {preview?.confirmation_required ? "Confirm and add" : "Add"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      <UntrackEntryDialog
        slug={slug}
        entry={untrackTarget}
        open={untrackTarget !== null}
        onOpenChange={(next) => {
          if (!next) setUntrackTarget(null);
        }}
        onMutated={onMutated}
      />
    </>
  );
}

function PreviewBlock({
  selected,
  preview,
  oversizedFile,
}: {
  selected: PickerRow;
  preview: InspectedEntryDto;
  oversizedFile: boolean;
}) {
  const size = oversizedFile
    ? (preview.skipped_too_large[0]?.bytes ?? preview.bytes)
    : preview.bytes;
  return (
    <div className="flex flex-col gap-1 text-sm">
      <p>
        <span className="font-mono">{selected.rel}</span>
        {" — "}
        {formatBytes(size)}
        {preview.kind === "directory" ? (
          <span className="text-muted-foreground">
            {" "}
            (limit {formatBytes(preview.folder_limit)})
          </span>
        ) : null}
      </p>
      {oversizedFile ? (
        <p className="text-destructive">
          This file exceeds the per-file limit and cannot be added.
        </p>
      ) : null}
      {preview.confirmation_required && !oversizedFile ? (
        <p>
          This folder is over the add limit. Confirm to track it. Confirmation
          is checked again if the folder grows.
        </p>
      ) : null}
    </div>
  );
}

type UntrackEntryDialogProps = {
  slug: string;
  entry: EntryView | null;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onMutated: () => void;
};

export function UntrackEntryDialog({
  slug,
  entry,
  open,
  onOpenChange,
  onMutated,
}: UntrackEntryDialogProps) {
  const { busy } = useRoots();

  async function confirm() {
    if (entry === null || busy) return;
    try {
      await untrackEntry(slug, entry.key.replace(/\/$/, ""));
      onMutated();
      onOpenChange(false);
    } catch {
      // Banner is set by `run()`.
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
            {entry ? untrackCopy(entry) : ""}
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel disabled={busy}>Cancel</AlertDialogCancel>
          <AlertDialogAction
            variant="destructive"
            disabled={busy || entry === null}
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
