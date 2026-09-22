import { FolderOpen, Star, Trash2, Unlink } from "lucide-react";
import type { ReactNode } from "react";

import { Badge } from "@/components/ui/badge";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
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
  linked?: boolean;
  onClick: () => void;
  onConflictClick?: () => void;
  onToggleStar?: () => void;
  onLink?: () => void;
  onRemove?: () => void;
  onReveal?: () => void;
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
  linked,
  onClick,
  onConflictClick,
  onToggleStar,
  onLink,
  onRemove,
  onReveal,
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
      {linked === false && onLink && (
        <button
          type="button"
          aria-label="Link to a local folder"
          disabled={writeDisabled}
          onClick={(event) => {
            event.stopPropagation();
            onLink();
          }}
          className="shrink-0 rounded-full p-0.5 text-muted-foreground hover:text-foreground"
        >
          <Unlink aria-hidden className="size-3.5" />
        </button>
      )}
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
      {onReveal && (
        <HoverOnly>
          <RowAction label="Go to location" onClick={onReveal}>
            <FolderOpen aria-hidden className="size-3.5" />
          </RowAction>
        </HoverOnly>
      )}
      {onRemove && (
        <HoverOnly>
          <RowAction
            label="Remove"
            disabled={writeDisabled}
            onClick={onRemove}
            className="hover:text-destructive"
          >
            <Trash2 aria-hidden className="size-3.5" />
          </RowAction>
        </HoverOnly>
      )}
      {onToggleStar && (
        <HoverOnly visible={starred}>
          <RowAction
            label={starred ? "Unstar" : "Star"}
            pressed={starred}
            onClick={onToggleStar}
            className={starred ? "text-foreground" : undefined}
          >
            <Star
              aria-hidden
              className={cn("size-3.5", starred && "fill-current text-foreground")}
            />
          </RowAction>
        </HoverOnly>
      )}
    </div>
  );

  if (!onRecover) return row;

  return (
    <ContextMenu>
      <ContextMenuTrigger render={<div className="w-full" />}>{row}</ContextMenuTrigger>
      <ContextMenuContent className="min-w-40">
        <ContextMenuItem disabled={writeDisabled} onClick={onRecover}>
          Recover
        </ContextMenuItem>
      </ContextMenuContent>
    </ContextMenu>
  );
}

function HoverOnly({
  visible = false,
  children,
}: {
  visible?: boolean;
  children: ReactNode;
}) {
  return (
    <span
      className={cn(
        "inline-flex",
        !visible && "hidden group-hover:inline-flex group-focus-within:inline-flex",
      )}
    >
      {children}
    </span>
  );
}

function RowAction({
  label,
  pressed,
  disabled,
  onClick,
  className,
  children,
}: {
  label: string;
  pressed?: boolean;
  disabled?: boolean;
  onClick: () => void;
  className?: string;
  children: ReactNode;
}) {
  return (
    <Tooltip>
      <TooltipTrigger
        render={
          <button
            type="button"
            aria-label={label}
            aria-pressed={pressed}
            disabled={disabled}
            onClick={(event) => {
              event.stopPropagation();
              onClick();
            }}
            className={cn(
              "shrink-0 rounded-full p-0.5 text-muted-foreground hover:text-foreground disabled:opacity-50",
              className,
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
