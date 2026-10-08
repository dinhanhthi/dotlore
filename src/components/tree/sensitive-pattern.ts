/**
 * Anchored gitignore line for `rel`: glob metacharacters escaped, and each
 * trailing whitespace character wrapped in a one-character `{c}` group so the
 * backend's `str::trim` (which also strips U+0085) cannot drop it. Not `[c]`:
 * the backend's globset matches a bracket class one byte at a time, so a
 * multi-byte character such as NBSP never matches inside one.
 */
export function sensitivePatternFor(rel: string): string {
  const [, body, trailing] = /^([\s\S]*?)([\s\u0085]*)$/.exec(rel)!;
  const escaped = body.replace(/[\\*?[\]{}]/g, "\\$&");
  return `/${escaped}${[...trailing].map((c) => `{${c}}`).join("")}`;
}

/** `patterns` with `pattern` appended, or the same list when a line already matches it. */
export function withSensitivePattern(patterns: string[], pattern: string): string[] {
  const wanted = pattern.trim();
  if (patterns.some((line) => line.trim() === wanted)) return patterns;
  return [...patterns, pattern];
}
