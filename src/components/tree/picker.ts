import type { EntryView, PickerRow } from "@/lib/types";

import { coveringEntry, explicitEntry } from "./entries";

export type PickerKind = "file" | "directory";
export type PendingAction = "track" | "untrack";
/** Include-list key → staged action. Absent means the server state stands. */
export type PendingMap = Record<string, PendingAction>;

export type PendingOp = {
  rel: string;
  kind: PickerKind;
  action: PendingAction;
};

export function entryKey(rel: string, kind: PickerKind): string {
  const base = rel.replace(/\/+$/, "");
  return kind === "directory" ? `${base}/` : base;
}

function ancestorAction(rel: string, pending: PendingMap): PendingAction | null {
  const path = rel.replace(/\/+$/, "");
  let best: { len: number; action: PendingAction } | null = null;
  for (const [key, action] of Object.entries(pending)) {
    if (!key.endsWith("/")) continue;
    const base = key.slice(0, -1);
    if (base.length === 0) continue;
    if (path === base || path.startsWith(`${base}/`)) {
      if (!best || key.length > best.len) best = { len: key.length, action };
    }
  }
  return best?.action ?? null;
}

/**
 * Tracked before staged marks.
 * Files follow the synced file list, so a hole punched in a parent folder
 * stays untracked. A folder is tracked when it is an explicit entry, or it
 * sits under a tracked folder and still contains a synced file.
 */
function baselineTracked(
  path: string,
  kind: PickerKind,
  entries: EntryView[],
  trackedRels: ReadonlySet<string>,
): boolean {
  if (kind === "file") return trackedRels.has(path);
  if (explicitEntry(path, entries)) return true;
  if (!coveringEntry(path, entries)) return false;
  const prefix = `${path}/`;
  for (const rel of trackedRels) {
    if (rel.startsWith(prefix)) return true;
  }
  return false;
}

/** Tracked after staged marks, including a parent folder that covers this path. */
export function isShownTracked(
  rel: string,
  kind: PickerKind,
  entries: EntryView[],
  pending: PendingMap,
  trackedRels: ReadonlySet<string>,
): boolean {
  const path = rel.replace(/\/+$/, "");
  const own = pending[entryKey(path, kind)];
  if (own === "track") return true;
  if (own === "untrack") return false;
  const ancestor = ancestorAction(path, pending);
  if (ancestor === "track") return true;
  if (ancestor === "untrack") return explicitEntry(path, entries) !== null;
  return baselineTracked(path, kind, entries, trackedRels);
}

/** A folder holds a path that is shown tracked, after staged marks. */
export function hasTrackedInside(
  rel: string,
  entries: EntryView[],
  pending: PendingMap,
  trackedRels: ReadonlySet<string>,
): boolean {
  const prefix = `${rel.replace(/\/+$/, "")}/`;
  const candidates: [string, PickerKind][] = [
    ...[...trackedRels].map((path): [string, PickerKind] => [path, "file"]),
    ...entries.map((entry): [string, PickerKind] => [entry.key, entry.kind]),
    ...Object.entries(pending)
      .filter(([, action]) => action === "track")
      .map(([key]): [string, PickerKind] => [
        key,
        key.endsWith("/") ? "directory" : "file",
      ]),
  ];
  return candidates.some(
    ([path, kind]) =>
      path.startsWith(prefix) &&
      path !== prefix &&
      isShownTracked(path, kind, entries, pending, trackedRels),
  );
}

/** A mark that repeats the state already shown without it is dropped. */
export function stagePending(
  pending: PendingMap,
  rel: string,
  kind: PickerKind,
  action: PendingAction,
  entries: EntryView[],
  trackedRels: ReadonlySet<string>,
): PendingMap {
  const key = entryKey(rel, kind);
  const without: PendingMap = { ...pending };
  delete without[key];
  const shown = isShownTracked(rel, kind, entries, without, trackedRels);
  if ((action === "track") === shown) return without;
  return { ...without, [key]: action };
}

function depth(rel: string): number {
  return rel.split("/").filter((part) => part.length > 0).length;
}

/**
 * Untrack a parent before tracking a path inside it.
 * Track a parent before untracking a path inside it.
 */
export function orderedPendingOps(pending: PendingMap): PendingOp[] {
  const ops: PendingOp[] = Object.entries(pending).map(([key, action]) => {
    const directory = key.endsWith("/");
    return {
      rel: directory ? key.slice(0, -1) : key,
      kind: directory ? "directory" : "file",
      action,
    };
  });
  const trackRels = new Set(
    ops.filter((op) => op.action === "track").map((op) => op.rel),
  );
  const underTrack = (rel: string) => {
    for (const parent of trackRels) {
      if (rel.startsWith(`${parent}/`)) return true;
    }
    return false;
  };
  const byDepth = (a: PendingOp, b: PendingOp) =>
    depth(a.rel) - depth(b.rel) || a.rel.localeCompare(b.rel);
  return [
    ...ops
      .filter((op) => op.action === "untrack" && !underTrack(op.rel))
      .sort(byDepth),
    ...ops.filter((op) => op.action === "track").sort(byDepth),
    ...ops
      .filter((op) => op.action === "untrack" && underTrack(op.rel))
      .sort(byDepth),
  ];
}

/**
 * A displayed row matches when its name contains `needle`, or an expanded
 * folder has a loaded child that matches. Collapsed and not-yet-loaded
 * folders are not searched inside.
 */
export function pickerRowMatchesQuery(
  row: PickerRow,
  needle: string,
  expanded: Readonly<Record<string, boolean>>,
  childrenByRel: Readonly<Record<string, readonly PickerRow[] | undefined>>,
): boolean {
  if (row.name.toLowerCase().includes(needle)) return true;
  if (row.kind !== "directory" || expanded[row.rel] !== true) return false;
  const children = childrenByRel[row.rel];
  if (!children) return false;
  return children.some((child) =>
    pickerRowMatchesQuery(child, needle, expanded, childrenByRel),
  );
}

export function sortPickerRows(rows: PickerRow[]): PickerRow[] {
  return rows
    .filter((row) => row.kind === "file" || row.kind === "directory")
    .sort((a, b) => {
      const rank = (kind: string) => (kind === "directory" ? 0 : 1);
      const byKind = rank(a.kind) - rank(b.kind);
      if (byKind !== 0) return byKind;
      return a.name.localeCompare(b.name);
    });
}

/** Drop staged marks when the dialog closes or the project changes. */
export function pickerStateAfterIdentityChange(): {
  pending: PendingMap;
  expanded: Record<string, boolean>;
} {
  return { pending: {}, expanded: {} };
}

/**
 * Total size of staged track marks.
 * A track inside a staged parent folder counts once, as the parent.
 * `null` is a measurement that failed and adds nothing.
 * `complete` is false while any included mark has not been measured yet.
 */
export function stagedTrackTotal(
  pending: PendingMap,
  bytesByKey: Readonly<Record<string, number | null>>,
): { bytes: number; complete: boolean } {
  const keys = Object.keys(pending).filter((key) => pending[key] === "track");
  let bytes = 0;
  let complete = true;
  for (const key of keys) {
    if (coveredByStagedFolder(key, keys)) continue;
    const size = bytesByKey[key];
    if (size === undefined) {
      complete = false;
      continue;
    }
    if (size !== null) bytes += size;
  }
  return { bytes, complete };
}

function coveredByStagedFolder(key: string, trackKeys: readonly string[]): boolean {
  const path = key.endsWith("/") ? key.slice(0, -1) : key;
  return trackKeys.some((other) => {
    if (other === key || !other.endsWith("/")) return false;
    const base = other.slice(0, -1);
    return path.startsWith(`${base}/`);
  });
}
