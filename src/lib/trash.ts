import { buildTree, type TreeNode } from "./tree";
import type { TrashFile, TrashProject, TrashStatus } from "./types";

const MONTHS = [
  "Jan",
  "Feb",
  "Mar",
  "Apr",
  "May",
  "Jun",
  "Jul",
  "Aug",
  "Sep",
  "Oct",
  "Nov",
  "Dec",
];

/** " (N+1)" for a `-N` sibling pass, "" otherwise. */
function siblingSuffix(n: string | undefined): string {
  return n === undefined ? "" : ` (${Number(n) + 1})`;
}

/**
 * "Oct 13, 2026, 14:25:01" for `YYYY-MM-DD` + `HHMMSS[-N]`, "Undated" for
 * `undated`. Built from the digits, so it never depends on locale or time zone.
 * Unrecognised names fall back to `day pass`.
 */
export function passLabel(day: string, pass: string): string {
  if (day === "undated") {
    const undated = /^\d+(?:-(\d+))?$/.exec(pass);
    if (undated) return `Undated${siblingSuffix(undated[1])}`;
  }
  const date = /^(\d{4})-(\d{2})-(\d{2})$/.exec(day);
  const time = /^(\d{2})(\d{2})(\d{2})(?:-(\d+))?$/.exec(pass);
  const month = date ? MONTHS[Number(date[2]) - 1] : undefined;
  if (!date || !time || !month) return `${day} ${pass}`;
  return (
    `${month} ${Number(date[3])}, ${date[1]}, ${time[1]}:${time[2]}:${time[3]}` +
    siblingSuffix(time[4])
  );
}

/** Folder/file tree of a pass via `buildTree`, plus each file's status by rel. */
export function trashTree(files: TrashFile[]): {
  nodes: TreeNode[];
  statusByRel: Map<string, TrashStatus | null>;
} {
  const nodes = buildTree(
    files.map((file) => ({
      rel: file.rel,
      bytes: file.bytes,
      state: "Synced",
      sensitivity: null,
    })),
  );
  const statusByRel = new Map(files.map((file) => [file.rel, file.status]));
  return { nodes, statusByRel };
}

/** `null` → every file; a file rel → that file; a folder rel → every file below it. */
export function filesUnder(files: TrashFile[], rel: string | null): TrashFile[] {
  if (rel === null) return files;
  const prefix = `${rel}/`;
  return files.filter((file) => file.rel === rel || file.rel.startsWith(prefix));
}

export function statusLabel(status: TrashStatus | null): string {
  switch (status) {
    case "not_in_project":
      return "Not in project";
    case "differs":
      return "Differs from project";
    case "same":
      return "Same as project";
    case "unknown":
      return "Cannot compare";
    case null:
      return "Project removed";
  }
}

/** A removed project (no root) has nowhere to restore into. */
export function canRestore(project: TrashProject): boolean {
  return project.root !== null;
}
