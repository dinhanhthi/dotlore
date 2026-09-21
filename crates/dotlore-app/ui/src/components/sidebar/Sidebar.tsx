import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { useCallback, useEffect, useState } from "react";
import { LayoutGrid, Plus, RefreshCw, Star } from "lucide-react";

import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { importInstalledAgents, linkRoot, recoverRoot, setBanner } from "@/lib/ipc";
import { pickLocalPath } from "@/lib/pick";
import { compareRoots } from "@/lib/order";
import { useRoots } from "@/lib/roots";
import { defaultSlug } from "@/lib/slug";
import type { RootRow } from "@/lib/types";
import { cn } from "@/lib/utils";

import { AddRootDialog } from "./AddRootDialog";
import { RemoveRootAlert } from "./RemoveRootAlert";
import { SearchBar } from "./SearchBar";
import { SidebarItem } from "./SidebarItem";
import { SidebarSection } from "./SidebarSection";

const COLLAPSED_KEY = "dotlore.sidebar.collapsed";

function readCollapsed(): string[] {
  try {
    const raw = localStorage.getItem(COLLAPSED_KEY);
    if (!raw) return [];
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed.filter((item): item is string => typeof item === "string");
  } catch {
    return [];
  }
}

function writeCollapsed(ids: string[]): void {
  try {
    localStorage.setItem(COLLAPSED_KEY, JSON.stringify(ids));
  } catch {
    // Quota or private-mode — keep the in-memory list.
  }
}

function matchesQuery(row: RootRow, query: string): boolean {
  const needle = query.trim().toLowerCase();
  if (!needle) return true;
  return (
    row.name.toLowerCase().includes(needle) ||
    row.slug.toLowerCase().includes(needle)
  );
}

function conflictCount(row: RootRow): number {
  return row.status.kind === "Conflicts" ? row.status.detail : 0;
}

function AddSectionButton({
  label,
  onPick,
  disabled,
}: {
  label: string;
  onPick: () => void;
  disabled?: boolean;
}) {
  return (
    <Tooltip>
      <TooltipTrigger
        render={
          <Button
            variant="ghost"
            size="icon-xs"
            aria-label={label}
            disabled={disabled}
            onClick={onPick}
          />
        }
      >
        <Plus aria-hidden />
      </TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
  );
}

export function Sidebar() {
  const {
    roots,
    selectedSlug,
    view,
    starredSlugs,
    focusRequest,
    selectRoot,
    openFirstConflict,
    showAllProjects,
    showStarred,
    toggleStar,
    refreshRoots,
    busy,
    seeding,
  } = useRoots();
  const [query, setQuery] = useState("");
  const [refreshing, setRefreshing] = useState(false);
  const [collapsed, setCollapsed] = useState<string[]>(() => readCollapsed());
  const [pendingAdd, setPendingAdd] = useState<{
    path: string;
    slug: string;
  } | null>(null);
  const [removeTarget, setRemoveTarget] = useState<RootRow | null>(null);

  const toggleCollapsed = useCallback((id: string) => {
    setCollapsed((current) => {
      const next = current.includes(id)
        ? current.filter((item) => item !== id)
        : [...current, id];
      writeCollapsed(next);
      return next;
    });
  }, []);

  useEffect(() => {
    if (!focusRequest) return;
    const row = roots.find((item) => item.slug === focusRequest.slug);
    if (!row) return;
    const sectionId = row.is_agent ? "agents" : "projects";
    setCollapsed((current) => {
      if (!current.includes(sectionId)) return current;
      const next = current.filter((id) => id !== sectionId);
      writeCollapsed(next);
      return next;
    });
  }, [focusRequest, roots]);

  useEffect(() => {
    if (!focusRequest) return;
    const node = document.getElementById(`sidebar-root-${focusRequest.slug}`);
    if (!node) return;
    node.scrollIntoView({ block: "nearest" });
    node.querySelector<HTMLButtonElement>("button")?.focus({ preventScroll: true });
  }, [focusRequest, collapsed]);

  const starred = new Set(starredSlugs);
  const visible = roots.filter((row) => matchesQuery(row, query));
  const agents = visible.filter((row) => row.is_agent).sort(compareRoots);
  const projects = visible.filter((row) => !row.is_agent).sort(compareRoots);

  async function startAdd() {
    if (busy) return;
    const path = await pickLocalPath();
    if (path === null) return;
    setPendingAdd({ path, slug: defaultSlug(path) });
  }

  async function refreshAgents() {
    if (busy || refreshing) return;
    setRefreshing(true);
    try {
      try {
        const report = await importInstalledAgents();
        const first = report.failed[0];
        if (first) setBanner(first.message);
      } catch {
        // Banner is set by `run()`.
        return;
      }
      await refreshRoots();
    } finally {
      setRefreshing(false);
    }
  }

  async function handleRecover(slug: string) {
    if (busy) return;
    try {
      await recoverRoot(slug);
      await refreshRoots();
    } catch {
      // Banner is set by `run()`.
    }
  }

  async function handleLink(slug: string) {
    if (busy) return;
    const path = await pickLocalPath();
    if (path === null) return;
    try {
      await linkRoot(slug, path);
      await refreshRoots();
      selectRoot(slug);
    } catch {
      // Banner is set by `run()`.
    }
  }

  function renderRoot(row: RootRow, id?: string) {
    return (
      <SidebarItem
        key={id ?? row.slug}
        id={id}
        label={row.name}
        title={row.path}
        selected={view === "root" && selectedSlug === row.slug}
        statusKind={row.status.kind}
        conflictCount={conflictCount(row)}
        starred={starred.has(row.slug)}
        linked={row.linked}
        onClick={() => selectRoot(row.slug)}
        onConflictClick={() => {
          openFirstConflict(row.slug);
        }}
        onToggleStar={() => toggleStar(row.slug)}
        onLink={() => void handleLink(row.slug)}
        onRemove={() => setRemoveTarget(row)}
        onReveal={
          row.linked && row.path
            ? () => {
                void revealItemInDir(row.path).catch(() => {
                  // Path missing or the file manager is unavailable.
                });
              }
            : undefined
        }
        writeDisabled={busy || seeding.some((item) => item.slug === row.slug)}
        onRecover={
          row.status.kind === "Error" ? () => void handleRecover(row.slug) : undefined
        }
      />
    );
  }

  return (
    <nav aria-label="Roots" className="flex h-full min-h-0 flex-col">
      <div className="flex items-center gap-2 px-3 pt-3 pb-2">
        <SearchBar value={query} onChange={setQuery} />
      </div>
      <ScrollArea className="min-h-0 flex-1">
        <div className="flex flex-col gap-0.5 px-1.5 pb-2">
        <SidebarItem
          label="All projects"
          selected={view === "all"}
          leading={
            <LayoutGrid aria-hidden className="size-4 shrink-0 text-muted-foreground" />
          }
          onClick={showAllProjects}
        />
        <SidebarItem
          label="Starred"
          selected={view === "starred"}
          leading={
            <Star aria-hidden className="size-4 shrink-0 text-muted-foreground" />
          }
          onClick={showStarred}
        />
        <SidebarSection
          id="agents"
          title="Agents"
          collapsed={collapsed.includes("agents")}
          onToggle={() => toggleCollapsed("agents")}
          action={
            <div className="flex items-center">
              <AddSectionButton
                label="Add agent"
                disabled={busy}
                onPick={() => void startAdd()}
              />
              <Tooltip>
                <TooltipTrigger
                  render={
                    <Button
                      variant="ghost"
                      size="icon-xs"
                      aria-label="Refresh agents"
                      aria-busy={refreshing || undefined}
                      disabled={busy || refreshing}
                      onClick={() => void refreshAgents()}
                    />
                  }
                >
                  <RefreshCw
                    aria-hidden
                    className={cn(refreshing && "animate-spin")}
                  />
                </TooltipTrigger>
                <TooltipContent>Refresh agents</TooltipContent>
              </Tooltip>
            </div>
          }
        >
          {agents.map((row) => renderRoot(row, `sidebar-root-${row.slug}`))}
        </SidebarSection>
        <SidebarSection
          id="projects"
          title="Projects"
          collapsed={collapsed.includes("projects")}
          onToggle={() => toggleCollapsed("projects")}
          action={
            <AddSectionButton
              label="Add project"
              disabled={busy}
              onPick={() => void startAdd()}
            />
          }
        >
          {projects.map((row) => renderRoot(row, `sidebar-root-${row.slug}`))}
        </SidebarSection>
        </div>
      </ScrollArea>
      <AddRootDialog
        path={pendingAdd?.path ?? ""}
        defaultSlug={pendingAdd?.slug ?? ""}
        open={pendingAdd !== null}
        onOpenChange={(open) => {
          if (!open) setPendingAdd(null);
        }}
      />
      <RemoveRootAlert
        slug={removeTarget?.slug ?? null}
        name={removeTarget?.name ?? "this root"}
        open={removeTarget !== null}
        onOpenChange={(open) => {
          if (!open) setRemoveTarget(null);
        }}
      />
    </nav>
  );
}
