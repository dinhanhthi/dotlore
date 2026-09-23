import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { useCallback, useEffect, useState } from "react";
import {
  Eye,
  EyeOff,
  LayoutGrid,
  Plus,
  RefreshCw,
  Star,
  TriangleAlert,
} from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { BLOCKED, importInstalledAgents, linkRoot, recoverRoot, setBanner } from "@/lib/ipc";
import { pickLocalPath } from "@/lib/pick";
import { compareRoots } from "@/lib/order";
import { conflictCount, conflictTotal, useRoots } from "@/lib/roots";
import { matchesRootQuery, useSidebarQuery } from "@/lib/sidebar-query";
import { defaultSlug } from "@/lib/slug";
import type { RootRow } from "@/lib/types";
import { cn } from "@/lib/utils";

import { AddRootDialog } from "./AddRootDialog";
import { RemoveRootAlert } from "./RemoveRootAlert";
import { SearchBar } from "./SearchBar";
import { SidebarItem } from "./SidebarItem";
import { SidebarSection } from "./SidebarSection";

const COLLAPSED_KEY = "dotlore.sidebar.collapsed";
const HIDE_UNLINKED_KEY = "dotlore.sidebar.hideUnlinked";

function readIds(key: string): string[] {
  try {
    const raw = localStorage.getItem(key);
    if (!raw) return [];
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed.filter((item): item is string => typeof item === "string");
  } catch {
    return [];
  }
}

function writeIds(key: string, ids: string[]): void {
  try {
    localStorage.setItem(key, JSON.stringify(ids));
  } catch {
    // Quota or private-mode — keep the in-memory list.
  }
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

function UnlinkedToggle({
  noun,
  hidden,
  onToggle,
}: {
  noun: string;
  hidden: boolean;
  onToggle: () => void;
}) {
  const label = `${hidden ? "Show" : "Hide"} unlinked ${noun}`;
  const Icon = hidden ? EyeOff : Eye;
  return (
    <Tooltip>
      <TooltipTrigger
        render={
          <Button
            variant="ghost"
            size="icon-xs"
            aria-label={label}
            aria-pressed={hidden}
            onClick={onToggle}
          />
        }
      >
        <Icon aria-hidden />
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
    showConflicts,
    toggleStar,
    refreshRoots,
    locked,
    seeding,
    loadingRoots,
  } = useRoots();
  const { query, setQuery } = useSidebarQuery();
  const conflicts = conflictTotal(roots);
  const [refreshingAgents, setRefreshingAgents] = useState(false);
  const refreshing = refreshingAgents || loadingRoots;
  const [collapsed, setCollapsed] = useState<string[]>(() =>
    readIds(COLLAPSED_KEY),
  );
  const [hideUnlinked, setHideUnlinked] = useState<string[]>(() =>
    readIds(HIDE_UNLINKED_KEY),
  );
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
      writeIds(COLLAPSED_KEY, next);
      return next;
    });
  }, []);

  const toggleHideUnlinked = useCallback((id: string) => {
    setHideUnlinked((current) => {
      const next = current.includes(id)
        ? current.filter((item) => item !== id)
        : [...current, id];
      writeIds(HIDE_UNLINKED_KEY, next);
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
      writeIds(COLLAPSED_KEY, next);
      return next;
    });
    if (row.linked) return;
    setHideUnlinked((current) => {
      if (!current.includes(sectionId)) return current;
      const next = current.filter((id) => id !== sectionId);
      writeIds(HIDE_UNLINKED_KEY, next);
      return next;
    });
  }, [focusRequest, roots]);

  useEffect(() => {
    if (!focusRequest) return;
    const node = document.getElementById(`sidebar-root-${focusRequest.slug}`);
    if (!node) return;
    const reveal = () => {
      node.scrollIntoView({ block: "nearest" });
      node.querySelector<HTMLButtonElement>("button")?.focus({ preventScroll: true });
    };
    reveal();
    const panel = node.closest("[data-collapsed]");
    if (!(panel instanceof HTMLElement)) return;
    const onEnd = (event: TransitionEvent) => {
      if (event.target !== panel || event.propertyName !== "grid-template-rows") return;
      reveal();
    };
    panel.addEventListener("transitionend", onEnd);
    return () => panel.removeEventListener("transitionend", onEnd);
  }, [focusRequest, collapsed, hideUnlinked]);

  const starred = new Set(starredSlugs);
  const visible = roots.filter((row) => matchesRootQuery(row, query));
  const allAgents = visible.filter((row) => row.is_agent);
  const allProjects = visible.filter((row) => !row.is_agent);
  const agentsHaveUnlinked = allAgents.some((row) => !row.linked);
  const projectsHaveUnlinked = allProjects.some((row) => !row.linked);
  const agents = allAgents
    .filter((row) => row.linked || !hideUnlinked.includes("agents"))
    .sort(compareRoots);
  const projects = allProjects
    .filter((row) => row.linked || !hideUnlinked.includes("projects"))
    .sort(compareRoots);

  async function startAdd() {
    if (locked) return;
    const path = await pickLocalPath();
    if (path === null) return;
    setPendingAdd({ path, slug: defaultSlug(path) });
  }

  async function refreshAgents() {
    if (locked || refreshing) return;
    setRefreshingAgents(true);
    try {
      try {
        const report = await importInstalledAgents();
        if (report === BLOCKED) return;
        const first = report.failed[0];
        if (first) setBanner(first.message);
      } catch {
        // Banner is set by `runTask()`.
        return;
      }
      await refreshRoots();
    } finally {
      setRefreshingAgents(false);
    }
  }

  async function handleRecover(slug: string) {
    if (locked) return;
    try {
      if ((await recoverRoot(slug)) === BLOCKED) return;
      await refreshRoots();
    } catch {
      // Banner is set by `runTask()`.
    }
  }

  async function handleLink(slug: string) {
    if (locked) return;
    const path = await pickLocalPath();
    if (path === null) return;
    try {
      if ((await linkRoot(slug, path)) === BLOCKED) return;
      await refreshRoots();
      selectRoot(slug);
    } catch {
      // Banner is set by `runTask()`.
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
        writeDisabled={locked || seeding.some((item) => item.slug === row.slug)}
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
      <div className="sidebar-scroll min-h-0 flex-1 overflow-x-hidden overflow-y-auto">
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
        {conflicts > 0 && (
          <SidebarItem
            label="Conflicts"
            selected={view === "conflicts"}
            conflictCount={conflicts}
            leading={
              <TriangleAlert aria-hidden className="size-4 shrink-0 text-status-conflict" />
            }
            onClick={showConflicts}
          />
        )}
        <SidebarSection
          id="agents"
          title="Agents"
          count={agents.length}
          conflicts={conflictTotal(agents)}
          collapsed={collapsed.includes("agents")}
          onToggle={() => toggleCollapsed("agents")}
          action={
            <div className="flex items-center">
              {agentsHaveUnlinked && (
                <UnlinkedToggle
                  noun="agents"
                  hidden={hideUnlinked.includes("agents")}
                  onToggle={() => toggleHideUnlinked("agents")}
                />
              )}
              <Tooltip>
                <TooltipTrigger
                  render={
                    <Button
                      variant="ghost"
                      size="icon-xs"
                      aria-label="Refresh agents"
                      aria-busy={refreshing || undefined}
                      disabled={locked || refreshing}
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
              <AddSectionButton
                label="Add agent"
                disabled={locked || loadingRoots}
                onPick={() => void startAdd()}
              />
            </div>
          }
        >
          {agents.map((row) => renderRoot(row, `sidebar-root-${row.slug}`))}
        </SidebarSection>
        <SidebarSection
          id="projects"
          title="Projects"
          count={projects.length}
          conflicts={conflictTotal(projects)}
          collapsed={collapsed.includes("projects")}
          onToggle={() => toggleCollapsed("projects")}
          action={
            <div className="flex items-center">
              {projectsHaveUnlinked && (
                <UnlinkedToggle
                  noun="projects"
                  hidden={hideUnlinked.includes("projects")}
                  onToggle={() => toggleHideUnlinked("projects")}
                />
              )}
              <AddSectionButton
                label="Add project"
                disabled={locked || loadingRoots}
                onPick={() => void startAdd()}
              />
            </div>
          }
        >
          {projects.map((row) => renderRoot(row, `sidebar-root-${row.slug}`))}
        </SidebarSection>
        </div>
      </div>
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
