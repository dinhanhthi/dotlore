import { ArchiveRestore, ArrowLeft, Info } from "lucide-react";
import { useEffect, useState, type ComponentProps, type ReactNode } from "react";

import { restoreIncomplete, restoreNotice } from "@/components/trash/TrashPanel";
import { TrashRestoreDialog } from "@/components/trash/TrashRestoreDialog";
import { formatBytes } from "@/components/tree/entries";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { toast } from "@/components/ui/toast";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { errorMessage } from "@/lib/errors";
import {
  BLOCKED,
  deletedFiles,
  fileHistory,
  readHistoryFile,
  reportError,
  restoreHistory,
} from "@/lib/ipc";
import { useRoots } from "@/lib/roots";
import type {
  DeletedFile,
  FileContent,
  HistoryVersion,
  RestoreChoice,
} from "@/lib/types";
import { cn } from "@/lib/utils";

const NOTICE = "History is kept on this Mac only.";

/** "Just now", "N min ago", "N h ago", "N d ago", then the date. */
export function relativeTime(secs: number, now: Date): string {
  const mins = Math.floor((now.getTime() / 1000 - secs) / 60);
  if (mins < 1) return "Just now";
  if (mins < 60) return `${mins} min ago`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours} h ago`;
  const days = Math.floor(hours / 24);
  if (days < 30) return `${days} d ago`;
  return new Date(secs * 1000).toLocaleDateString();
}

function absoluteTime(secs: number): string {
  return new Date(secs * 1000).toLocaleString();
}

export type HistoryTarget = { slug: string; rel: string; commit: string };

/** A deleted file drilled into from a project's list. */
export type OpenedFile = { slug: string; file: string };

/**
 * The file the panel shows: the `rel` prop, else the deleted file opened in
 * the same project. One opened under another slug is stale and ignored.
 */
export function currentRel(
  slug: string,
  rel: string | null,
  opened: OpenedFile | null,
): string | null {
  if (rel !== null) return rel;
  return opened !== null && opened.slug === slug ? opened.file : null;
}

/**
 * Restore `target` with `choice`. An "ask" that finds the file already in
 * the project hands its rels to `onAsk`. Returns whether a restore finished.
 */
export async function restoreVersion(
  target: HistoryTarget,
  choice: RestoreChoice,
  onAsk: (rels: string[]) => void,
  setBanner: (message: string | null) => void,
): Promise<boolean> {
  try {
    const result = await restoreHistory(target.slug, target.rel, target.commit, choice);
    if (result === BLOCKED) return false;
    if (result.kind === "existing") {
      onAsk(result.rels);
      return false;
    }
    const notice = restoreNotice(result);
    if (notice !== null) {
      if (restoreIncomplete(result)) setBanner(notice);
      else toast.add({ title: notice, type: "success", timeout: 8000 });
    }
    return true;
  } catch {
    // Banner is set by `runTask()`.
    return false;
  }
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

/** A selectable list row whose tooltip repeats its aria-label. */
function Row({
  label,
  selected = false,
  onClick,
  children,
}: {
  label: string;
  selected?: boolean;
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <Tooltip>
      <TooltipTrigger
        render={
          <button
            type="button"
            aria-label={label}
            aria-current={selected || undefined}
            onClick={onClick}
            className={cn(
              "flex w-full min-w-0 items-center gap-2 rounded-2xl px-3 py-2 text-left text-foreground",
              "transition-colors duration-[var(--dur-short)] ease-[var(--ease-out)]",
              "hover:bg-muted/70",
              selected && "bg-muted",
            )}
          />
        }
      >
        {children}
      </TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
  );
}

function Centered({ children }: { children: ReactNode }) {
  return (
    <div className="flex min-h-0 flex-1 items-center justify-center px-6">{children}</div>
  );
}

function Message({ children }: { children: ReactNode }) {
  return (
    <Centered>
      <p className="text-center text-muted-foreground">{children}</p>
    </Centered>
  );
}

function versionLabel(version: HistoryVersion, now: Date): string {
  const label = `${relativeTime(version.time, now)} · ${version.device_name}`;
  return version.deleted ? `${label} · Deleted` : label;
}

function Preview({
  version,
  content,
  error,
}: {
  version: HistoryVersion;
  content: FileContent | null;
  error: string | null;
}) {
  if (version.deleted) {
    return <Message>This version deleted the file. There is nothing to restore.</Message>;
  }
  if (error) return <Message>{error}</Message>;
  if (!content) {
    return (
      <Centered>
        <p role="status" className="text-muted-foreground">
          Loading…
        </p>
      </Centered>
    );
  }
  if (content.too_large) {
    return <Message>{`File too large to preview (${content.bytes_len} bytes)`}</Message>;
  }
  if (content.binary) return <Message>{`Binary file — ${content.bytes_len} bytes`}</Message>;
  if (content.text === null) return <Message>Could not preview this file.</Message>;
  return (
    <ScrollArea className="min-h-0 flex-1">
      <pre className="px-6 py-4 font-mono text-xs whitespace-pre-wrap break-words text-foreground select-text">
        {content.text}
      </pre>
    </ScrollArea>
  );
}

export type HistoryViewProps = {
  /** The file whose versions are shown; `null` lists the deleted files. */
  rel: string | null;
  /** `null` while loading, or outside file mode. */
  versions: HistoryVersion[] | null;
  /** `null` while loading, or outside deleted-files mode. */
  files: DeletedFile[] | null;
  /** Why the listing failed; shown instead of the list while it is `null`. */
  listError: string | null;
  onRetry: () => void;
  /** The selected version's commit. */
  selected: string | null;
  onSelect: (commit: string) => void;
  /** A deleted file's row opens its history. */
  onOpenHistory: (rel: string) => void;
  /** The selected version's content; `null` while loading. */
  content: FileContent | null;
  error: string | null;
  writeDisabled: boolean;
  onRestore: () => void;
  /** Files already in the project, while the Replace / Keep both dialog is open. */
  ask: string[] | null;
  /** The dialog's answer; `null` cancels. */
  onAnswer: (choice: Exclude<RestoreChoice, "ask"> | null) => void;
  onBack: () => void;
  /** Clock for the relative times; defaults to the current time. */
  now?: Date;
};

export function HistoryView({
  rel,
  versions,
  files,
  listError,
  onRetry,
  selected,
  onSelect,
  onOpenHistory,
  content,
  error,
  writeDisabled,
  onRestore,
  ask,
  onAnswer,
  onBack,
  now,
}: HistoryViewProps) {
  const at = now ?? new Date();
  const list = rel === null ? files : versions;

  let body: ReactNode;
  if (list === null) {
    body =
      listError !== null ? (
        <Centered>
          <div className="flex flex-col items-center gap-3 text-center">
            <div className="flex flex-col gap-1">
              <p className="text-foreground">Could not read the history.</p>
              <p className="text-sm text-muted-foreground">{listError}</p>
            </div>
            <TipButton label="Try again" variant="outline" size="sm" onClick={onRetry}>
              Try again
            </TipButton>
          </div>
        </Centered>
      ) : (
        <Centered>
          <p role="status" className="text-muted-foreground">
            Loading…
          </p>
        </Centered>
      );
  } else if (list.length === 0) {
    body = (
      <Centered>
        <div className="flex flex-col gap-1 text-center">
          <p className="text-foreground">
            {rel === null ? "No deleted files" : "No earlier versions"}
          </p>
          <p className="text-sm text-muted-foreground">
            {rel === null
              ? "A file appears here when a sync records its deletion."
              : "A version appears here each time a sync records a change to this file."}
          </p>
        </div>
      </Centered>
    );
  } else if (rel === null) {
    body = (
      <ScrollArea className="min-h-0 flex-1">
        <div className="flex flex-col gap-0.5 p-2">
          {(files ?? []).map((file) => (
            <Row
              key={file.rel}
              label={`History of ${file.rel}`}
              onClick={() => onOpenHistory(file.rel)}
            >
              <span className="flex min-w-0 flex-1 flex-col gap-0.5">
                <span className="truncate font-path" title={file.rel}>
                  {file.rel}
                </span>
                <span className="flex min-w-0 gap-1 text-xs text-muted-foreground">
                  <span className="shrink-0" title={absoluteTime(file.deleted_at)}>
                    Deleted {relativeTime(file.deleted_at, at)} ·
                  </span>
                  <span className="truncate" title={file.device_name}>
                    {file.device_name}
                  </span>
                </span>
              </span>
            </Row>
          ))}
        </div>
      </ScrollArea>
    );
  } else {
    const version = (versions ?? []).find((item) => item.commit === selected) ?? null;
    body = (
      <div className="flex min-h-0 flex-1">
        <div className="flex w-72 shrink-0 flex-col border-r border-border">
          <ScrollArea className="min-h-0 flex-1">
            <div className="flex flex-col gap-0.5 p-2">
              {(versions ?? []).map((item) => (
                <Row
                  key={item.commit}
                  label={versionLabel(item, at)}
                  selected={item.commit === selected}
                  onClick={() => onSelect(item.commit)}
                >
                  <span className="flex min-w-0 flex-1 flex-col gap-0.5">
                    <span className="flex min-w-0 items-center gap-2">
                      <span className="truncate" title={absoluteTime(item.time)}>
                        {relativeTime(item.time, at)}
                      </span>
                      {item.deleted ? <Badge variant="secondary">Deleted</Badge> : null}
                    </span>
                    <span className="truncate text-xs text-muted-foreground" title={item.device_name}>
                      {item.device_name}
                    </span>
                  </span>
                </Row>
              ))}
            </div>
          </ScrollArea>
        </div>
        <div className="flex min-h-0 min-w-0 flex-1 flex-col">
          {version === null ? (
            <Message>Select a version to see it here.</Message>
          ) : (
            <>
              <header className="flex h-row shrink-0 items-center gap-3 border-b border-border px-4">
                <span className="min-w-0 flex-1 truncate text-xs" title={version.device_name}>
                  {absoluteTime(version.time)} · {version.device_name}
                </span>
                <span className="shrink-0 tabular-nums text-xs text-muted-foreground">
                  {content && !version.deleted ? formatBytes(content.bytes_len) : ""}
                </span>
                <TipButton
                  label="Restore"
                  variant="ghost"
                  size="icon-xs"
                  className="text-muted-foreground"
                  disabled={writeDisabled || version.deleted}
                  onClick={onRestore}
                >
                  <ArchiveRestore className="size-3.5" aria-hidden />
                </TipButton>
              </header>
              <Preview version={version} content={content} error={error} />
            </>
          )}
        </div>
      </div>
    );
  }

  return (
    <div className="flex h-full min-h-0 flex-col">
      <header className="flex shrink-0 items-start gap-3 border-b border-border px-6 py-4">
        <TipButton label="Back" variant="ghost" size="icon-sm" onClick={onBack}>
          <ArrowLeft aria-hidden />
        </TipButton>
        <div className="flex min-w-0 flex-1 flex-col gap-2">
          <div className="flex min-w-0 items-baseline gap-3">
            <h1 className="shrink-0 font-heading text-base font-medium text-foreground">
              {rel === null ? "Deleted files" : "History"}
            </h1>
            {rel !== null ? (
              <span className="truncate font-path text-xs text-muted-foreground" title={rel}>
                {rel}
              </span>
            ) : null}
          </div>
          <p className="flex items-center gap-2 text-sm text-muted-foreground">
            <Info aria-hidden className="size-4 shrink-0" />
            <span>{NOTICE}</span>
          </p>
        </div>
      </header>
      {body}
      <TrashRestoreDialog
        open={ask !== null}
        rels={ask ?? []}
        title="This file already exists in the project"
        onCancel={() => onAnswer(null)}
        onReplace={() => onAnswer("replace")}
        onKeepBoth={() => onAnswer("keep_both")}
      />
    </div>
  );
}

export type HistoryPanelProps = {
  slug: string;
  /** The file whose versions are shown; `null` lists the project's deleted files. */
  rel: string | null;
  onBack: () => void;
};

export function HistoryPanel({ slug, rel, onBack }: HistoryPanelProps) {
  const { locked, setBanner, refreshRoots, cycleGen } = useRoots();
  /** The deleted file opened from the list when `rel` is `null`. */
  const [opened, setOpened] = useState<OpenedFile | null>(null);
  const current = currentRel(slug, rel, opened);
  const [versions, setVersions] = useState<HistoryVersion[] | null>(null);
  const [files, setFiles] = useState<DeletedFile[] | null>(null);
  const [listError, setListError] = useState<string | null>(null);
  const [retries, setRetries] = useState(0);
  const [selected, setSelected] = useState<string | null>(null);
  const [content, setContent] = useState<FileContent | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [ask, setAsk] = useState<{ target: HistoryTarget; rels: string[] } | null>(null);

  // A new target forgets the drilled-in file, so it cannot come back later.
  useEffect(() => setOpened(null), [slug, rel]);

  // Another file or list: drop what was shown for the previous one.
  useEffect(() => {
    setVersions(null);
    setFiles(null);
    setSelected(null);
    setListError(null);
  }, [slug, current]);

  // A cycle can record new versions; a reload keeps a selection that still exists.
  useEffect(() => {
    let cancelled = false;
    const load =
      current === null
        ? deletedFiles(slug).then((list) => {
            if (!cancelled) setFiles(list);
          })
        : fileHistory(slug, current).then((list) => {
            if (cancelled) return;
            setVersions(list);
            setSelected((prev) =>
              list.some((item) => item.commit === prev)
                ? prev
                : (list.find((item) => !item.deleted)?.commit ?? null),
            );
          });
    load
      .then(() => {
        if (!cancelled) setListError(null);
      })
      .catch((cause: unknown) => {
        if (cancelled) return;
        const message = errorMessage(cause, "Could not read the history");
        reportError(message);
        setListError(message);
      });
    return () => {
      cancelled = true;
    };
  }, [slug, current, cycleGen, retries]);

  const version = versions?.find((item) => item.commit === selected) ?? null;
  const previewCommit = version && !version.deleted ? version.commit : null;

  useEffect(() => {
    if (current === null || previewCommit === null) return;
    let cancelled = false;
    setContent(null);
    setError(null);
    void readHistoryFile(slug, current, previewCommit)
      .then((file) => {
        if (!cancelled) setContent(file);
      })
      .catch((cause: unknown) => {
        if (!cancelled) setError(errorMessage(cause, "Could not read file"));
      });
    return () => {
      cancelled = true;
    };
  }, [slug, current, previewCommit]);

  async function restore(target: HistoryTarget, choice: RestoreChoice) {
    if (locked) return;
    const onAsk = (rels: string[]) => setAsk({ target, rels });
    if (await restoreVersion(target, choice, onAsk, setBanner)) await refreshRoots();
  }

  return (
    <HistoryView
      rel={current}
      versions={versions}
      files={files}
      listError={listError}
      onRetry={() => setRetries((n) => n + 1)}
      selected={selected}
      onSelect={setSelected}
      onOpenHistory={(file) => setOpened({ slug, file })}
      content={content}
      error={error}
      writeDisabled={locked}
      onRestore={() => {
        if (current !== null && previewCommit !== null) {
          void restore({ slug, rel: current, commit: previewCommit }, "ask");
        }
      }}
      ask={ask?.rels ?? null}
      onAnswer={(choice) => {
        const pending = ask;
        setAsk(null);
        if (pending && choice !== null) void restore(pending.target, choice);
      }}
      onBack={() => {
        if (rel === null && current !== null) setOpened(null);
        else onBack();
      }}
    />
  );
}
