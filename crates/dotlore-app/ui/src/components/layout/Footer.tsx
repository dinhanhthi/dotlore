import { Loader2 } from "lucide-react";

import { SettingsPopover } from "@/components/settings/SettingsPopover";
import { Button } from "@/components/ui/button";
import { syncNow } from "@/lib/ipc";
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
  const { roots, providerDir, trackedBySlug, error, busy } = useRoots();
  const status = aggregateStatus(providerDir, roots);
  const filesTracked = Object.values(trackedBySlug).reduce((n, c) => n + c, 0);
  const conflicts = roots.reduce((n, r) => n + conflictCount(r.status), 0);
  const shortProvider =
    providerDir === null ? "No folder" : shortenProvider(providerDir);

  return (
    <footer className="flex h-6 items-center gap-3 border-t border-border px-2 text-[11px] text-muted-foreground">
      <div className="flex min-w-0 flex-1 items-center gap-1.5" aria-busy={busy}>
        {busy ? (
          <>
            <Loader2 className="size-3 shrink-0 animate-spin" aria-hidden />
            <span className="truncate">Working…</span>
          </>
        ) : status.glyph === "warn" ? (
          <span className={cn("shrink-0 leading-none", status.color)} aria-hidden>
            ▲
          </span>
        ) : (
          <span
            className={cn("size-2 shrink-0 rounded-full", status.color)}
            aria-hidden
          />
        )}
        {!busy && <span className="truncate">{status.text}</span>}
        {error !== null && (
          <span className="min-w-0 truncate text-[#eb5757]">{error}</span>
        )}
      </div>
      <div className="shrink-0 tabular-nums">
        {roots.length} roots · {filesTracked} files tracked · {conflicts}{" "}
        conflicts
      </div>
      <div className="flex min-w-0 flex-1 items-center justify-end gap-2">
        <span
          className="min-w-0 truncate font-path text-[11px]"
          title={providerDir ?? "No cloud folder set"}
        >
          {shortProvider}
        </span>
        <Button
          variant="ghost"
          size="xs"
          className="h-5 px-1.5 text-[11px] text-muted-foreground"
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
