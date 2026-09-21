import { Star } from "lucide-react";
import type { ReactNode } from "react";

import { Badge } from "@/components/ui/badge";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
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
  onConflictClick?: () => void;
  onToggleStar?: () => void;
  onRemove?: () => void;
  onRecover?: () => void;
  writeDisabled?: boolean;
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
  onConflictClick,
  onToggleStar,
  onRemove,
  onRecover,
  writeDisabled = false,
}: SidebarItemProps) {
  const row = (
    <div
      id={id}
      className={cn(
        "group relative flex h-8 w-full items-center gap-2 rounded-2xl px-2 mb-2",
        "transition-colors duration-[var(--dur-short)] ease-[var(--ease-out)]",
        "hover:bg-sidebar-accent/80",
        selected && "bg-sidebar-accent",
      )}
    >
      <button
        type="button"
        title={title}
        onClick={onClick}
        className="flex min-w-0 flex-1 items-center gap-2 text-left text-sidebar-foreground"
      >
        {statusKind ? (
          <span
            aria-hidden
            className={cn("size-2.5 shrink-0 rounded-full", statusDotClass(statusKind))}
          />
        ) : (
          leading
        )}
        <span className="min-w-0 flex-1 truncate text-sm">{label}</span>
      </button>
      {conflictCount > 0 && (
        <button
          type="button"
          aria-label={`${conflictCount} ${conflictCount === 1 ? "conflict" : "conflicts"}`}
          onClick={(event) => {
            event.stopPropagation();
            (onConflictClick ?? onClick)();
          }}
          className="shrink-0"
        >
          <Badge
            variant="secondary"
            className="h-5 min-w-5 px-1.5 text-xs tabular-nums"
          >
            {conflictCount}
          </Badge>
        </button>
      )}
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
            "shrink-0 rounded-full p-0.5 text-muted-foreground hover:text-foreground",
            starred
              ? "opacity-100"
              : "opacity-0 group-hover:opacity-100 focus-visible:opacity-100",
          )}
        >
          <Star
            aria-hidden
            className={cn("size-3.5", starred && "fill-current text-foreground")}
          />
        </button>
      )}
    </div>
  );

  if (!onRemove) return row;

  return (
    <ContextMenu>
      <ContextMenuTrigger render={<div className="w-full" />}>{row}</ContextMenuTrigger>
      <ContextMenuContent className="min-w-40">
        {onRecover && (
          <ContextMenuItem disabled={writeDisabled} onClick={onRecover}>
            Recover
          </ContextMenuItem>
        )}
        <ContextMenuItem
          variant="destructive"
          disabled={writeDisabled}
          onClick={onRemove}
        >
          Remove from Dotlore
        </ContextMenuItem>
      </ContextMenuContent>
    </ContextMenu>
  );
}
