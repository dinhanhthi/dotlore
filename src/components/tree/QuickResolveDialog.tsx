import { useEffect, useRef, useState } from "react";

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
import { closeResolution, openResolution, resolveBinary } from "@/lib/ipc";
import { useRoots } from "@/lib/roots";

import {
  applyKeepAll,
  keepAllDialogAction,
  keepAllRemainder,
  quickResolveTargetCopy,
  type QuickResolveTarget,
} from "./quick-resolve";

export type { QuickResolveTarget };

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
  const appliedRels = useRef(new Set<string>());

  useEffect(() => {
    setMessage(null);
    appliedRels.current = new Set();
  }, [target]);

  const copy = target ? quickResolveTargetCopy(target) : null;

  async function confirm() {
    if (target === null || locked || running) return;
    const remaining = keepAllRemainder(target.files, appliedRels.current);
    setRunning(true);
    setMessage(null);
    try {
      const result = await applyKeepAll(slug, remaining, {
        openResolution,
        resolveBinary,
        closeResolution,
      });
      for (const rel of result.applied) appliedRels.current.add(rel);
      const action = keepAllDialogAction(result);
      setMessage(action.message);
      if (!action.refresh) return;
      await refreshRoots().catch(() => {
        // Tree/status still refresh from the status event.
      });
      if (action.resolved) onResolved();
      if (action.close) onOpenChange(false);
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
