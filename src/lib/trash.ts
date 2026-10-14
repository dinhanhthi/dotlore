import { buildTree, filterTree, type TreeNode } from "./tree";
import type { TrashFile, TrashPass, TrashProject, TrashStatus } from "./types";

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

function fileRels(nodes: TreeNode[], into: Set<string>): Set<string> {
  for (const node of nodes) {
    if (node.kind === "file") into.add(node.path);
    else fileRels(node.children, into);
  }
  return into;
}

/**
 * Keep the files `filterTree` matches by name or path (case-insensitive);
 * passes and projects left with none are dropped. An empty query returns
 * `projects` itself.
 */
export function filterTrash(projects: TrashProject[], query: string): TrashProject[] {
  if (!query.trim()) return projects;
  return projects.flatMap((project) => {
    const passes = project.passes.flatMap((pass) => {
      const keep = fileRels(filterTree(trashTree(pass.files).nodes, query), new Set());
      const files = pass.files.filter((file) => keep.has(file.rel));
      return files.length > 0 ? [{ ...pass, files }] : [];
    });
    return passes.length > 0 ? [{ ...project, passes }] : [];
  });
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

/**
 * `YYYY-MM-DD` for `months` calendar months before `now`, from local date
 * fields; the day is clamped to the target month's last day (Mar 31 → Feb 28).
 */
export function cutoffDay(now: Date, months: number): string {
  const total = now.getFullYear() * 12 + now.getMonth() - months;
  const year = Math.floor(total / 12);
  const month = total - year * 12;
  const lastDay = new Date(year, month + 1, 0).getDate();
  const day = Math.min(now.getDate(), lastDay);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${year}-${pad(month + 1)}-${pad(day)}`;
}

export interface TrashCounts {
  passes: number;
  files: number;
  bytes: number;
}

function countPasses(passes: TrashPass[]): TrashCounts {
  const counts = { passes: 0, files: 0, bytes: 0 };
  for (const pass of passes) {
    counts.passes += 1;
    counts.files += pass.files.length;
    for (const file of pass.files) counts.bytes += file.bytes;
  }
  return counts;
}

/** Passes, files and bytes over every pass of every project. */
export function trashTotals(projects: TrashProject[]): TrashCounts {
  return countPasses(projects.flatMap((project) => project.passes));
}

/** Like `trashTotals`, over dated passes whose day is before `before` (`YYYY-MM-DD`). */
export function olderThan(projects: TrashProject[], before: string): TrashCounts {
  return countPasses(
    projects
      .flatMap((project) => project.passes)
      .filter((pass) => pass.day !== "undated" && pass.day < before),
  );
}
