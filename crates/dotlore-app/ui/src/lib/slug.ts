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
 * `<parent>-<name without leading dots>`, sanitized.
 * Matches core `default_slug` except the `home_dir` parent → `home` special case
 * (the UI does not have `$HOME` / state-dir).
 */
export function defaultSlug(path: string): string {
  const parts = path.split("/").filter((part) => part.length > 0);
  const name = (parts.at(-1) ?? "").replace(/^\.+/, "");
  const parent = parts.at(-2) ?? "";
  const slug = sanitize(`${parent}-${name}`);
  return slug.length > 0 ? slug : "root";
}
