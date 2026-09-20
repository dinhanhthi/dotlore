import { Star } from "lucide-react";
import type { MouseEvent, ReactNode } from "react";

import { Badge } from "@/components/ui/badge";
import type { RootStatus } from "@/lib/types";
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

type SidebarItemProps = {
  id?: string;
  label: string;
  title?: string;
  selected?: boolean;
  statusKind?: RootStatus["kind"];
  leading?: ReactNode;
  conflictCount?: number;
  starred?: boolean;
  onClick: () => void;
  onToggleStar?: () => void;
};

export function SidebarItem({
  id,
  label,
  title,
  selected = false,
  statusKind,
  leading,
  conflictCount = 0,
  starred = false,
  onClick,
  onToggleStar,
}: SidebarItemProps) {
  function handleContextMenu(event: MouseEvent) {
    if (!onToggleStar) return;
    event.preventDefault();
    onToggleStar();
  }

  return (
    <div
      id={id}
      className={cn(
        "group relative flex h-row w-full items-center gap-1.5 px-pad-x",
        selected && "bg-white/[0.06]",
      )}
      onContextMenu={handleContextMenu}
    >
      {selected && (
        <span aria-hidden className="absolute inset-y-0 left-0 w-0.5 bg-primary" />
      )}
      <button
        type="button"
        title={title}
        onClick={onClick}
        className="flex min-w-0 flex-1 items-center gap-2 text-left text-sidebar-foreground"
      >
        {statusKind ? (
          <span
            aria-hidden
            className={cn("size-2 shrink-0 rounded-full", statusDotClass(statusKind))}
          />
        ) : (
          leading
        )}
        <span className="min-w-0 flex-1 truncate">{label}</span>
        {conflictCount > 0 && (
          <Badge
            variant="secondary"
            className="h-4 min-w-4 px-1 text-[10px] tabular-nums"
          >
            {conflictCount}
          </Badge>
        )}
      </button>
      {onToggleStar && (
        <button
          type="button"
          aria-label={starred ? "Unstar" : "Star"}
          aria-pressed={starred}
          onClick={(event) => {
            event.stopPropagation();
            onToggleStar();
          }}
          className={cn(
            "shrink-0 rounded-sm p-0.5 text-muted-foreground hover:text-foreground",
            starred
              ? "opacity-100"
              : "opacity-0 group-hover:opacity-100 focus-visible:opacity-100",
          )}
        >
          <Star
            aria-hidden
            className={cn("size-3", starred && "fill-current text-foreground")}
          />
        </button>
      )}
    </div>
  );
}
