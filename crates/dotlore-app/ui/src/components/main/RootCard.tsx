import { Star } from "lucide-react";

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

function statusLabel(status: RootStatus): string {
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
  const { trackedBySlug, starredSlugs, selectRoot, toggleStar } = useRoots();
  const starred = starredSlugs.includes(row.slug);
  const fileCount = trackedBySlug[row.slug] ?? 0;

  return (
    <div
      className={cn(
        "relative flex flex-col gap-2 rounded-lg border border-border bg-card p-3",
        "hover:border-[#2a2b2f] hover:bg-[#141517]",
      )}
    >
      <button
        type="button"
        onClick={() => selectRoot(row.slug, { focusSidebar: true })}
        className="flex flex-col gap-2 pr-6 text-left"
      >
        <span className="truncate text-foreground">{row.name}</span>
        <span
          className="truncate font-mono text-[11px] text-muted-foreground"
          title={row.path}
        >
          {shortenPath(row.path)}
        </span>
        <span className="flex items-center gap-1.5 text-muted-foreground">
          <span
            aria-hidden
            className={cn("size-2 shrink-0 rounded-full", statusDotClass(row.status.kind))}
          />
          <span>{statusLabel(row.status)}</span>
        </span>
        <span className="tabular-nums text-muted-foreground">
          {fileCount} {fileCount === 1 ? "file" : "files"}
        </span>
      </button>
      <button
        type="button"
        aria-label={starred ? "Unstar" : "Star"}
        aria-pressed={starred}
        onClick={() => toggleStar(row.slug)}
        className={cn(
          "absolute top-2.5 right-2.5 rounded-sm p-0.5 text-muted-foreground hover:text-foreground",
          starred && "text-foreground",
        )}
      >
        <Star
          aria-hidden
          className={cn("size-3.5", starred && "fill-current")}
        />
      </button>
    </div>
  );
}
