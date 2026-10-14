import { ChevronDown, ChevronRight } from "lucide-react";
import { useState, type ReactNode } from "react";

import { Badge } from "@/components/ui/badge";
import type { TreeNode } from "@/lib/tree";
import { passLabel, statusLabel, trashTree } from "@/lib/trash";
import type { TrashPass, TrashProject, TrashStatus } from "@/lib/types";
import { cn } from "@/lib/utils";

export type TrashSelection = {
  slug: string;
  day: string;
  pass: string;
  /** `null` for the whole pass. */
  rel: string | null;
  kind: "pass" | "folder" | "file";
};

type TrashTreeProps = {
  projects: TrashProject[];
  /** A search is active: every row stays open and clicks never fold. */
  filtering: boolean;
  nameOf: (slug: string) => string;
  selection: TrashSelection | null;
  onSelect: (selection: TrashSelection) => void;
};

/** Left inset shared by every row, then one chevron plus its gap per level. */
function indent(depth: number): string {
  return `calc(10px + ${depth} * (1rem + 0.375rem))`;
}

function sameItem(a: TrashSelection | null, b: TrashSelection): boolean {
  return (
    a !== null &&
    a.slug === b.slug &&
    a.day === b.day &&
    a.pass === b.pass &&
    a.rel === b.rel &&
    a.kind === b.kind
  );
}

function Chevron({ open }: { open: boolean }) {
  return open ? (
    <ChevronDown aria-hidden className="size-4 shrink-0 text-muted-foreground" />
  ) : (
    <ChevronRight aria-hidden className="size-4 shrink-0 text-muted-foreground" />
  );
}

function Row({
  depth,
  selected = false,
  expanded,
  onClick,
  children,
}: {
  depth: number;
  selected?: boolean;
  /** Undefined for a leaf row. */
  expanded?: boolean;
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      aria-expanded={expanded}
      aria-current={selected || undefined}
      onClick={onClick}
      className={cn(
        "flex h-8 w-full min-w-0 items-center gap-1.5 rounded-2xl pr-2 text-left text-foreground",
        "transition-colors duration-[var(--dur-short)] ease-[var(--ease-out)]",
        "hover:bg-muted/70",
        selected && "bg-muted",
      )}
      style={{ paddingLeft: indent(depth) }}
    >
      {expanded === undefined ? <span aria-hidden className="size-4 shrink-0" /> : null}
      {expanded !== undefined ? <Chevron open={expanded} /> : null}
      {children}
    </button>
  );
}

export function TrashTree({ projects, filtering, nameOf, selection, onSelect }: TrashTreeProps) {
  /** Everything starts open; this holds the rows the user folded. */
  const [collapsed, setCollapsed] = useState<ReadonlySet<string>>(new Set());

  function isOpen(key: string): boolean {
    return filtering || !collapsed.has(key);
  }

  function setOpen(key: string, open: boolean) {
    if (filtering) return;
    setCollapsed((current) => {
      if (open !== current.has(key)) return current;
      const next = new Set(current);
      if (open) next.delete(key);
      else next.add(key);
      return next;
    });
  }

  /** First click selects (and opens); a click on the selected row folds it. */
  function chooseBranch(key: string, item: TrashSelection) {
    if (sameItem(selection, item)) {
      setOpen(key, !isOpen(key));
    } else {
      setOpen(key, true);
      onSelect(item);
    }
  }

  function renderNodes(
    nodes: TreeNode[],
    pass: TrashPass,
    slug: string,
    statusByRel: Map<string, TrashStatus | null>,
    depth: number,
  ): ReactNode {
    return nodes.map((node) => {
      const item: TrashSelection = {
        slug,
        day: pass.day,
        pass: pass.pass,
        rel: node.path,
        kind: node.kind === "folder" ? "folder" : "file",
      };
      if (node.kind === "file") {
        const status = statusByRel.get(node.path) ?? null;
        return (
          <li key={node.path}>
            <Row depth={depth} selected={sameItem(selection, item)} onClick={() => onSelect(item)}>
              <span className="whitespace-nowrap text-sm">{node.name}</span>
              {status !== null ? (
                <Badge variant="secondary" className="shrink-0 font-normal">
                  {statusLabel(status)}
                </Badge>
              ) : null}
            </Row>
          </li>
        );
      }
      const key = `f:${slug}/${pass.day}/${pass.pass}/${node.path}`;
      const open = isOpen(key);
      return (
        <li key={node.path}>
          <Row
            depth={depth}
            selected={sameItem(selection, item)}
            expanded={open}
            onClick={() => chooseBranch(key, item)}
          >
            <span className="whitespace-nowrap text-sm">{node.name}</span>
          </Row>
          {open ? (
            <ul className="flex flex-col gap-0.5">
              {renderNodes(node.children, pass, slug, statusByRel, depth + 1)}
            </ul>
          ) : null}
        </li>
      );
    });
  }

  return (
    <ul className="flex w-max min-w-full flex-col gap-0.5">
      {projects.map((project) => {
        const key = `p:${project.slug}`;
        const open = isOpen(key);
        return (
          <li key={project.slug}>
            <Row depth={0} expanded={open} onClick={() => setOpen(key, !open)}>
              <span className="whitespace-nowrap text-sm font-medium">
                {nameOf(project.slug)}
              </span>
              {project.root === null ? (
                <span className="shrink-0 text-xs text-muted-foreground">Project removed</span>
              ) : null}
            </Row>
            {open ? (
              <ul className="flex flex-col gap-0.5">
                {project.passes.map((pass) => {
                  const passKey = `s:${project.slug}/${pass.day}/${pass.pass}`;
                  const passOpen = isOpen(passKey);
                  const item: TrashSelection = {
                    slug: project.slug,
                    day: pass.day,
                    pass: pass.pass,
                    rel: null,
                    kind: "pass",
                  };
                  const { nodes, statusByRel } = trashTree(pass.files);
                  return (
                    <li key={`${pass.day}/${pass.pass}`}>
                      <Row
                        depth={1}
                        selected={sameItem(selection, item)}
                        expanded={passOpen}
                        onClick={() => chooseBranch(passKey, item)}
                      >
                        <span className="whitespace-nowrap text-sm">
                          {passLabel(pass.day, pass.pass)}
                        </span>
                        <span className="shrink-0 tabular-nums text-xs text-muted-foreground">
                          {pass.files.length}
                        </span>
                      </Row>
                      {passOpen ? (
                        <ul className="flex flex-col gap-0.5">
                          {renderNodes(nodes, pass, project.slug, statusByRel, 2)}
                        </ul>
                      ) : null}
                    </li>
                  );
                })}
              </ul>
            ) : null}
          </li>
        );
      })}
    </ul>
  );
}
