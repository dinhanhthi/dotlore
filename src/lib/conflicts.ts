import type { ConflictView } from "./types";

/** File, folder, or whole-root set of conflict views. */
export type ConflictScope = {
  kind: "file" | "folder" | "root";
  path?: string;
};

/** One cloud device, collapsed from every sibling that shares its id. */
export type CloudDevice = {
  id8: string;
  name: string;
  siblingRel: string;
};

/** Which side a keep-all batch writes for each live path. */
export type KeepAllChoice = "live" | "unnamed" | { deviceId: string };

/** One whole-file resolution inside a keep-all batch. */
export type KeepAllFile = {
  rel: string;
  keep: "live" | "other";
  siblingRel?: string;
};

function slashed(path: string): string {
  return String(path).replace(/\\/g, "/");
}

/** Staging-relative live path, always `/`-separated. */
export function conflictLiveRel(view: ConflictView): string {
  return slashed(view.live);
}

function siblingRel(view: ConflictView): string {
  return slashed(view.sibling);
}

/** Unique live paths, first-seen order — one entry per conflicted file. */
export function uniqueConflictRels(views: ConflictView[]): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const view of views) {
    const rel = conflictLiveRel(view);
    if (seen.has(rel)) continue;
    seen.add(rel);
    out.push(rel);
  }
  return out;
}

function viewsByLive(views: ConflictView[]): Map<string, ConflictView[]> {
  const grouped = new Map<string, ConflictView[]>();
  for (const view of views) {
    const rel = conflictLiveRel(view);
    const group = grouped.get(rel);
    if (group) group.push(view);
    else grouped.set(rel, [view]);
  }
  return grouped;
}

function inFolder(rel: string, folder: string): boolean {
  return rel === folder || rel.startsWith(`${folder}/`);
}

/** Views whose live path is the file, sits in the folder, or is every path for a root. */
export function viewsInScope(views: ConflictView[], scope: ConflictScope): ConflictView[] {
  if (scope.kind === "root") return views;
  const path = slashed(scope.path ?? "");
  if (scope.kind === "file") {
    return views.filter((view) => conflictLiveRel(view) === path);
  }
  return views.filter((view) => inFolder(conflictLiveRel(view), path));
}

/** Named cloud devices in first-seen order. This machine (`loserIsMe`) is omitted. */
export function cloudDevices(views: ConflictView[]): CloudDevice[] {
  const seen = new Set<string>();
  const out: CloudDevice[] = [];
  for (const view of views) {
    if (view.loserIsMe || seen.has(view.loserId8)) continue;
    seen.add(view.loserId8);
    out.push({
      id8: view.loserId8,
      name: view.loserName,
      siblingRel: siblingRel(view),
    });
  }
  return out;
}

/** True when some live path has only this machine's lost copy, with no named device. */
export function hasUnnamedCloud(views: ConflictView[]): boolean {
  for (const group of viewsByLive(views).values()) {
    if (group.every((view) => view.loserIsMe)) return true;
  }
  return false;
}

function keepLive(rel: string): KeepAllFile {
  return { rel, keep: "live" };
}

function keepOther(rel: string, view: ConflictView): KeepAllFile {
  return { rel, keep: "other", siblingRel: siblingRel(view) };
}

/** One keep target per live path in scope, in first-seen order. */
export function keepAllFiles(
  views: ConflictView[],
  scope: ConflictScope,
  choice: KeepAllChoice,
): KeepAllFile[] {
  const out: KeepAllFile[] = [];
  for (const [rel, group] of viewsByLive(viewsInScope(views, scope))) {
    if (choice === "live") {
      const mine = group.find((view) => view.loserIsMe);
      out.push(mine ? keepOther(rel, mine) : keepLive(rel));
    } else if (choice === "unnamed") {
      if (group.every((view) => view.loserIsMe)) out.push(keepLive(rel));
    } else {
      const device = group.find(
        (view) => !view.loserIsMe && view.loserId8 === choice.deviceId,
      );
      if (device) out.push(keepOther(rel, device));
    }
  }
  return out;
}
