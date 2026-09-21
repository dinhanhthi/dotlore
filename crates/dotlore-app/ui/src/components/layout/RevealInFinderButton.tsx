import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { FolderOpen } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";

export function RevealInFinderButton({ path }: { path: string | null }) {
  return (
    <Tooltip>
      <TooltipTrigger
        render={
          <Button
            variant="ghost"
            size="icon-xs"
            className="text-muted-foreground"
            disabled={path === null}
            aria-label="Reveal in Finder"
            onClick={() => {
              if (!path) return;
              void revealItemInDir(path).catch(() => {
                // Path missing or Finder unavailable.
              });
            }}
          />
        }
      >
        <FolderOpen className="size-3.5" aria-hidden />
      </TooltipTrigger>
      <TooltipContent>Reveal in Finder</TooltipContent>
    </Tooltip>
  );
}
