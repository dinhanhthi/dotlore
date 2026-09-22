import type { EntryView, InspectedEntryDto } from "@/lib/types";

export function explicitEntry(
  path: string,
  entries: EntryView[],
): EntryView | null {
  const dirKey = path.endsWith("/") ? path : `${path}/`;
  return entries.find((entry) => entry.key === path || entry.key === dirKey) ?? null;
}

export function coveringEntry(
  path: string,
  entries: EntryView[],
): EntryView | null {
  if (explicitEntry(path, entries)) return null;
  let best: EntryView | null = null;
  for (const entry of entries) {
    if (entry.kind !== "directory") continue;
    const base = entry.key.replace(/\/$/, "");
    if (path === base || path.startsWith(`${base}/`)) {
      if (!best || entry.key.length > best.key.length) best = entry;
    }
  }
  return best;
}

/** Parent of `rel`, or `null` at the picker root so navigation can hide. */
export function parentRel(rel: string): string | null {
  if (rel === "") return null;
  const index = rel.lastIndexOf("/");
  return index === -1 ? "" : rel.slice(0, index);
}

export function untrackCopy(entry: EntryView): string {
  const stay = "Files stay on disk.";
  if (entry.covering.length > 0) {
    return `This path stops syncing on every Mac. ${stay} ${entry.key} is excluded from ${entry.covering.join(", ")}.`;
  }
  return `The include entry ${entry.key} is removed on every Mac. ${stay} Files under this entry will stop syncing.`;
}

/** Explicit entry, or a synthesized hole-punch target for an inherited path. */
export function untrackTarget(
  path: string,
  kind: "file" | "folder",
  entries: EntryView[],
): EntryView | null {
  const explicit = explicitEntry(path, entries);
  if (explicit) return explicit;
  const covering = coveringEntry(path, entries);
  if (!covering) return null;
  return {
    key: kind === "folder" ? `${path.replace(/\/$/, "")}/` : path,
    kind: kind === "folder" ? "directory" : "file",
    covering: [covering.key],
  };
}

export function fileTooLarge(info: InspectedEntryDto): boolean {
  return info.kind === "file" && info.skipped_too_large.length > 0;
}

export function formatBytes(n: number): string {
  if (n < 1024) return `${n} bytes`;
  const kb = n / 1024;
  if (kb < 1024) return `${kb.toFixed(1)} KB`;
  return `${(kb / 1024).toFixed(1)} MB`;
}
