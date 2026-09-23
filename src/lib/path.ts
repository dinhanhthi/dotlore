export function composeLivePath(rootPath: string, rel: string): string {
  const base = rootPath.replace(/[/\\]+$/, "");
  const norm = rel.replace(/\\/g, "/");
  return `${base}/${norm}`;
}

/**
 * Shorten a path from the middle, keeping both ends.
 *
 * A CSS `truncate` drops the tail, which is the part that names the folder —
 * two Google Drive accounts differ only after a long identical prefix.
 */
export function middleEllipsis(path: string, max = 46): string {
  if (path.length <= max) return path;
  const keep = Math.max(0, max - 1);
  const tail = Math.ceil(keep / 2);
  return `${path.slice(0, keep - tail)}…${path.slice(path.length - tail)}`;
}
