import { useCallback, useEffect, useState } from "react";
import { LayoutGrid, Plus } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { ScrollArea } from "@/components/ui/scroll-area";
import { recoverRoot } from "@/lib/ipc";
import { pickLocalPath } from "@/lib/pick";
import { useRoots } from "@/lib/roots";
import { defaultSlug } from "@/lib/slug";
import type { RootRow } from "@/lib/types";

import { AddRootDialog } from "./AddRootDialog";
import { LinkRootDialog } from "./LinkRootDialog";
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
  onPick: (directory: boolean) => void;
  disabled?: boolean;
}) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        render={
          <Button
            variant="ghost"
            size="icon-xs"
            aria-label={label}
            disabled={disabled}
          />
        }
      >
        <Plus aria-hidden />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="min-w-28">
        <DropdownMenuItem disabled={disabled} onClick={() => onPick(true)}>
          Folder…
        </DropdownMenuItem>
        <DropdownMenuItem disabled={disabled} onClick={() => onPick(false)}>
          File…
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
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
    showAllProjects,
    toggleStar,
    refreshRoots,
    busy,
  } = useRoots();
  const [query, setQuery] = useState("");
  const [collapsed, setCollapsed] = useState<string[]>(() => readCollapsed());
  const [pendingAdd, setPendingAdd] = useState<{
    path: string;
    slug: string;
  } | null>(null);
  const [linkOpen, setLinkOpen] = useState(false);
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
  const starredRows = visible.filter((row) => starred.has(row.slug));
  const agents = visible.filter((row) => row.is_agent);
  const projects = visible.filter((row) => !row.is_agent);

  async function startAdd(directory: boolean) {
    if (busy) return;
    const path = await pickLocalPath(directory);
    if (path === null) return;
    setPendingAdd({ path, slug: defaultSlug(path) });
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
        onClick={() => selectRoot(row.slug)}
        onToggleStar={() => toggleStar(row.slug)}
        onRemove={() => setRemoveTarget(row)}
        writeDisabled={busy}
        onRecover={
          row.status.kind === "Error" ? () => void handleRecover(row.slug) : undefined
        }
      />
    );
  }

  return (
    <nav aria-label="Roots" className="flex h-full min-h-0 flex-col">
      <SearchBar value={query} onChange={setQuery} />
      <div className="flex justify-end px-pad-x pb-1">
        <Button
          type="button"
          variant="ghost"
          size="xs"
          disabled={busy}
          onClick={() => setLinkOpen(true)}
        >
          Link…
        </Button>
      </div>
      <ScrollArea className="min-h-0 flex-1">
        {starredRows.length > 0 && (
          <SidebarSection
            id="starred"
            title="Starred"
            collapsed={collapsed.includes("starred")}
            onToggle={() => toggleCollapsed("starred")}
          >
            {starredRows.map((row) => renderRoot(row))}
          </SidebarSection>
        )}
        <SidebarItem
          label="All projects"
          selected={view === "all"}
          leading={
            <LayoutGrid aria-hidden className="size-3 shrink-0 text-muted-foreground" />
          }
          onClick={showAllProjects}
        />
        <SidebarSection
          id="agents"
          title="Agents"
          collapsed={collapsed.includes("agents")}
          onToggle={() => toggleCollapsed("agents")}
          action={
            <AddSectionButton
              label="Add agent"
              disabled={busy}
              onPick={(directory) => void startAdd(directory)}
            />
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
              onPick={(directory) => void startAdd(directory)}
            />
          }
        >
          {projects.map((row) => renderRoot(row, `sidebar-root-${row.slug}`))}
        </SidebarSection>
      </ScrollArea>
      <AddRootDialog
        path={pendingAdd?.path ?? ""}
        defaultSlug={pendingAdd?.slug ?? ""}
        open={pendingAdd !== null}
        onOpenChange={(open) => {
          if (!open) setPendingAdd(null);
        }}
      />
      <LinkRootDialog open={linkOpen} onOpenChange={setLinkOpen} />
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
