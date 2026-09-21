import { useState } from "react";
import { MoreHorizontal, Star } from "lucide-react";

import { RemoveRootAlert } from "@/components/sidebar/RemoveRootAlert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { recoverRoot } from "@/lib/ipc";
import { useRoots } from "@/lib/roots";
import type { RootRow, RootStatus } from "@/lib/types";
import { cn } from "@/lib/utils";

function statusDotClass(kind: RootStatus["kind"]): string {
  switch (kind) {
    case "Synced":
      return "bg-status-synced";
    case "Conflicts":
      return "bg-status-conflict";
    case "Pending":
      return "bg-status-pending";
    case "RootMissing":
    case "GitMissing":
    case "Error":
      return "bg-status-error";
  }
}

function statusAria(status: RootStatus): string {
  switch (status.kind) {
    case "Synced":
      return "Synced";
    case "Conflicts":
      return status.detail === 1 ? "1 conflict" : `${status.detail} conflicts`;
    case "Pending":
      return "Pending";
    case "RootMissing":
      return "Folder missing";
    case "GitMissing":
      return "git not found";
    case "Error":
      return "Error";
  }
}

/** `~/dir/.../leaf` — keep first dir after home and the last segment. */
export function shortenPath(path: string): string {
  const tilde = path.replace(/^\/Users\/[^/]+/, "~");
  const parts = tilde.split("/").filter((p) => p.length > 0);
  if (parts.length <= 3) return tilde;
  return `${parts[0]}/${parts[1]}/.../${parts[parts.length - 1]}`;
}

type RootCardProps = {
  row: RootRow;
};

export function RootCard({ row }: RootCardProps) {
  const {
    trackedBySlug,
    starredSlugs,
    selectRoot,
    openFirstConflict,
    toggleStar,
    refreshRoots,
    busy,
  } = useRoots();
  const starred = starredSlugs.includes(row.slug);
  const fileCount = trackedBySlug[row.slug] ?? 0;
  const [removeOpen, setRemoveOpen] = useState(false);
  const conflicts = row.status.kind === "Conflicts" ? row.status.detail : 0;

  async function handleRecover() {
    if (busy) return;
    try {
      await recoverRoot(row.slug);
      await refreshRoots();
    } catch {
      // Banner is set by `run()`.
    }
  }

  return (
    <div
      className={cn(
        "flex flex-col overflow-hidden rounded-2xl bg-card ring-1 ring-foreground/20 dark:ring-foreground/10",
        "transition-[box-shadow] duration-[var(--dur-short)] ease-[var(--ease-out)]",
        "hover:ring-foreground/30 dark:hover:ring-foreground/15",
      )}
    >
      <button
        type="button"
        onClick={() => selectRoot(row.slug, { focusSidebar: true })}
        className="flex flex-col gap-2 px-4 py-3.5 text-left"
      >
        <span className="truncate font-medium text-foreground">{row.name}</span>
        <span
          className="truncate font-path text-muted-foreground"
          title={row.path}
        >
          {shortenPath(row.path)}
        </span>
      </button>
      <footer className="flex items-center gap-3 border-t border-border px-4 py-2.5 text-xs">
        <span
          aria-label={statusAria(row.status)}
          className="flex min-w-0 flex-1 items-center gap-2.5 text-muted-foreground"
        >
          {conflicts > 0 ? (
            <button
              type="button"
              aria-label={statusAria(row.status)}
              onClick={() => openFirstConflict(row.slug)}
              className="flex items-center gap-2 hover:text-foreground"
            >
              <span
                aria-hidden
                className={cn(
                  "size-2 shrink-0 rounded-full",
                  statusDotClass(row.status.kind),
                )}
              />
              <Badge
                variant="secondary"
                className="h-5 min-w-5 px-1.5 text-xs tabular-nums"
              >
                {conflicts}
              </Badge>
            </button>
          ) : (
            <span
              aria-hidden
              className={cn(
                "size-2 shrink-0 rounded-full",
                statusDotClass(row.status.kind),
              )}
            />
          )}
          <span className="tabular-nums">
            {fileCount} {fileCount === 1 ? "file" : "files"}
          </span>
          {row.is_agent ? (
            <span className="text-muted-foreground/80">Agent</span>
          ) : null}
        </span>
        <div className="flex shrink-0 items-center gap-0.5">
          <button
            type="button"
            aria-label={starred ? "Unstar" : "Star"}
            aria-pressed={starred}
            onClick={() => toggleStar(row.slug)}
            className={cn(
              "rounded-md p-1.5 text-muted-foreground transition-colors duration-[var(--dur-short)] ease-[var(--ease-out)] hover:text-foreground",
              starred && "text-foreground",
            )}
          >
            <Star
              aria-hidden
              className={cn("size-3", starred && "fill-current")}
            />
          </button>
          <DropdownMenu>
            <DropdownMenuTrigger
              render={
                <Button
                  variant="ghost"
                  size="icon-xs"
                  aria-label="Root actions"
                  className="text-muted-foreground hover:text-foreground"
                />
              }
            >
              <MoreHorizontal aria-hidden />
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="min-w-40">
              {row.status.kind === "Error" && (
                <DropdownMenuItem
                  disabled={busy}
                  onClick={() => {
                    void handleRecover();
                  }}
                >
                  Recover
                </DropdownMenuItem>
              )}
              <DropdownMenuItem
                variant="destructive"
                disabled={busy}
                onClick={() => setRemoveOpen(true)}
              >
                Remove from Dotlore
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
      </footer>
      <RemoveRootAlert
        slug={row.slug}
        name={row.name}
        open={removeOpen}
        onOpenChange={setRemoveOpen}
      />
    </div>
  );
}
