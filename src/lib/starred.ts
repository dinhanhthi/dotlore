const STORAGE_KEY = "dotlore.starred";

function parseSlugArray(raw: string | null): string[] {
  if (!raw) return [];
  try {
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed.filter((item): item is string => typeof item === "string");
  } catch {
    return [];
  }
}

export function readStarred(): string[] {
  try {
    return parseSlugArray(localStorage.getItem(STORAGE_KEY));
  } catch {
    return [];
  }
}

export function writeStarred(slugs: string[]): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(slugs));
  } catch {
    // Quota or private-mode — keep the in-memory list.
  }
}

export function toggleStarred(slug: string): string[] {
  const current = readStarred();
  const next = current.includes(slug)
    ? current.filter((item) => item !== slug)
    : [...current, slug];
  writeStarred(next);
  return next;
}
