export type TreeKind = "file" | "folder";

export type FileStatus = "synced" | "conflict" | "pending";

export type TreeNode = {
  name: string;
  path: string;
  kind: TreeKind;
  children: TreeNode[];
};

const STATUS_RANK: Record<FileStatus, number> = {
  synced: 0,
  pending: 1,
  conflict: 2,
};

/** File in `conflicts` → conflict; else synced. Folders take the worst child. */
export function nodeStatus(node: TreeNode, conflicts: Set<string>): FileStatus {
  if (node.kind === "file") {
    return conflicts.has(node.path) ? "conflict" : "synced";
  }
  let worst: FileStatus = "synced";
  for (const child of node.children) {
    const status = nodeStatus(child, conflicts);
    if (STATUS_RANK[status] > STATUS_RANK[worst]) worst = status;
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

export function buildTree(paths: string[]): TreeNode[] {
  const root: TreeNode[] = [];

  for (const raw of paths) {
    const parts = raw.split("/").filter((part) => part.length > 0);
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
        };
        level.push(node);
      } else if (!isFile) {
        node.kind = "folder";
      }
      if (!isFile) level = node.children;
    }
  }

  sortTree(root);
  return root;
}
