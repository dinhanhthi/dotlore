import { Info, KeyRound } from "lucide-react";

import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import type { Sensitivity } from "@/lib/types";

/** Warning color for a Secret file's name; nothing otherwise. */
export function sensitiveNameClass(
  sensitivity: Sensitivity | null | undefined,
): string | undefined {
  return sensitivity === "secret" ? "text-status-conflict" : undefined;
}

/** Key or hint icon with a tooltip; renders nothing for a plain file. */
export function SensitivityMark({
  sensitivity,
}: {
  sensitivity: Sensitivity | null | undefined;
}) {
  const label =
    sensitivity === "secret"
      ? "Sensitive file — may contain secrets"
      : sensitivity === "tokenHint"
        ? "May contain API tokens"
        : null;
  if (!label) return null;

  return (
    <Tooltip>
      <TooltipTrigger
        render={<span className="inline-flex shrink-0" aria-label={label} />}
      >
        {sensitivity === "secret" ? (
          <KeyRound aria-hidden className="size-3.5 text-status-conflict" />
        ) : (
          <Info aria-hidden className="size-3.5 text-muted-foreground" />
        )}
      </TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
  );
}
