import type { ConflictView } from "./types";

/** Staging-relative live path, always `/`-separated. */
export function conflictLiveRel(view: ConflictView): string {
  return String(view.live).replace(/\\/g, "/");
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
