import { ChevronDown, ChevronRight } from "lucide-react";

import type { FileStatus, TreeNode as TreeNodeData } from "@/lib/tree";
import { nodeStatus } from "@/lib/tree";
import { cn } from "@/lib/utils";

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
};

export function TreeNode({
  node,
  depth,
  selectedRel,
  conflictSet,
  isOpen,
  onToggle,
  onSelect,
}: TreeNodeProps) {
  const status = nodeStatus(node, conflictSet);
  const selected = node.kind === "file" && selectedRel === node.path;
  const open = node.kind === "folder" && isOpen(node.path, depth);

  return (
    <>
      <div
        className={cn(
          "group relative flex h-row w-full items-center gap-1",
          selected && "bg-white/[0.06]",
        )}
        style={{ paddingLeft: 8 + depth * 12, paddingRight: 8 }}
      >
        {selected && (
          <span aria-hidden className="absolute inset-y-0 left-0 w-0.5 bg-primary" />
        )}
        {node.kind === "folder" ? (
          <button
            type="button"
            aria-expanded={open}
            aria-label={open ? `Collapse ${node.name}` : `Expand ${node.name}`}
            onClick={() => onToggle(node.path, depth)}
            className="flex min-w-0 flex-1 items-center gap-1 text-left text-foreground"
          >
            {open ? (
              <ChevronDown aria-hidden className="size-3 shrink-0 text-muted-foreground" />
            ) : (
              <ChevronRight aria-hidden className="size-3 shrink-0 text-muted-foreground" />
            )}
            <StatusMark status={status} />
            <span className="min-w-0 truncate">{node.name}</span>
          </button>
        ) : (
          <button
            type="button"
            onClick={() => onSelect(node.path)}
            className="flex min-w-0 flex-1 items-center gap-1 text-left text-foreground"
          >
            <span className="size-3 shrink-0" aria-hidden />
            <StatusMark status={status} />
            <span className="min-w-0 truncate">{node.name}</span>
          </button>
        )}
      </div>
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
          />
        ))}
    </>
  );
}

function StatusMark({ status }: { status: FileStatus }) {
  return (
    <span className="flex shrink-0 items-center gap-0.5">
      <span
        aria-hidden
        className={cn("h-[6px] w-[6px] rounded-full", statusDotClass(status))}
      />
      {status === "conflict" && (
        <span aria-hidden className="text-[9px] leading-none text-status-conflict">
          ▲
        </span>
      )}
    </span>
  );
}
