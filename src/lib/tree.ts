import type { FileSync, TrackedFile } from "./types";

export type TreeKind = "file" | "folder";

export type FileStatus = "synced" | "conflict" | "pending";

export type NodeWeight = "ok" | "notice" | "heavy" | "warning" | "danger";

export type TreeNode = {
  name: string;
  path: string;
  kind: TreeKind;
  children: TreeNode[];
  bytes: number;
  state: FileSync;
};

const STATUS_RANK: Record<FileStatus, number> = {
  synced: 0,
  pending: 1,
  conflict: 2,
};

const WEIGHT_RANK: Record<NodeWeight, number> = {
  ok: 0,
  notice: 1,
  heavy: 2,
  warning: 3,
  danger: 4,
};

const MB = 1024 * 1024;

function fileStatus(state: FileSync): FileStatus {
  return state === "Pending" ? "pending" : "synced";
}

/** File in `conflicts` → conflict; else from `state`. Folders take the worst child. */
export function nodeStatus(node: TreeNode, conflicts: Set<string>): FileStatus {
  if (node.kind === "file") {
    return conflicts.has(node.path) ? "conflict" : fileStatus(node.state);
  }
  let worst: FileStatus = "synced";
  for (const child of node.children) {
    const status = nodeStatus(child, conflicts);
    if (STATUS_RANK[status] > STATUS_RANK[worst]) worst = status;
  }
  return worst;
}

/** Bands a tree row uses for its byte count. */
export function bytesWeight(bytes: number, maxFileBytes: number): NodeWeight {
  if (bytes > maxFileBytes) return "danger";
  if (bytes >= maxFileBytes / 2) return "warning";
  if (bytes >= 4 * MB) return "heavy";
  if (bytes >= MB) return "notice";
  return "ok";
}

export function nodeWeight(node: TreeNode, maxFileBytes: number): NodeWeight {
  if (node.kind === "file") {
    if (node.state === "TooLarge") return "danger";
    return bytesWeight(node.bytes, maxFileBytes);
  }
  let worst: NodeWeight = "ok";
  for (const child of node.children) {
    const weight = nodeWeight(child, maxFileBytes);
    if (WEIGHT_RANK[weight] > WEIGHT_RANK[worst]) worst = weight;
  }
  return worst;
}

function compareNodes(a: TreeNode, b: TreeNode): number {
  if (a.kind !== b.kind) return a.kind === "folder" ? -1 : 1;
  return a.name.localeCompare(b.name);
}

function sortTree(nodes: TreeNode[]): void {
  nodes.sort(compareNodes);
  for (const node of nodes) sortTree(node.children);
}

function rollupBytes(nodes: TreeNode[]): number {
  let total = 0;
  for (const node of nodes) {
    if (node.kind === "folder") {
      node.bytes = rollupBytes(node.children);
    }
    total += node.bytes;
  }
  return total;
}

/**
 * Keep nodes whose path matches `query`, plus ancestor folders of a match.
 * With `keep`, a file must also pass `keep(rel)`, and folders keep only such files.
 */
export function filterTree(
  nodes: TreeNode[],
  query: string,
  keep?: (rel: string) => boolean,
): TreeNode[] {
  const needle = query.trim().toLowerCase();
  if (!needle && !keep) return nodes;
  const filtered: TreeNode[] = [];
  for (const node of nodes) {
    const next = filterNode(node, needle, keep);
    if (next) filtered.push(next);
  }
  return filtered;
}

/** Inclusive lower bound, exclusive upper bound, except `5-20` which includes 20MB. */
export type SizeBand = "1-5" | "5-20" | "over-20";

export const SIZE_BANDS: readonly { id: SizeBand; label: string }[] = [
  { id: "1-5", label: "1MB - 5MB" },
  { id: "5-20", label: "5MB - 20MB" },
  { id: "over-20", label: ">20MB" },
];

export function inSizeBand(bytes: number, band: SizeBand): boolean {
  switch (band) {
    case "1-5":
      return bytes >= MB && bytes < 5 * MB;
    case "5-20":
      return bytes >= 5 * MB && bytes <= 20 * MB;
    case "over-20":
      return bytes > 20 * MB;
  }
}

/**
 * Keep files whose size is in `band`. Ancestor folders stay so a match is
 * still reachable. A folder is never kept for its own rolled-up size, and
 * the byte count on a kept folder is left as-is.
 */
export function filterTreeBySize(nodes: TreeNode[], band: SizeBand): TreeNode[] {
  const filtered: TreeNode[] = [];
  for (const node of nodes) {
    const next = filterSizeNode(node, band);
    if (next) filtered.push(next);
  }
  return filtered;
}

function filterSizeNode(node: TreeNode, band: SizeBand): TreeNode | null {
  if (node.kind === "file") return inSizeBand(node.bytes, band) ? node : null;
  const children: TreeNode[] = [];
  for (const child of node.children) {
    const next = filterSizeNode(child, band);
    if (next) children.push(next);
  }
  if (children.length === 0) return null;
  return children.length === node.children.length ? node : { ...node, children };
}

function filterNode(
  node: TreeNode,
  needle: string,
  keep?: (rel: string) => boolean,
): TreeNode | null {
  const matches = !needle || node.path.toLowerCase().includes(needle);
  if (node.kind === "file") {
    return matches && (!keep || keep(node.path)) ? node : null;
  }
  if (matches && needle && !keep) return node;
  const children: TreeNode[] = [];
  for (const child of node.children) {
    const next = filterNode(child, needle, keep);
    if (next) children.push(next);
  }
  if (children.length === 0) return null;
  return { ...node, children };
}

export function buildTree(files: TrackedFile[]): TreeNode[] {
  const root: TreeNode[] = [];

  for (const file of files) {
    const parts = file.rel.split("/").filter((part) => part.length > 0);
    if (parts.length === 0) continue;

    let level = root;
    let prefix = "";
    for (let i = 0; i < parts.length; i++) {
      const name = parts[i]!;
      prefix = prefix ? `${prefix}/${name}` : name;
      const isFile = i === parts.length - 1;
      let node = level.find((existing) => existing.name === name);
      if (!node) {
        node = {
          name,
          path: prefix,
          kind: isFile ? "file" : "folder",
          children: [],
          bytes: isFile ? file.bytes : 0,
          state: isFile ? file.state : "Synced",
        };
        level.push(node);
      } else if (isFile) {
        node.bytes = file.bytes;
        node.state = file.state;
      } else {
        node.kind = "folder";
      }
      if (!isFile) level = node.children;
    }
  }

  sortTree(root);
  rollupBytes(root);
  return root;
}
