import { Loader2, Star, Unlink } from "lucide-react";
import type { ReactNode } from "react";

import { KeepAllMenuItems } from "@/components/tree/KeepAllMenuItems";
import { quickResolveItems } from "@/components/tree/quick-resolve";
import { Badge } from "@/components/ui/badge";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import type { KeepAllChoice } from "@/lib/conflicts";
import type { ConflictView, RootStatus } from "@/lib/types";
import { cn } from "@/lib/utils";

function statusDotClass(kind: RootStatus["kind"]): string {
  switch (kind) {
    case "Synced":
      return "bg-status-synced";
    case "Conflicts":
      return "bg-status-conflict";
    case "Checking":
    case "Pending":
    case "Retrying":
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
  /** A plain count badge, for nav rows. */
  count?: number;
  starred?: boolean;
  linked?: boolean;
  /** A link to a local folder is in progress for this row. */
  linking?: boolean;
  onClick: () => void;
  onConflictClick?: () => void;
  onToggleStar?: () => void;
  onLink?: () => void;
  onRemove?: () => void;
  onReveal?: () => void;
  onRecover?: () => void;
  /** Conflict model for the keep section. `"loading"` is a disabled placeholder. */
  keepAll?: { views: ConflictView[] } | "loading";
  onKeepAll?: (choice: KeepAllChoice) => void;
  keepAllDisabled?: boolean;
  /** Fired when the row context menu opens or closes. */
  onMenuOpenChange?: (open: boolean) => void;
  writeDisabled?: boolean;
  /** Nothing is tracked in this root; the label is shown at lower opacity. */
  dimmed?: boolean;
};

export function SidebarItem({
  id,
  label,
  title,
  selected = false,
  statusKind,
  leading,
  conflictCount = 0,
  count = 0,
  starred = false,
  linked,
  linking = false,
  onClick,
  onConflictClick,
  onToggleStar,
  onLink,
  onRemove,
  onReveal,
  onRecover,
  keepAll,
  onKeepAll,
  keepAllDisabled = false,
  onMenuOpenChange,
  writeDisabled = false,
  dimmed = false,
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
        className={cn(
          "flex min-w-0 flex-1 items-center gap-2 text-left text-sidebar-foreground",
          dimmed && "opacity-50",
        )}
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
      {count > 0 && (
        <Badge variant="secondary" className="h-5 min-w-5 shrink-0 px-1.5 text-xs tabular-nums">
          {count}
        </Badge>
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
      {linked === false && onLink && (
        <button
          type="button"
          aria-label="Link to a local folder"
          aria-busy={linking || undefined}
          disabled={writeDisabled || linking}
          onClick={(event) => {
            event.stopPropagation();
            onLink();
          }}
          className="shrink-0 rounded-full p-0.5 text-muted-foreground hover:text-foreground"
        >
          {linking ? (
            <Loader2 aria-hidden className="size-3.5 animate-spin" />
          ) : (
            <Unlink aria-hidden className="size-3.5" />
          )}
        </button>
      )}
    </div>
  );

  if (!onMenuOpenChange && keepAll === undefined && !onReveal && !onRemove && !onRecover) {
    return row;
  }

  const keepMenu =
    keepAll !== undefined && keepAll !== "loading" && keepAll.views.length > 0
      ? quickResolveItems(keepAll.views)
      : null;
  const showKeep = keepAll === "loading" || keepMenu !== null;
  const showOther = Boolean(onReveal || onRemove || onRecover);

  return (
    <ContextMenu onOpenChange={onMenuOpenChange}>
      <ContextMenuTrigger render={<div className="w-full" />}>{row}</ContextMenuTrigger>
      <ContextMenuContent className="min-w-40">
        {keepAll === "loading" ? (
          <ContextMenuItem disabled>Keep all from this machine</ContextMenuItem>
        ) : keepMenu ? (
          <KeepAllMenuItems
            menu={keepMenu}
            disabled={keepAllDisabled}
            onChoose={(choice) => onKeepAll?.(choice)}
          />
        ) : null}
        {showKeep && showOther ? <ContextMenuSeparator /> : null}
        {onReveal ? (
          <ContextMenuItem onClick={onReveal}>Go to location</ContextMenuItem>
        ) : null}
        {onRemove ? (
          <ContextMenuItem
            variant="destructive"
            disabled={writeDisabled}
            onClick={onRemove}
          >
            Remove
          </ContextMenuItem>
        ) : null}
        {onRecover ? (
          <ContextMenuItem disabled={writeDisabled} onClick={onRecover}>
            Recover
          </ContextMenuItem>
        ) : null}
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
