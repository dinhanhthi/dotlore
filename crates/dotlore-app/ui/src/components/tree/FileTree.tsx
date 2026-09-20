import { useCallback, useEffect, useMemo, useState } from "react";

import { TreeNode } from "@/components/tree/TreeNode";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { conflicts as fetchConflicts, syncNow, trackedFiles } from "@/lib/ipc";
import { useRoots } from "@/lib/roots";
import { buildTree } from "@/lib/tree";
import type { ConflictView } from "@/lib/types";

function conflictPathSet(views: ConflictView[]): Set<string> {
  return new Set(views.map((view) => String(view.live).replace(/\\/g, "/")));
}

export function FileTree() {
  const { roots, selectedSlug, selectedRel, selectFile, openResolver, busy } =
    useRoots();
  const root = roots.find((row) => row.slug === selectedSlug) ?? null;

  const [paths, setPaths] = useState<string[]>([]);
  const [conflictSet, setConflictSet] = useState<Set<string>>(() => new Set());
  const [openBySlug, setOpenBySlug] = useState<Record<string, Record<string, boolean>>>(
    {},
  );

  const statusKey = root ? JSON.stringify(root.status) : "";

  useEffect(() => {
    if (!selectedSlug) {
      setPaths([]);
      setConflictSet(new Set());
      return;
    }

    let cancelled = false;
    void (async () => {
      const [files, views] = await Promise.all([
        trackedFiles(selectedSlug).catch((): string[] => []),
        fetchConflicts(selectedSlug).catch((): ConflictView[] => []),
      ]);
      if (cancelled) return;
      setPaths(files);
      setConflictSet(conflictPathSet(views));
    })();

    return () => {
      cancelled = true;
    };
  }, [selectedSlug, statusKey]);

  const tree = useMemo(() => buildTree(paths), [paths]);

  const isOpen = useCallback(
    (path: string, depth: number): boolean => {
      if (!selectedSlug) return false;
      const explicit = openBySlug[selectedSlug]?.[path];
      if (explicit !== undefined) return explicit;
      return depth === 0;
    },
    [openBySlug, selectedSlug],
  );

  const onToggle = useCallback(
    (path: string, depth: number) => {
      if (!selectedSlug) return;
      const next = !isOpen(path, depth);
      setOpenBySlug((current) => ({
        ...current,
        [selectedSlug]: { ...current[selectedSlug], [path]: next },
      }));
    },
    [isOpen, selectedSlug],
  );

  if (!root) {
    return <div className="h-full" />;
  }

  return (
    <div className="flex h-full min-h-0 flex-col">
      <header className="flex h-row shrink-0 items-center gap-3 border-b border-border px-3">
        <span className="min-w-0 flex-1 truncate text-foreground">{root.name}</span>
        <span className="shrink-0 tabular-nums text-muted-foreground">
          {paths.length} {paths.length === 1 ? "file" : "files"}
        </span>
        <Button
          variant="ghost"
          size="xs"
          className="text-muted-foreground"
          disabled={busy}
          onClick={() => {
            void syncNow().catch(() => {
              // Banner is set by `run()`.
            });
          }}
        >
          Sync
        </Button>
      </header>
      <ScrollArea className="min-h-0 flex-1">
        <div className="flex flex-col gap-0.5 px-1.5 py-1">
        {tree.map((node) => (
          <TreeNode
            key={node.path}
            node={node}
            depth={0}
            selectedRel={selectedRel}
            conflictSet={conflictSet}
            isOpen={isOpen}
            onToggle={onToggle}
            onSelect={(path) => {
              if (conflictSet.has(path) && selectedSlug) {
                openResolver(selectedSlug, path);
              } else {
                selectFile(path);
              }
            }}
          />
        ))}
        </div>
      </ScrollArea>
    </div>
  );
}
