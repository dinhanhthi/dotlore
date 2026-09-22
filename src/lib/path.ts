export function composeLivePath(rootPath: string, rel: string): string {
  const base = rootPath.replace(/[/\\]+$/, "");
  const norm = rel.replace(/\\/g, "/");
  return `${base}/${norm}`;
}
