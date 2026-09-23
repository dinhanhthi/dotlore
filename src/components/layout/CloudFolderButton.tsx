import { FolderOpen } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { openCloudFolder } from "@/lib/ipc";
import { useRoots } from "@/lib/roots";

const LABEL = "Go to cloud folder";

export function CloudFolderButton({ size }: { size: "icon-xs" | "icon-sm" }) {
  const { providerDir } = useRoots();
  return (
    <Tooltip>
      <TooltipTrigger
        render={
          <Button
            variant="ghost"
            size={size}
            className="text-muted-foreground"
            disabled={providerDir === null}
            aria-label={LABEL}
            onClick={() => {
              void openCloudFolder().catch(() => {
                // Folder missing or Finder is unavailable.
              });
            }}
          />
        }
      >
        <FolderOpen className={size === "icon-xs" ? "size-3.5" : undefined} aria-hidden />
      </TooltipTrigger>
      <TooltipContent>{LABEL}</TooltipContent>
    </Tooltip>
  );
}
