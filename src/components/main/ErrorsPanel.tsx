import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { X } from "lucide-react";

import { shortenPath } from "@/components/main/RootCard";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import {
  BLOCKED,
  clearErrors,
  dismissError,
  recoverRoot,
  syncNow,
  type ErrorLogEntry,
} from "@/lib/ipc";
import { compareRoots } from "@/lib/order";
import { hasError, useRoots, useSyncing } from "@/lib/roots";
import type { RootRow } from "@/lib/types";

/** What went wrong, in the engine's own words where it has them. */
function errorTitle(row: RootRow): string {
  switch (row.status.kind) {
    case "RootMissing":
      return "Folder missing";
    case "GitMissing":
      return "git not found";
    default:
      return "Sync stopped";
  }
}

function errorRemedy(row: RootRow): string {
  switch (row.status.kind) {
    case "RootMissing":
      return "The project folder was moved or deleted. Put it back at this path, or remove the project and add it again from its new location.";
    case "GitMissing":
      return "Dotlore needs the system git. Install the Xcode Command Line Tools (xcode-select --install), then retry.";
    default:
      return "Do what the message asks, then retry. If this Mac's copy is out of date or damaged, for example after another Mac wiped the cloud data, Recover rebuilds it from the cloud.";
  }
}

function ErrorItem({ row }: { row: RootRow }) {
  const { selectRoot, refreshRoots, locked, seeding } = useRoots();
  const syncing = useSyncing();
  const writeDisabled = locked || seeding.some((item) => item.slug === row.slug);

  async function handleRecover() {
    if (writeDisabled) return;
    try {
      if ((await recoverRoot(row.slug)) === BLOCKED) return;
      await refreshRoots();
    } catch {
      // Banner is set by `runTask()`.
    }
  }

  return (
    <li className="flex flex-col gap-3 rounded-2xl bg-card px-4 py-3.5 ring-1 ring-foreground/20 dark:ring-foreground/10">
      <div className="flex min-w-0 flex-col gap-1">
        <span className="flex min-w-0 items-center gap-2">
          <span aria-hidden className="size-2.5 shrink-0 rounded-full bg-status-error" />
          <span className="truncate font-medium text-foreground">{row.name}</span>
          {row.is_agent ? (
            <Badge variant="secondary" className="shrink-0 font-normal">
              Agent
            </Badge>
          ) : null}
          <span className="shrink-0 text-sm text-status-error">{errorTitle(row)}</span>
        </span>
        <span className="truncate font-path text-muted-foreground" title={row.path}>
          {shortenPath(row.path)}
        </span>
      </div>
      {row.status.kind === "Error" && (
        <p className="rounded-lg bg-muted px-3 py-2 font-path text-xs break-words whitespace-pre-wrap text-foreground select-text">
          {row.status.detail}
        </p>
      )}
      <p className="text-sm text-muted-foreground">{errorRemedy(row)}</p>
      <div className="flex flex-wrap gap-2">
        <Button variant="outline" size="sm" onClick={() => selectRoot(row.slug)}>
          Open project
        </Button>
        {row.status.kind !== "RootMissing" && (
          <Button
            variant="outline"
            size="sm"
            onClick={() => {
              void revealItemInDir(row.path).catch(() => {
                // Path missing or the file manager is unavailable.
              });
            }}
          >
            Go to location
          </Button>
        )}
        <Button
          variant="outline"
          size="sm"
          disabled={writeDisabled || syncing}
          onClick={() => {
            void syncNow().catch(() => {
              // Banner is set by `run()`.
            });
          }}
        >
          Retry sync
        </Button>
        {row.status.kind === "Error" && (
          <Button
            variant="outline"
            size="sm"
            disabled={writeDisabled}
            onClick={() => void handleRecover()}
          >
            Recover
          </Button>
        )}
      </div>
    </li>
  );
}

function CommandErrorItem({ entry }: { entry: ErrorLogEntry }) {
  return (
    <li className="flex items-start gap-3 rounded-2xl bg-card px-4 py-3.5 ring-1 ring-foreground/20 dark:ring-foreground/10">
      <div className="flex min-w-0 flex-1 flex-col gap-1">
        <p className="font-path text-xs break-words whitespace-pre-wrap text-foreground select-text">
          {entry.message}
        </p>
        <time
          className="text-sm text-muted-foreground"
          dateTime={new Date(entry.at).toISOString()}
        >
          {new Date(entry.at).toLocaleTimeString()}
        </time>
      </div>
      <Button
        variant="ghost"
        size="icon-xs"
        className="shrink-0 text-muted-foreground"
        aria-label="Dismiss"
        onClick={() => dismissError(entry.id)}
      >
        <X aria-hidden />
      </Button>
    </li>
  );
}

export function ErrorsPanel() {
  const { roots, commandErrors } = useRoots();
  const rows = roots.filter(hasError).sort(compareRoots);

  if (rows.length === 0 && commandErrors.length === 0) {
    return (
      <div className="flex h-full items-center justify-center px-6">
        <p className="text-center text-muted-foreground">No errors.</p>
      </div>
    );
  }

  return (
    <ScrollArea className="h-full">
      <div className="flex flex-col gap-6 p-6">
        {rows.length > 0 && (
          <section className="flex flex-col gap-3">
            <h2 className="text-label text-muted-foreground">Projects</h2>
            <ul className="flex flex-col gap-3">
              {rows.map((row) => (
                <ErrorItem key={row.slug} row={row} />
              ))}
            </ul>
          </section>
        )}
        {commandErrors.length > 0 && (
          <section className="flex flex-col gap-3">
            <div className="flex items-center justify-between gap-2">
              <h2 className="text-label text-muted-foreground">Recent errors</h2>
              <Button variant="ghost" size="xs" onClick={() => clearErrors()}>
                Clear all
              </Button>
            </div>
            <ul className="flex flex-col gap-3">
              {commandErrors.map((entry) => (
                <CommandErrorItem key={entry.id} entry={entry} />
              ))}
            </ul>
          </section>
        )}
      </div>
    </ScrollArea>
  );
}
