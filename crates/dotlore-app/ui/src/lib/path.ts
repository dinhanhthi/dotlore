/** Kind::File root is the file itself; Dir roots join `path` + `rel`. */
export function composeLivePath(rootPath: string, rel: string): string {
  const base = rootPath.replace(/[/\\]+$/, "");
  const norm = rel.replace(/\\/g, "/");
  const last = base.split("/").pop() ?? "";
  if (norm === last) return base;
  return `${base}/${norm}`;
}
