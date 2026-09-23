import { useEffect, useState } from "react";

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { errorMessage } from "@/lib/errors";
import { BLOCKED, closeResolution, openResolution, resolveBinary } from "@/lib/ipc";
import { useRoots } from "@/lib/roots";
import type { ConflictView } from "@/lib/types";

import { pickSiblingPath, quickResolveCopy } from "./quick-resolve";

export type QuickResolveTarget = {
  rel: string;
  keep: "live" | "other";
  siblingRel?: string;
  device?: string;
  views: ConflictView[];
};

type QuickResolveDialogProps = {
  slug: string;
  target: QuickResolveTarget | null;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onResolved: () => void;
};

export function QuickResolveDialog({
  slug,
  target,
  open,
  onOpenChange,
  onResolved,
}: QuickResolveDialogProps) {
  const { locked, refreshRoots } = useRoots();
  const [running, setRunning] = useState(false);
  const [message, setMessage] = useState<string | null>(null);

  useEffect(() => {
    setMessage(null);
  }, [target]);

  const copy = target
    ? quickResolveCopy(target.keep, target.rel, target.views, target.device)
    : null;

  async function confirm() {
    if (target === null || locked || running) return;
    const { rel, keep, siblingRel } = target;
    setRunning(true);
    setMessage(null);
    try {
      const dto = await openResolution(slug, rel);
      try {
        let path: string | null = null;
        if (keep === "other") {
          path = siblingRel ? pickSiblingPath(dto, siblingRel) : null;
          if (path === null) {
            setMessage("That version is no longer available. Refresh and try again.");
            return;
          }
        }
        const result = await resolveBinary(slug, rel, keep, path);
        if (result === BLOCKED) {
          onOpenChange(false);
          return;
        }
        if (result.outcome === "applied") {
          await refreshRoots().catch(() => {
            // Tree/status still refresh from the status event.
          });
          onResolved();
          onOpenChange(false);
          return;
        }
        setMessage(
          result.outcome === "stale"
            ? "The file changed on another device. Review and try again."
            : "Sync has not finished yet. Try again in a moment.",
        );
      } finally {
        void closeResolution(slug, rel).catch(() => {
          // Nothing to release.
        });
      }
    } catch (cause) {
      setMessage(errorMessage(cause, "Could not resolve the conflict"));
    } finally {
      setRunning(false);
    }
  }

  const disabled = locked || running || target === null;

  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{copy?.title ?? "Resolve conflict?"}</AlertDialogTitle>
          <AlertDialogDescription>{copy?.description ?? ""}</AlertDialogDescription>
        </AlertDialogHeader>
        {message ? (
          <p role="status" className="text-sm text-status-conflict">
            {message}
          </p>
        ) : null}
        <AlertDialogFooter>
          <AlertDialogCancel disabled={locked || running}>Cancel</AlertDialogCancel>
          <AlertDialogAction
            variant="destructive"
            disabled={disabled}
            onClick={(event) => {
              event.preventDefault();
              void confirm();
            }}
          >
            {copy?.action ?? "Resolve"}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
