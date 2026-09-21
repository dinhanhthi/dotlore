import type { FileSync, TrackedFile } from "./types";

export type TreeKind = "file" | "folder";

export type FileStatus = "synced" | "conflict" | "pending";

export type NodeWeight = "ok" | "warning" | "danger";

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
  warning: 1,
  danger: 2,
};

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

export function nodeWeight(node: TreeNode, maxFileBytes: number): NodeWeight {
  if (node.kind === "file") {
    if (node.state === "TooLarge" || node.bytes > maxFileBytes) return "danger";
    if (node.bytes >= maxFileBytes / 2) return "warning";
    return "ok";
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
