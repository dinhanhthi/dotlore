/** Lowercase `[a-z0-9-]`, runs of `-` collapsed, ends trimmed. Matches core `sanitize`. */
function sanitize(s: string): string {
  let out = "";
  for (const raw of s) {
    const c = raw.toLowerCase();
    if ((c >= "a" && c <= "z") || (c >= "0" && c <= "9")) {
      out += c;
    } else if (!out.endsWith("-")) {
      out += "-";
    }
  }
  return out.replace(/^-+|-+$/g, "");
}

/**
 * The chosen folder's name, leading dots stripped, then sanitized.
 * `/Users/x/git/dotlore` → `dotlore`, `/a/myproj/.claude` → `claude`.
 */
export function defaultSlug(path: string): string {
  const parts = path.split("/").filter((part) => part.length > 0);
  const name = (parts.at(-1) ?? "").replace(/^\.+/, "");
  const slug = sanitize(name);
  return slug.length > 0 ? slug : "root";
}
