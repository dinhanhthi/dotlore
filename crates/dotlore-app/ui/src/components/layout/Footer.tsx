import { useEffect, useState } from "react";
import { Loader2 } from "lucide-react";

import { SettingsPopover } from "@/components/settings/SettingsPopover";
import { Button } from "@/components/ui/button";
import { uniqueConflictRels } from "@/lib/conflicts";
import { conflicts as fetchConflicts, syncNow } from "@/lib/ipc";
import { useRoots } from "@/lib/roots";
import type { RootRow, RootStatus } from "@/lib/types";
import { cn } from "@/lib/utils";

type Aggregate = {
  glyph: "dot" | "warn";
  color: string;
  text: string;
};

function conflictCount(status: RootStatus): number {
  return status.kind === "Conflicts" ? status.detail : 0;
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

/** `~/Library/.../CloudDocs` — keep first dir after home and the last segment. */
export function shortenProvider(path: string): string {
  const tilde = path.replace(/^\/Users\/[^/]+/, "~");
  const parts = tilde.split("/").filter((p) => p.length > 0);
  if (parts.length <= 3) return tilde;
  return `${parts[0]}/${parts[1]}/.../${parts[parts.length - 1]}`;
}

export function Footer() {
  const {
    roots,
    providerDir,
    trackedBySlug,
    error,
    busy,
    selectedSlug,
    resolvingRel,
    openResolver,
  } = useRoots();
  const status = aggregateStatus(providerDir, roots);
  const filesTracked = Object.values(trackedBySlug).reduce((n, c) => n + c, 0);
  const conflicts = roots.reduce((n, r) => n + conflictCount(r.status), 0);
  const shortProvider =
    providerDir === null ? "No folder" : shortenProvider(providerDir);
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
    <footer className="flex h-11 items-center gap-4 border-t border-border px-3 text-[13px] text-muted-foreground">
      <div className="flex min-w-0 flex-1 items-center gap-2.5" aria-busy={busy}>
        {busy ? (
          <>
            <Loader2 className="size-4 shrink-0 animate-spin" aria-hidden />
            <span className="truncate">Working…</span>
          </>
        ) : status.glyph === "warn" ? (
          <span className={cn("shrink-0 leading-none", status.color)} aria-hidden>
            ▲
          </span>
        ) : (
          <span
            className={cn("size-2.5 shrink-0 rounded-full", status.color)}
            aria-hidden
          />
        )}
        {!busy && <span className="truncate">{status.text}</span>}
        {error !== null && (
          <span className="min-w-0 truncate text-destructive">{error}</span>
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
          {roots.length} roots · {filesTracked} files tracked · {conflicts}{" "}
          conflicts
        </div>
      )}
      <div className="flex min-w-0 flex-1 items-center justify-end gap-3">
        <span
          className="min-w-0 truncate font-path"
          title={providerDir ?? "No cloud folder set"}
        >
          {shortProvider}
        </span>
        <Button
          variant="ghost"
          size="xs"
          className="text-muted-foreground"
          disabled={busy || providerDir === null}
          onClick={() => {
            void syncNow().catch(() => {
              // Banner is set by `run()`.
            });
          }}
        >
          Sync now
        </Button>
        <SettingsPopover />
      </div>
    </footer>
  );
}
