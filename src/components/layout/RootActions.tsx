import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { EllipsisVertical, Star, Trash2 } from "lucide-react";
import { useState, type ReactNode } from "react";

import { RemoveRootAlert } from "@/components/sidebar/RemoveRootAlert";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { useRoots } from "@/lib/roots";
import { cn } from "@/lib/utils";

import { RevealInFinderButton } from "./RevealInFinderButton";

function useSelectedRoot() {
  const { roots, selectedSlug, view, starredSlugs, toggleStar, busy, seeding } =
    useRoots();
  const root = roots.find((row) => row.slug === selectedSlug) ?? null;
  const active = view === "root" && root !== null;
  const starred = root !== null && starredSlugs.includes(root.slug);
  const seedingThis =
    selectedSlug !== null && seeding.some((item) => item.slug === selectedSlug);
  const revealPath = root?.path ? root.path : null;
  const removeDisabled = !active || busy || seedingThis;
  return {
    root,
    active,
    starred,
    revealPath,
    removeDisabled,
    toggleStar,
  };
}

export function StarRootButton() {
  const { root, active, starred, toggleStar } = useSelectedRoot();

  return (
    <Tooltip>
      <TooltipTrigger
        render={
          <Button
            type="button"
            variant="ghost"
            size="icon-xs"
            disabled={!active}
            aria-label={starred ? "Unstar" : "Star"}
            aria-pressed={starred}
            onClick={() => {
              if (root) toggleStar(root.slug);
            }}
            className={cn("text-muted-foreground", starred && "text-foreground")}
          />
        }
      >
        <Star aria-hidden className={cn(starred && "fill-current")} />
      </TooltipTrigger>
      <TooltipContent>{starred ? "Unstar" : "Star"}</TooltipContent>
    </Tooltip>
  );
}

function RemoveRootControl({
  children,
}: {
  children: (openRemove: () => void, disabled: boolean) => ReactNode;
}) {
  const { root, removeDisabled } = useSelectedRoot();
  const [removeOpen, setRemoveOpen] = useState(false);

  return (
    <>
      {children(() => setRemoveOpen(true), removeDisabled)}
      <RemoveRootAlert
        slug={root?.slug ?? null}
        name={root?.name ?? "this root"}
        open={removeOpen}
        onOpenChange={setRemoveOpen}
      />
    </>
  );
}

export function RemoveRootButton() {
  return (
    <RemoveRootControl>
      {(openRemove, disabled) => (
        <Tooltip>
          <TooltipTrigger
            render={
              <Button
                type="button"
                variant="ghost"
                size="icon-xs"
                disabled={disabled}
                aria-label="Remove from Dotlore"
                onClick={openRemove}
                className="text-muted-foreground hover:text-destructive"
              />
            }
          >
            <Trash2 aria-hidden />
          </TooltipTrigger>
          <TooltipContent>Remove from Dotlore</TooltipContent>
        </Tooltip>
      )}
    </RemoveRootControl>
  );
}

export function RootOverflowMenu() {
  const { active, revealPath } = useSelectedRoot();
  const [menuOpen, setMenuOpen] = useState(false);

  function reveal() {
    if (!revealPath) return;
    void revealItemInDir(revealPath).catch(() => {
      // Path missing or the file manager is unavailable.
    });
  }

  return (
    <RemoveRootControl>
      {(openRemove, removeDisabled) => (
        <DropdownMenu open={menuOpen} onOpenChange={setMenuOpen}>
          <Tooltip disabled={menuOpen}>
            <TooltipTrigger
              render={
                <DropdownMenuTrigger
                  render={
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon-xs"
                      disabled={!active}
                      aria-label="More"
                      className="text-muted-foreground"
                    />
                  }
                />
              }
            >
              <EllipsisVertical aria-hidden />
            </TooltipTrigger>
            <TooltipContent>More</TooltipContent>
          </Tooltip>
          <DropdownMenuContent align="end" sideOffset={6}>
            <DropdownMenuItem
              variant="destructive"
              disabled={removeDisabled}
              onClick={openRemove}
            >
              Remove
            </DropdownMenuItem>
            <DropdownMenuItem disabled={!active || !revealPath} onClick={reveal}>
              Go to location
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      )}
    </RemoveRootControl>
  );
}

/** File panel header: reveal the file, then the project star and remove controls. */
export function FileHeaderActions({ path }: { path: string | null }) {
  return (
    <div className="flex shrink-0 items-center gap-0.5">
      <RevealInFinderButton path={path} />
      <StarRootButton />
      <RemoveRootButton />
    </div>
  );
}
