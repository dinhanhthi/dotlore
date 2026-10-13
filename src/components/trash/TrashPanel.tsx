import { ArchiveRestore, Info, Trash2 } from "lucide-react";
import { useCallback, useEffect, useState, type ComponentProps, type ReactNode } from "react";

import { RevealInFinderButton } from "@/components/layout/RevealInFinderButton";
import { FilePane } from "@/components/main/FileViewer";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { errorMessage } from "@/lib/errors";
import {
  BLOCKED,
  deleteTrash,
  emptyTrash,
  listTrash,
  readTrashFile,
  reportError,
  restoreTrash,
} from "@/lib/ipc";
import { composeLivePath } from "@/lib/path";
import { useRoots } from "@/lib/roots";
import { canRestore, filesUnder, passLabel } from "@/lib/trash";
import type {
  FileContent,
  RestoreChoice,
  RestoreResult,
  TrashPass,
  TrashProject,
} from "@/lib/types";

import { TrashConfirmAlert } from "./TrashConfirmAlert";
import { TrashRestoreDialog } from "./TrashRestoreDialog";
import { TrashTree, type TrashSelection } from "./TrashTree";

const NOTICE = "Everything here is on this Mac only and never synced.";
const REMOVED_NOTE =
  "This project was removed from Dotlore, so its files cannot be restored here. Reveal them in Finder to copy them by hand.";

type Located = { project: TrashProject; pass: TrashPass };

function locate(projects: TrashProject[], selection: TrashSelection | null): Located | null {
  if (!selection) return null;
  const project = projects.find((item) => item.slug === selection.slug);
  const pass = project?.passes.find(
    (item) => item.day === selection.day && item.pass === selection.pass,
  );
  if (!project || !pass) return null;
  if (selection.rel !== null && filesUnder(pass.files, selection.rel).length === 0) return null;
  return { project, pass };
}

function plural(n: number, word: string): string {
  return `${n} ${word}${n === 1 ? "" : "s"}`;
}

/** What a finished restore must tell the user, or `null` for a clean success. */
export function restoreNotice(result: Extract<RestoreResult, { kind: "done" }>): string | null {
  const clean = result.skipped.length === 0;
  if (clean && (result.restored.length > 0 || result.unchanged.length > 0)) return null;
  const parts: string[] = [];
  if (result.restored.length > 0) parts.push(`Restored ${plural(result.restored.length, "file")}.`);
  if (result.unchanged.length > 0) {
    parts.push(`${plural(result.unchanged.length, "file")} already matched the project.`);
  }
  if (result.skipped.length > 0) {
    const names = result.skipped.slice(0, 3).join(", ");
    const rest = result.skipped.length > 3 ? ", …" : "";
    parts.push(`Skipped ${result.skipped.length}: ${names}${rest}`);
  }
  return parts.length > 0 ? parts.join(" ") : "Nothing to restore.";
}

/** A button whose tooltip repeats its aria-label. */
function TipButton({
  label,
  children,
  ...props
}: { label: string; children: ReactNode } & ComponentProps<typeof Button>) {
  return (
    <Tooltip>
      <TooltipTrigger render={<Button type="button" aria-label={label} {...props} />}>
        {children}
      </TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
  );
}

function TrashActions({
  restorable,
  revealPath,
  writeDisabled,
  onRestore,
  onDelete,
}: {
  restorable: boolean;
  revealPath: string;
  writeDisabled: boolean;
  onRestore: () => void;
  onDelete: () => void;
}) {
  return (
    <>
      {restorable ? (
        <TipButton
          label="Restore"
          variant="ghost"
          size="icon-xs"
          className="text-muted-foreground"
          disabled={writeDisabled}
          onClick={onRestore}
        >
          <ArchiveRestore className="size-3.5" aria-hidden />
        </TipButton>
      ) : null}
      <TipButton
        label="Delete"
        variant="ghost"
        size="icon-xs"
        className="text-muted-foreground hover:text-destructive"
        disabled={writeDisabled}
        onClick={onDelete}
      >
        <Trash2 className="size-3.5" aria-hidden />
      </TipButton>
      <RevealInFinderButton path={revealPath} />
    </>
  );
}

function Centered({ children }: { children: ReactNode }) {
  return (
    <div className="flex min-h-0 flex-1 items-center justify-center px-6">{children}</div>
  );
}

export type TrashViewProps = {
  /** `null` while the first listing loads. */
  projects: TrashProject[] | null;
  selection: TrashSelection | null;
  onSelect: (selection: TrashSelection) => void;
  /** The selected file's content; `null` while loading. */
  content: FileContent | null;
  error: string | null;
  /** Why the listing failed; shown instead of the tree while `projects` is `null`. */
  listError: string | null;
  onRetry: () => void;
  writeDisabled: boolean;
  onRestore: () => void;
  onDelete: () => void;
  onEmpty: () => void;
};

export function TrashView({
  projects,
  selection,
  onSelect,
  content,
  error,
  listError,
  onRetry,
  writeDisabled,
  onRestore,
  onDelete,
  onEmpty,
}: TrashViewProps) {
  const { roots } = useRoots();
  const nameOf = (slug: string) => roots.find((row) => row.slug === slug)?.name ?? slug;
  const empty = projects !== null && projects.length === 0;
  const located = projects ? locate(projects, selection) : null;

  let detail: ReactNode;
  if (!selection || !located) {
    detail = (
      <Centered>
        <p className="text-center text-muted-foreground">
          Select a file, folder or sync time to see it here.
        </p>
      </Centered>
    );
  } else {
    const { project, pass } = located;
    const restorable = canRestore(project);
    const rel = selection.rel;
    const actions = (
      <TrashActions
        restorable={restorable}
        revealPath={rel === null ? pass.path : composeLivePath(pass.path, rel)}
        writeDisabled={writeDisabled}
        onRestore={onRestore}
        onDelete={onDelete}
      />
    );
    const removedNote = restorable ? null : (
      <p className="text-sm text-muted-foreground">{REMOVED_NOTE}</p>
    );
    if (selection.kind === "file" && rel !== null) {
      detail = (
        <div className="flex h-full min-h-0 flex-col">
          {removedNote ? (
            <div className="shrink-0 border-b border-border px-4 py-2">{removedNote}</div>
          ) : null}
          <div className="min-h-0 flex-1">
            <FilePane rel={rel} content={content} error={error} actions={actions} />
          </div>
        </div>
      );
    } else {
      const count = filesUnder(pass.files, rel).length;
      const path = rel === null ? pass.path : composeLivePath(pass.path, rel);
      detail = (
        <ScrollArea className="h-full">
          <div className="p-6">
            <div className="flex flex-col gap-3 rounded-2xl bg-card px-4 py-3.5 ring-1 ring-foreground/20 dark:ring-foreground/10">
              <div className="flex min-w-0 items-start gap-3">
                <div className="flex min-w-0 flex-1 flex-col gap-1">
                  <span className="truncate font-medium text-foreground">
                    {nameOf(project.slug)}
                  </span>
                  <span className="text-sm text-muted-foreground">
                    {passLabel(pass.day, pass.pass)}
                    {rel !== null ? ` · ${rel}` : ""} · {plural(count, "file")}
                  </span>
                  <span className="truncate font-path text-muted-foreground" title={path}>
                    {path}
                  </span>
                </div>
                <div className="flex shrink-0 items-center gap-0.5">{actions}</div>
              </div>
              {removedNote}
            </div>
          </div>
        </ScrollArea>
      );
    }
  }

  return (
    <div className="flex h-full min-h-0 flex-col">
      <header className="flex shrink-0 items-start gap-4 border-b border-border px-6 py-4">
        <div className="flex min-w-0 flex-1 flex-col gap-2">
          <h1 className="font-heading text-base font-medium text-foreground">Trash</h1>
          <p className="flex items-center gap-2 text-sm text-foreground">
            <Info aria-hidden className="size-4 shrink-0 text-muted-foreground" />
            <span>{NOTICE}</span>
          </p>
        </div>
        <TipButton
          label="Empty trash"
          variant="outline"
          size="sm"
          disabled={projects === null || empty || writeDisabled}
          onClick={onEmpty}
        >
          Empty trash
        </TipButton>
      </header>
      {projects === null ? (
        listError !== null ? (
          <Centered>
            <div className="flex flex-col items-center gap-3 text-center">
              <div className="flex flex-col gap-1">
                <p className="text-foreground">Could not read the trash.</p>
                <p className="text-sm text-muted-foreground">{listError}</p>
              </div>
              <TipButton label="Try again" variant="outline" size="sm" onClick={onRetry}>
                Try again
              </TipButton>
            </div>
          </Centered>
        ) : null
      ) : empty ? (
        <Centered>
          <div className="flex flex-col gap-1 text-center">
            <p className="text-foreground">The trash is empty</p>
            <p className="text-sm text-muted-foreground">
              A copy appears here when a sync deletes or overwrites a file in one of your projects.
            </p>
          </div>
        </Centered>
      ) : (
        <div className="flex min-h-0 flex-1">
          <ScrollArea className="w-72 shrink-0 border-r border-border">
            <div className="p-2">
              <TrashTree
                projects={projects}
                nameOf={nameOf}
                selection={selection}
                onSelect={onSelect}
              />
            </div>
          </ScrollArea>
          <div className="min-w-0 flex-1">{detail}</div>
        </div>
      )}
    </div>
  );
}

type Target = Omit<TrashSelection, "kind">;

function targetOf(selection: TrashSelection): Target {
  const { slug, day, pass, rel } = selection;
  return { slug, day, pass, rel };
}

export function TrashPanel() {
  const { locked, refreshRoots, setBanner } = useRoots();
  const [projects, setProjects] = useState<TrashProject[] | null>(null);
  const [selection, setSelection] = useState<TrashSelection | null>(null);
  const [content, setContent] = useState<FileContent | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [listError, setListError] = useState<string | null>(null);
  const [restoreAsk, setRestoreAsk] = useState<{ target: Target; rels: string[] } | null>(null);
  const [confirm, setConfirm] = useState<
    { kind: "delete"; target: Target } | { kind: "empty" } | null
  >(null);
  const [confirmOpen, setConfirmOpen] = useState(false);

  const reload = useCallback(async () => {
    try {
      const list = await listTrash();
      setProjects(list);
      setListError(null);
      setSelection((current) => (locate(list, current) ? current : null));
    } catch (cause) {
      const message = errorMessage(cause, "Could not list the trash");
      reportError(message);
      setListError(message);
    }
  }, []);

  useEffect(() => {
    void reload();
  }, [reload]);

  const fileKey =
    selection?.kind === "file" && selection.rel !== null
      ? [selection.slug, selection.day, selection.pass, selection.rel]
      : null;
  const [slug, day, pass, rel] = fileKey ?? [];

  useEffect(() => {
    if (slug === undefined || day === undefined || pass === undefined || rel === undefined) {
      return;
    }
    let cancelled = false;
    setContent(null);
    setError(null);
    void readTrashFile(slug, day, pass, rel)
      .then((file) => {
        if (!cancelled) setContent(file);
      })
      .catch((cause: unknown) => {
        if (!cancelled) setError(errorMessage(cause, "Could not read file"));
      });
    return () => {
      cancelled = true;
    };
  }, [slug, day, pass, rel]);

  async function restore(target: Target, choice: RestoreChoice) {
    if (locked) return;
    try {
      const result = await restoreTrash(target.slug, target.day, target.pass, target.rel, choice);
      if (result === BLOCKED) return;
      if (result.kind === "existing") {
        setRestoreAsk({ target, rels: result.rels });
        return;
      }
      const notice = restoreNotice(result);
      if (notice !== null) setBanner(notice);
      await reload();
      await refreshRoots();
    } catch {
      // Banner is set by `runTask()`.
    }
  }

  async function remove(target: Target) {
    try {
      if ((await deleteTrash(target.slug, target.day, target.pass, target.rel)) === BLOCKED) {
        return;
      }
      await reload();
    } catch {
      // Banner is set by `runTask()`.
    }
  }

  async function empty() {
    try {
      if ((await emptyTrash()) === BLOCKED) return;
      await reload();
    } catch {
      // Banner is set by `runTask()`.
    }
  }

  function answer(choice: RestoreChoice) {
    const ask = restoreAsk;
    setRestoreAsk(null);
    if (ask) void restore(ask.target, choice);
  }

  const confirmCopy =
    confirm?.kind === "empty"
      ? {
          title: "Empty the trash?",
          description: "Every copy in the trash on this Mac is deleted. This cannot be undone.",
          actionLabel: "Empty trash",
        }
      : {
          title: "Delete from trash?",
          description: "The selected files are deleted from the trash on this Mac. This cannot be undone.",
          actionLabel: "Delete",
        };

  return (
    <>
      <TrashView
        projects={projects}
        selection={selection}
        onSelect={setSelection}
        content={content}
        error={error}
        listError={listError}
        onRetry={() => void reload()}
        writeDisabled={locked}
        onRestore={() => {
          if (selection) void restore(targetOf(selection), "ask");
        }}
        onDelete={() => {
          if (!selection) return;
          setConfirm({ kind: "delete", target: targetOf(selection) });
          setConfirmOpen(true);
        }}
        onEmpty={() => {
          setConfirm({ kind: "empty" });
          setConfirmOpen(true);
        }}
      />
      <TrashRestoreDialog
        open={restoreAsk !== null}
        rels={restoreAsk?.rels ?? []}
        onCancel={() => setRestoreAsk(null)}
        onReplace={() => answer("replace")}
        onKeepBoth={() => answer("keep_both")}
      />
      <TrashConfirmAlert
        open={confirmOpen}
        onOpenChange={setConfirmOpen}
        {...confirmCopy}
        onConfirm={() => {
          const current = confirm;
          if (current?.kind === "empty") void empty();
          else if (current?.kind === "delete") void remove(current.target);
        }}
      />
    </>
  );
}
