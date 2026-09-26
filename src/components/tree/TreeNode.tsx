import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { ChevronDown, ChevronRight } from "lucide-react";

import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuSub,
  ContextMenuSubContent,
  ContextMenuSubTrigger,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import { composeLivePath } from "@/lib/path";
import type { FileStatus, NodeWeight, TreeNode as TreeNodeData } from "@/lib/tree";
import { nodeStatus, nodeWeight } from "@/lib/tree";
import type { ConflictView, EntryView, Sensitivity } from "@/lib/types";
import { cn } from "@/lib/utils";

import { coveringEntry, formatBytes, untrackTarget } from "./entries";
import { quickResolveItems } from "./quick-resolve";
import { SensitivityMark, sensitiveNameClass } from "./SensitivityMark";

/** Left inset shared by every row. */
const TREE_INSET = "10px";
/**
 * One level: the `size-4` chevron plus the `gap-1.5` before the folder label.
 * A child then starts where its parent's name starts.
 */
const TREE_LEVEL = "calc(1rem + 0.375rem)";

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
  sensitivityByRel: ReadonlyMap<string, Sensitivity | null>;
  conflictViews?: (rel: string) => ConflictView[] | undefined;
  onQuickResolve?: (rel: string, keep: "live" | "other", siblingRel?: string) => void;
  quickResolveDisabled?: (rel: string) => boolean;
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
  sensitivityByRel,
  conflictViews,
  onQuickResolve,
  quickResolveDisabled,
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
  const views = node.kind === "file" ? conflictViews?.(node.path) : undefined;
  const quick = views && views.length > 0 ? quickResolveItems(views) : null;
  const quickDisabled = quickResolveDisabled?.(node.path) ?? false;
  const sensitivity = node.kind === "file" ? sensitivityByRel.get(node.path) : null;

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
              style={{ paddingLeft: TREE_INSET }}
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
              className="flex min-w-0 flex-1 items-center text-left text-foreground"
            >
              <span
                className={cn("min-w-0 truncate text-sm", sensitiveNameClass(sensitivity))}
              >
                {node.name}
              </span>
            </button>
          )}
          <SensitivityMark sensitivity={sensitivity} />
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
          {quick && onQuickResolve ? (
            <>
              <ContextMenuSeparator />
              <ContextMenuItem
                disabled={quickDisabled}
                onClick={() => onQuickResolve(node.path, "live")}
              >
                {quick.live.label}
              </ContextMenuItem>
              {quick.cloud.length === 1 ? (
                <ContextMenuItem
                  disabled={quickDisabled}
                  onClick={() =>
                    onQuickResolve(node.path, "other", quick.cloud[0].siblingRel)
                  }
                >
                  {quick.cloud[0].label}
                </ContextMenuItem>
              ) : (
                <ContextMenuSub>
                  <ContextMenuSubTrigger disabled={quickDisabled}>
                    Keep cloud
                  </ContextMenuSubTrigger>
                  <ContextMenuSubContent className="min-w-40">
                    {quick.cloud.map((item) => (
                      <ContextMenuItem
                        key={item.siblingRel}
                        disabled={quickDisabled}
                        onClick={() =>
                          onQuickResolve(node.path, "other", item.siblingRel)
                        }
                      >
                        {item.label}
                      </ContextMenuItem>
                    ))}
                  </ContextMenuSubContent>
                </ContextMenuSub>
              )}
            </>
          ) : null}
        </ContextMenuContent>
      </ContextMenu>
      {open ? (
        <div
          className="relative flex w-full flex-col gap-0.5"
          style={{ paddingLeft: TREE_LEVEL }}
        >
          <span
            aria-hidden
            className="pointer-events-none absolute -top-0.5 bottom-0 z-10 w-[0.5px] -translate-x-1/2 bg-foreground/15"
            style={{ left: `calc(${TREE_INSET} + 0.5rem)` }}
          />
          {node.children.map((child) => (
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
              sensitivityByRel={sensitivityByRel}
              conflictViews={conflictViews}
              onQuickResolve={onQuickResolve}
              quickResolveDisabled={quickResolveDisabled}
            />
          ))}
        </div>
      ) : null}
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
