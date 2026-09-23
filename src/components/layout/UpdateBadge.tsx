import { useEffect, useState } from "react";
import { Download, Loader2 } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import {
  listenUpdate,
  listenUpdateProgress,
  promptUpdate,
  updateAvailable,
  type UpdateProgress,
} from "@/lib/ipc";

export function progressLabel(progress: UpdateProgress): string | null {
  if (progress.phase === "installing") return "Installing update…";
  if (progress.phase === "downloading") {
    return progress.percent === null
      ? "Downloading update…"
      : `Downloading update ${progress.percent}%`;
  }
  return null;
}

/** Title-bar badge for an update the background check found. */
export function UpdateBadge() {
  const [version, setVersion] = useState<string | null>(null);
  const [progress, setProgress] = useState<UpdateProgress>({ phase: "idle" });

  useEffect(() => {
    let cancelled = false;
    const stopUpdate = listenUpdate(setVersion);
    const stopProgress = listenUpdateProgress(setProgress);
    // The launch check can finish before these listeners exist.
    void updateAvailable()
      .then((found) => {
        if (!cancelled) setVersion(found);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      void stopUpdate.then((stop) => stop());
      void stopProgress.then((stop) => stop());
    };
  }, []);

  const working = progressLabel(progress);
  if (working !== null) {
    return (
      <span
        className="flex items-center gap-1.5 px-2 text-xs text-muted-foreground tabular-nums"
        role="status"
      >
        <Loader2 className="size-3 animate-spin" aria-hidden />
        {working}
      </span>
    );
  }
  if (version === null) return null;

  const label = `Update to Dotlore ${version}`;
  return (
    <Tooltip>
      <TooltipTrigger
        render={
          <Button
            type="button"
            variant="secondary"
            size="xs"
            aria-label={label}
            onClick={() => {
              void promptUpdate().catch(() => {});
            }}
          />
        }
      >
        <Download aria-hidden />
        Update
      </TooltipTrigger>
      <TooltipContent>{`Dotlore ${version} is available`}</TooltipContent>
    </Tooltip>
  );
}
