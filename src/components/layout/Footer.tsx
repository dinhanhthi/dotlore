import { useEffect, useState } from "react";
import { Loader2, RefreshCw } from "lucide-react";

import { formatBytes } from "@/components/tree/entries";
import { Button } from "@/components/ui/button";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { uniqueConflictRels } from "@/lib/conflicts";
import { conflicts as fetchConflicts, syncNow } from "@/lib/ipc";
import { useRoots, useSyncing, useTaskLabel, type SeedingRoot } from "@/lib/roots";
import type { RootRow, RootStatus } from "@/lib/types";
import { cn } from "@/lib/utils";

type Aggregate = {
  glyph: "dot" | "warn";
  color: string;
  text: string;
};

function addingLabel(seeding: SeedingRoot[]): string {
  const first = seeding[0];
  if (seeding.length === 1 && first) return `Adding files to ${first.name}…`;
  return `Adding files to ${seeding.length} projects…`;
}

function conflictCount(status: RootStatus): number {
  return status.kind === "Conflicts" ? status.detail : 0;
}

/**
 * First run has no sync runtime until a cloud folder is chosen.
 * Onboarding already asks for that folder, so this is not a footer error.
 */
const NO_RUNTIME_YET = "no sync runtime is running — pick a provider folder first";

function visibleFooterError(
  providerDir: string | null,
  error: string | null,
): string | null {
  if (providerDir === null && error === NO_RUNTIME_YET) return null;
  return error;
}

/** Old `window.rs` ~970–997 priority. */
function aggregateStatus(
  providerDir: string | null,
  roots: RootRow[],
): Aggregate {
  if (providerDir === null) {
    return { glyph: "dot", color: "bg-status-pending", text: "No cloud folder set" };
  }
  if (roots.length === 0) {
    return { glyph: "dot", color: "bg-status-pending", text: "Nothing tracked" };
  }
  if (roots.some((r) => r.status.kind === "GitMissing")) {
    return { glyph: "dot", color: "bg-status-error", text: "git not found" };
  }
  if (roots.some((r) => r.status.kind === "RootMissing")) {
    return { glyph: "dot", color: "bg-status-error", text: "Folder missing" };
  }
  if (roots.some((r) => r.status.kind === "Error")) {
    return { glyph: "dot", color: "bg-status-error", text: "Error" };
  }
  const conflicts = roots.reduce((n, r) => n + conflictCount(r.status), 0);
  if (conflicts > 0) {
    return {
      glyph: "warn",
      color: "text-status-conflict",
      text: conflicts === 1 ? "1 conflict" : `${conflicts} conflicts`,
    };
  }
  if (roots.some((r) => r.status.kind === "Pending")) {
    return { glyph: "dot", color: "bg-status-pending", text: "Pending" };
  }
  return { glyph: "dot", color: "bg-status-synced", text: "Synced" };
}

export function Footer() {
  const {
    roots: allRoots,
    providerDir,
    trackedBySlug,
    error,
    busy,
    seeding,
    loadingRoots,
    selectedSlug,
    resolvingRel,
    openResolver,
    showConflicts,
  } = useRoots();
  // Cloud-only projects are optional on this device, not waiting to sync.
  const roots = allRoots.filter((row) => row.linked);
  const syncing = useSyncing();
  const taskLabel = useTaskLabel();
  const status = aggregateStatus(providerDir, roots);
  const footerError = visibleFooterError(providerDir, error);
  const tracked = Object.values(trackedBySlug);
  const filesTracked = tracked.reduce((n, stats) => n + stats.files, 0);
  const bytesTracked = tracked.reduce((n, stats) => n + stats.bytes, 0);
  const conflicts = roots.reduce((n, r) => n + conflictCount(r.status), 0);
  const selected = roots.find((row) => row.slug === selectedSlug) ?? null;
  const statusKey = selected ? JSON.stringify(selected.status) : "";

  const inResolver = Boolean(resolvingRel && selectedSlug);
  const [resolverRels, setResolverRels] = useState<string[]>([]);

  useEffect(() => {
    if (!inResolver || !selectedSlug) {
      setResolverRels([]);
      return;
    }
    let cancelled = false;
    void fetchConflicts(selectedSlug)
      .then((views) => {
        if (!cancelled) setResolverRels(uniqueConflictRels(views));
      })
      .catch(() => {
        if (!cancelled) setResolverRels([]);
      });
    return () => {
      cancelled = true;
    };
  }, [inResolver, selectedSlug, statusKey]);

  const navRels =
    !resolvingRel
      ? []
      : resolverRels.length === 0
        ? [resolvingRel]
        : resolverRels.includes(resolvingRel)
          ? resolverRels
          : [resolvingRel, ...resolverRels];
  const resolverIndex = resolvingRel ? navRels.indexOf(resolvingRel) : -1;
  const resolverN = navRels.length;
  const resolverI = resolverIndex >= 0 ? resolverIndex + 1 : 0;
  const canStep = Boolean(selectedSlug && resolvingRel && resolverN > 1);

  function stepConflict(delta: number) {
    if (!selectedSlug || resolverN === 0) return;
    const from = resolverIndex >= 0 ? resolverIndex : 0;
    const next = (from + delta + resolverN) % resolverN;
    const rel = navRels[next];
    if (rel) openResolver(selectedSlug, rel);
  }

  return (
    <footer className="flex h-11 items-center gap-4 border-t border-border px-3 text-[0.8rem] text-muted-foreground">
      <div
        className="flex min-w-0 flex-1 items-center gap-2.5"
        aria-busy={busy || loadingRoots || taskLabel !== null}
      >
        {taskLabel ? (
          <>
            <Loader2 className="size-4 shrink-0 animate-spin" aria-hidden />
            <span className="truncate">{taskLabel}</span>
          </>
        ) : seeding.length > 0 ? (
          <>
            <Loader2 className="size-4 shrink-0 animate-spin" aria-hidden />
            <span className="truncate">{addingLabel(seeding)}</span>
          </>
        ) : loadingRoots ? (
          <>
            <Loader2 className="size-4 shrink-0 animate-spin" aria-hidden />
            <span className="truncate">Loading projects…</span>
          </>
        ) : busy ? (
          <>
            <Loader2 className="size-4 shrink-0 animate-spin" aria-hidden />
            <span className="truncate">Working…</span>
          </>
        ) : status.glyph === "warn" ? (
          <Button
            variant="ghost"
            size="xs"
            className="-ml-1.5 min-w-0 gap-2 px-1.5 font-normal"
            aria-label={`${status.text} — show them`}
            onClick={showConflicts}
          >
            <span className={cn("shrink-0 leading-none", status.color)} aria-hidden>
              ▲
            </span>
            <span className="truncate">{status.text}</span>
          </Button>
        ) : (
          <span
            className={cn("size-2.5 shrink-0 rounded-full", status.color)}
            aria-hidden
          />
        )}
        {!taskLabel && seeding.length === 0 && !loadingRoots && !busy && status.glyph !== "warn" && (
          <span className="truncate">{status.text}</span>
        )}
        <Tooltip>
          <TooltipTrigger
            render={
              <Button
                variant="ghost"
                size="icon-xs"
                className="text-muted-foreground"
                disabled={busy || syncing || loadingRoots || providerDir === null}
                aria-busy={syncing || undefined}
                aria-label="Sync now"
                onClick={() => {
                  void syncNow().catch(() => {
                    // Banner is set by `run()`.
                  });
                }}
              />
            }
          >
            <RefreshCw
              className={cn("size-3.5", syncing && "animate-spin")}
              aria-hidden
            />
          </TooltipTrigger>
          <TooltipContent>Sync now</TooltipContent>
        </Tooltip>
        {footerError !== null && (
          <span className="min-w-0 truncate text-destructive">{footerError}</span>
        )}
      </div>
      {resolvingRel && selectedSlug ? (
        <div className="flex shrink-0 items-center gap-2 tabular-nums">
          <Button
            variant="ghost"
            size="xs"
            className="text-muted-foreground"
            disabled={!canStep}
            aria-label="Previous conflict"
            onClick={() => stepConflict(-1)}
          >
            ←
          </Button>
          <span>
            {resolverI} of {resolverN} conflicts
          </span>
          <Button
            variant="ghost"
            size="xs"
            className="text-muted-foreground"
            disabled={!canStep}
            aria-label="Next conflict"
            onClick={() => stepConflict(1)}
          >
            →
          </Button>
        </div>
      ) : (
        <div className="shrink-0 tabular-nums">
          {roots.length === allRoots.length
            ? `${roots.length} roots`
            : `${roots.length} of ${allRoots.length} roots linked`}{" "}
          · {filesTracked} files tracked ·{" "}
          {formatBytes(bytesTracked)} · {conflicts} conflicts
        </div>
      )}
    </footer>
  );
}
