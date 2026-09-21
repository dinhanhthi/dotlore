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
            aria-label="Go to location"
            onClick={() => {
              if (!path) return;
              void revealItemInDir(path).catch(() => {
                // Path missing or the file manager is unavailable.
              });
            }}
          />
        }
      >
        <FolderOpen className="size-3.5" aria-hidden />
      </TooltipTrigger>
      <TooltipContent>Go to location</TooltipContent>
    </Tooltip>
  );
}
