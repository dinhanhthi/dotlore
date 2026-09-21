import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { ChevronDown, ChevronRight } from "lucide-react";

import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import { composeLivePath } from "@/lib/path";
import type { FileStatus, NodeWeight, TreeNode as TreeNodeData } from "@/lib/tree";
import { nodeStatus, nodeWeight } from "@/lib/tree";
import type { EntryView } from "@/lib/types";
import { cn } from "@/lib/utils";

import { coveringEntry, formatBytes, untrackTarget } from "./entries";

function weightClass(weight: NodeWeight): string {
  switch (weight) {
    case "ok":
      return "text-muted-foreground";
    case "warning":
      return "text-status-conflict";
    case "danger":
      return "text-destructive";
  }
}

function statusDotClass(status: FileStatus): string {
  switch (status) {
    case "synced":
      return "bg-status-synced";
    case "conflict":
      return "bg-status-conflict";
    case "pending":
      return "bg-status-pending";
  }
}

type TreeNodeProps = {
  node: TreeNodeData;
  depth: number;
  selectedRel: string | null;
  conflictSet: Set<string>;
  isOpen: (path: string, depth: number) => boolean;
  onToggle: (path: string, depth: number) => void;
  onSelect: (path: string) => void;
  entries: EntryView[];
  onUntrack: (entry: EntryView) => void;
  rootPath: string;
  maxFileBytes: number;
};

export function TreeNode({
  node,
  depth,
  selectedRel,
  conflictSet,
  isOpen,
  onToggle,
  onSelect,
  entries,
  onUntrack,
  rootPath,
  maxFileBytes,
}: TreeNodeProps) {
  const status = nodeStatus(node, conflictSet);
  const weight = nodeWeight(node, maxFileBytes);
  const selected = node.kind === "file" && selectedRel === node.path;
  const open = node.kind === "folder" && isOpen(node.path, depth);
  const covering = coveringEntry(node.path, entries);
  const target = untrackTarget(node.path, node.kind, entries);
  const overLimit =
    node.kind === "file" && weight === "danger"
      ? "Exceeds the size limit and is not being synced"
      : undefined;

  return (
    <>
      <ContextMenu>
        <ContextMenuTrigger
          render={
            <div
              className={cn(
                "group relative flex h-8 w-full items-center gap-2 rounded-2xl pr-2",
                "transition-colors duration-[var(--dur-short)] ease-[var(--ease-out)]",
                "hover:bg-muted/70",
                selected && "bg-muted",
              )}
              style={{ paddingLeft: 10 + depth * 14 }}
              title={covering ? `Covered by ${covering.key}` : undefined}
            />
          }
        >
          {node.kind === "folder" ? (
            <button
              type="button"
              aria-expanded={open}
              aria-label={open ? `Collapse ${node.name}` : `Expand ${node.name}`}
              onClick={() => onToggle(node.path, depth)}
              className="flex min-w-0 flex-1 items-center gap-1.5 text-left text-foreground"
            >
              {open ? (
                <ChevronDown aria-hidden className="size-4 shrink-0 text-muted-foreground" />
              ) : (
                <ChevronRight aria-hidden className="size-4 shrink-0 text-muted-foreground" />
              )}
              <span className="min-w-0 truncate text-sm">{node.name}</span>
            </button>
          ) : (
            <button
              type="button"
              onClick={() => onSelect(node.path)}
              className="flex min-w-0 flex-1 items-center gap-1.5 text-left text-foreground"
            >
              <span className="size-4 shrink-0" aria-hidden />
              <span className="min-w-0 truncate text-sm">{node.name}</span>
            </button>
          )}
          <span
            className={cn(
              "ml-auto shrink-0 text-right tabular-nums text-xs",
              weightClass(weight),
            )}
            title={overLimit}
          >
            {formatBytes(node.bytes)}
          </span>
          {covering ? (
            <span className="sr-only">Covered by {covering.key}</span>
          ) : null}
          <StatusMark status={status} />
        </ContextMenuTrigger>
        <ContextMenuContent className="min-w-40">
          <ContextMenuItem
            onClick={() => {
              void revealItemInDir(composeLivePath(rootPath, node.path)).catch(() => {
                // Path missing or the file manager is unavailable.
              });
            }}
          >
            Go to location
          </ContextMenuItem>
          {target ? (
            <>
              <ContextMenuSeparator />
              <ContextMenuItem variant="destructive" onClick={() => onUntrack(target)}>
                Untrack
              </ContextMenuItem>
            </>
          ) : null}
        </ContextMenuContent>
      </ContextMenu>
      {open &&
        node.children.map((child) => (
          <TreeNode
            key={child.path}
            node={child}
            depth={depth + 1}
            selectedRel={selectedRel}
            conflictSet={conflictSet}
            isOpen={isOpen}
            onToggle={onToggle}
            onSelect={onSelect}
            entries={entries}
            onUntrack={onUntrack}
            rootPath={rootPath}
            maxFileBytes={maxFileBytes}
          />
        ))}
    </>
  );
}

function StatusMark({ status }: { status: FileStatus }) {
  return (
    <span
      className="flex w-4 shrink-0 items-center justify-center"
      aria-label={status}
    >
      <span
        aria-hidden
        className={cn("size-2 rounded-full", statusDotClass(status))}
      />
    </span>
  );
}
