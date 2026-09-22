import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { FolderOpen, Star, Trash2 } from "lucide-react";
import { useState } from "react";

import { RemoveRootAlert } from "@/components/sidebar/RemoveRootAlert";
import { Button } from "@/components/ui/button";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { useRoots } from "@/lib/roots";
import { cn } from "@/lib/utils";

export function TitleBarActions() {
  const {
    roots,
    selectedSlug,
    selectedRel,
    view,
    starredSlugs,
    toggleStar,
    busy,
    seeding,
  } = useRoots();
  const [removeOpen, setRemoveOpen] = useState(false);
  const root = roots.find((row) => row.slug === selectedSlug) ?? null;
  const active = view === "root" && root !== null;
  const starred = root !== null && starredSlugs.includes(root.slug);
  const fileOpen = selectedRel !== null;
  const seedingThis =
    selectedSlug !== null && seeding.some((item) => item.slug === selectedSlug);
  const revealPath = root === null || fileOpen ? null : root.path;

  function reveal() {
    if (!revealPath) return;
    void revealItemInDir(revealPath).catch(() => {
      // Path missing or the file manager is unavailable.
    });
  }

  return (
    <div className="flex shrink-0 items-center gap-0.5 pr-2">
      <Tooltip>
        <TooltipTrigger
          render={
            <Button
              variant="ghost"
              size="icon-sm"
              disabled={!active}
              aria-label={starred ? "Unstar" : "Star"}
              aria-pressed={starred}
              onClick={() => {
                if (root) toggleStar(root.slug);
              }}
              className={cn(
                "text-muted-foreground",
                starred && "text-foreground",
              )}
            />
          }
        >
          <Star aria-hidden className={cn(starred && "fill-current")} />
        </TooltipTrigger>
        <TooltipContent>{starred ? "Unstar" : "Star"}</TooltipContent>
      </Tooltip>

      <Tooltip>
        <TooltipTrigger
          render={
            <Button
              variant="ghost"
              size="icon-sm"
              disabled={!active || busy || seedingThis}
              aria-label="Remove from Dotlore"
              onClick={() => setRemoveOpen(true)}
              className="text-muted-foreground hover:text-destructive"
            />
          }
        >
          <Trash2 aria-hidden />
        </TooltipTrigger>
        <TooltipContent>Remove from Dotlore</TooltipContent>
      </Tooltip>

      {!fileOpen && (
        <Tooltip>
          <TooltipTrigger
            render={
              <Button
                variant="ghost"
                size="icon-sm"
                disabled={!active || !revealPath}
                aria-label="Go to location"
                onClick={reveal}
                className="text-muted-foreground"
              />
            }
          >
            <FolderOpen aria-hidden />
          </TooltipTrigger>
          <TooltipContent>Go to location</TooltipContent>
        </Tooltip>
      )}

      <RemoveRootAlert
        slug={root?.slug ?? null}
        name={root?.name ?? "this root"}
        open={removeOpen}
        onOpenChange={setRemoveOpen}
      />
    </div>
  );
}
