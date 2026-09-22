/** Case-insensitive alphabetical order. `.claude` sorts before `.codex`. */
export function compareAlpha(a: string, b: string): number {
  return a.localeCompare(b, "en", { numeric: true, sensitivity: "base" });
}

export function compareRoots(
  a: { name: string; slug: string },
  b: { name: string; slug: string },
): number {
  return compareAlpha(a.name, b.name) || compareAlpha(a.slug, b.slug);
}
