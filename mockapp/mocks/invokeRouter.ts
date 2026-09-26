import { defaultSlug } from "@/lib/slug";
import type {
  EntryKind,
  EntryView,
  InspectedEntryDto,
  PickerRow,
  RootRow,
} from "@/lib/types";

import {
  mockSensitivity,
  toFileContent,
  toTrackedFile,
  type FileRecord,
} from "../fixtures/files";
import { __emit } from "./event";
import { MOCK_FILE_PATHS } from "./plugin-dialog";
import {
  fileRecord,
  gdriveMounts,
  icloudPath,
  resolutionFor,
  store,
  syncConflictCount,
} from "./store";

function maxFileBytes(): number {
  return store.maxFileMb * 1024 * 1024;
}

function maxSeedFolderBytes(): number {
  return store.maxSeedFolderMb * 1024 * 1024;
}

function argString(args: Record<string, unknown>, key: string): string {
  const value = args[key];
  if (typeof value !== "string" || value.length === 0) {
    throw new Error(`mockapp: ${key} is required`);
  }
  return value;
}

function emitStatus(error: string | null = null): void {
  __emit("dotlore://status", { roots: store.roots, error });
}

function nameFromPath(path: string): string {
  const parts = path.split("/").filter((part) => part.length > 0);
  return parts.at(-1) ?? path;
}

function requireRoot(slug: string): RootRow {
  const row = store.roots.find((item) => item.slug === slug);
  if (!row) throw new Error(`Unknown root: ${slug}`);
  return row;
}

function plainRel(rel: string, allowEmpty: boolean): boolean {
  if (rel === "") return allowEmpty;
  if (rel.startsWith("/")) return false;
  return rel.split("/").every((part) => part.length > 0 && part !== "." && part !== "..");
}

function recordBytes(record: FileRecord): number {
  if (typeof record.bytes === "number") return record.bytes;
  return toFileContent(record).bytes_len;
}

function fileOverLimit(record: FileRecord): boolean {
  return record.too_large || recordBytes(record) > maxFileBytes();
}

function keysOverlap(a: string, b: string): boolean {
  const aBase = a.replace(/\/$/, "");
  const bBase = b.replace(/\/$/, "");
  const under = (entry: string, ancestor: string): boolean => {
    const trimmed = entry.replace(/\/$/, "");
    return ancestor === "" ? trimmed.length > 0 : trimmed.startsWith(`${ancestor}/`);
  };
  return (
    (a.endsWith("/") && (bBase === aBase || under(b, aBase))) ||
    (b.endsWith("/") && (aBase === bBase || under(a, bBase)))
  );
}

function withCovering(entries: EntryView[]): EntryView[] {
  return entries
    .map((entry) => ({
      ...entry,
      covering: entries
        .filter((other) => other.key !== entry.key && keysOverlap(other.key, entry.key))
        .map((other) => other.key),
    }))
    .sort((a, b) => a.key.localeCompare(b.key));
}

function isExcluded(rel: string, excludes: string[]): boolean {
  return excludes.some((key) => {
    if (key.endsWith("/")) {
      const base = key.replace(/\/$/, "");
      return rel === base || rel.startsWith(`${base}/`);
    }
    return rel === key;
  });
}

function coveredBy(rel: string, entries: EntryView[], excludes: string[] = []): boolean {
  if (entries.some((entry) => entry.key === rel || entry.key === `${rel}/`)) {
    return true;
  }
  if (isExcluded(rel, excludes)) return false;
  return entries.some((entry) => {
    if (entry.kind === "file") return entry.key === rel;
    const base = entry.key.replace(/\/$/, "");
    return rel === base || rel.startsWith(`${base}/`);
  });
}

function excludeKey(slug: string, rel: string): string {
  const files = store.files[slug] ?? {};
  if (files[rel]) return rel;
  const prefix = `${rel}/`;
  if (Object.keys(files).some((path) => path.startsWith(prefix))) {
    return prefix;
  }
  return rel;
}

function dropExclude(slug: string, key: string): void {
  const list = store.excludes[slug];
  if (!list) return;
  store.excludes[slug] = list.filter((item) => item !== key && item !== `${key}/`);
}

function coveringKey(entries: EntryView[], rel: string): string | undefined {
  return entries.find((entry) => {
    if (entry.kind !== "directory") return false;
    const base = entry.key.replace(/\/$/, "");
    return rel !== base && rel.startsWith(`${base}/`);
  })?.key;
}

function inspectPreview(slug: string, rel: string): InspectedEntryDto {
  if (!plainRel(rel, false)) throw new Error(`unsafe path ${rel}`);
  const files = store.files[slug] ?? {};
  if (files[rel]) {
    const bytes = recordBytes(files[rel]);
    const over = fileOverLimit(files[rel]);
    return {
      kind: "file",
      bytes: over ? 0 : bytes,
      folder_limit: maxSeedFolderBytes(),
      confirmation_required: false,
      skipped_too_large: over ? [{ rel, bytes }] : [],
      sensitivity: mockSensitivity(rel, store.sensitivePatterns),
      secret_descendants: [],
      secret_descendants_more: false,
    };
  }
  const prefix = `${rel}/`;
  let bytes = 0;
  const skipped: InspectedEntryDto["skipped_too_large"] = [];
  const secrets: string[] = [];
  for (const [path, record] of Object.entries(files)) {
    if (path !== rel && !path.startsWith(prefix)) continue;
    if (mockSensitivity(path, store.sensitivePatterns) === "secret") secrets.push(path);
    const size = recordBytes(record);
    if (fileOverLimit(record)) skipped.push({ rel: path, bytes: size });
    else bytes += size;
  }
  return {
    kind: "directory",
    bytes,
    folder_limit: maxSeedFolderBytes(),
    confirmation_required: bytes > maxSeedFolderBytes(),
    skipped_too_large: skipped,
    sensitivity: null,
    secret_descendants: secrets.sort().slice(0, 20),
    secret_descendants_more: secrets.length > 20,
  };
}

function listChildren(slug: string, rel: string): PickerRow[] {
  if (!plainRel(rel, true)) throw new Error(`unsafe path ${rel}`);
  const names = new Map<string, string>();
  const prefix = rel ? `${rel}/` : "";
  const consider = (path: string, asDir: boolean): void => {
    if (rel === "") {
      const [first, ...rest] = path.split("/");
      if (!first) return;
      names.set(first, rest.length || asDir ? "directory" : "file");
      return;
    }
    if (!path.startsWith(prefix)) return;
    const rest = path.slice(prefix.length);
    if (!rest) return;
    const [first, ...more] = rest.split("/");
    if (!first) return;
    names.set(first, more.length || asDir ? "directory" : "file");
  };
  for (const path of Object.keys(store.files[slug] ?? {})) {
    consider(path, false);
  }
  for (const entry of store.entries[slug] ?? []) {
    consider(entry.key.replace(/\/$/, ""), entry.kind === "directory");
  }
  for (const extra of store.pickerExtra[slug]?.[rel] ?? []) {
    if (extra.kind === "symlink") continue;
    names.set(extra.name, extra.kind);
  }
  return [...names.entries()]
    .map(([name, kind]) => ({
      name,
      kind,
      rel: rel ? `${rel}/${name}` : name,
      sensitivity:
        kind === "file"
          ? mockSensitivity(rel ? `${rel}/${name}` : name, store.sensitivePatterns)
          : null,
    }))
    .sort((a, b) => a.name.localeCompare(b.name));
}

function patternMatchesRel(pattern: string, rel: string): boolean {
  if (pattern.endsWith("/")) {
    const base = pattern.replace(/\/$/, "");
    return rel === base || rel.startsWith(`${base}/`);
  }
  return rel === pattern;
}

function emptyFileRecord(): FileRecord {
  return { text: "", binary: false, too_large: false, bytes: 0, state: "Synced" };
}

/** Dropdown order. `projects` lines come from `store.defaultPatterns`; other ids use a short builtin. */
const PATTERN_CATALOGS: readonly {
  id: string;
  label: string;
  builtin: readonly string[];
}[] = [
  { id: "projects", label: "Projects", builtin: [] },
  { id: "claude", label: "Claude", builtin: ["CLAUDE.md"] },
  { id: "codex", label: "Codex", builtin: ["AGENTS.md"] },
  { id: "cursor", label: "Cursor", builtin: [".cursor/"] },
  { id: "gemini", label: "Gemini", builtin: ["GEMINI.md"] },
  { id: "opencode", label: "OpenCode", builtin: ["opencode.json"] },
  { id: "continue", label: "Continue", builtin: ["config.yaml"] },
  { id: "junie", label: "Junie", builtin: ["guidelines.md"] },
  { id: "kiro", label: "Kiro", builtin: ["steering/"] },
  { id: "roo", label: "Roo", builtin: ["rules/"] },
  { id: "cline", label: "Cline", builtin: ["skills/"] },
  { id: "windsurf", label: "Windsurf", builtin: [".windsurfrules"] },
  { id: "other", label: "Other agents", builtin: ["AGENTS.md", "skills/"] },
];

function stringPatterns(value: unknown): string[] {
  if (!Array.isArray(value)) {
    throw new Error("mockapp: patterns is required");
  }
  return value.filter((item): item is string => typeof item === "string");
}

function catalogLines(id: string, builtin: readonly string[]): string[] {
  if (id === "projects") return store.defaultPatterns;
  if (Object.hasOwn(store.agentPatterns, id)) return store.agentPatterns[id] ?? [];
  return [...builtin];
}

/** Seed include-list entries from `defaultPatterns` — match fixture files or create empty keys. */
function seedEntriesFromDefaults(slug: string): EntryView[] {
  const files = store.files[slug] ?? (store.files[slug] = {});
  const seeded: EntryView[] = [];
  for (const pattern of store.defaultPatterns) {
    const isDir = pattern.endsWith("/");
    const matches = Object.keys(files).filter((rel) => patternMatchesRel(pattern, rel));
    if (matches.length === 0 && !isDir) {
      files[pattern] = files[pattern] ?? emptyFileRecord();
    }
    seeded.push({
      key: pattern,
      kind: isDir ? "directory" : "file",
      covering: [],
    });
  }
  return withCovering(seeded);
}

function confirmedFolderBytes(args: Record<string, unknown>): number | null {
  if (typeof args.confirmedFolderBytes === "number") {
    return args.confirmedFolderBytes;
  }
  if (typeof args.confirmed_folder_bytes === "number") {
    return args.confirmed_folder_bytes;
  }
  return null;
}

const handlers: Record<
  string,
  (args: Record<string, unknown>) => unknown
> = {
  list_roots: () => store.roots,
  tracked_files: (args) => {
    const slug = argString(args, "slug");
    const entries = store.entries[slug] ?? [];
    return Object.entries(store.files[slug] ?? {})
      .filter(([rel]) => coveredBy(rel, entries, store.excludes[slug] ?? []))
      .map(([rel, record]) => toTrackedFile(rel, record, store.sensitivePatterns));
  },
  read_file: (args) => {
    const slug = argString(args, "slug");
    const rel = argString(args, "rel");
    const record = fileRecord(slug, rel);
    if (!record) throw new Error(`No such file: ${slug}/${rel}`);
    return toFileContent(record);
  },
  conflicts: (args) => store.conflicts[argString(args, "slug")] ?? [],
  open_resolution: (args) =>
    resolutionFor(argString(args, "slug"), argString(args, "rel")),
  close_resolution: () => undefined,
  resolve_conflict: (args) => {
    const slug = argString(args, "slug");
    const rel = argString(args, "rel");
    const content = typeof args.content === "string" ? args.content : "";
    const files = store.files[slug] ?? (store.files[slug] = {});
    files[rel] = { text: content, binary: false, too_large: false };
    store.conflicts[slug] = (store.conflicts[slug] ?? []).filter(
      (view) => view.live !== rel,
    );
    syncConflictCount(slug);
    emitStatus();
    return { outcome: "applied" };
  },
  resolve_binary: (args) => {
    const slug = argString(args, "slug");
    const rel = argString(args, "rel");
    if (args.keep === "other") {
      const { binary, siblings } = resolutionFor(slug, rel);
      const path = typeof args.sibling === "string" ? args.sibling : null;
      const kept = path ? siblings.find((item) => item.path === path) : siblings[0];
      if (!kept || (!path && siblings.length > 1)) {
        throw new Error(`mockapp: no single sibling to keep for ${rel}`);
      }
      if (!binary && kept.text !== null) {
        const files = store.files[slug] ?? (store.files[slug] = {});
        files[rel] = { text: kept.text, binary: false, too_large: false };
      }
    }
    store.conflicts[slug] = (store.conflicts[slug] ?? []).filter(
      (view) => view.live !== rel,
    );
    syncConflictCount(slug);
    emitStatus();
    return { outcome: "applied" };
  },
  provider_dir: () => store.providerDir,
  git_missing: () => store.gitMissing,
  sync_now: () => {
    emitStatus();
  },
  set_provider: (args) => {
    store.providerDir = argString(args, "dir");
  },
  add_root: (args) => {
    const path = argString(args, "path");
    if (MOCK_FILE_PATHS.has(path)) {
      throw new Error(`${path} is not a directory`);
    }
    const slug =
      typeof args.slug === "string" && args.slug.length > 0
        ? args.slug
        : defaultSlug(path);
    if (store.roots.some((row) => row.slug === slug)) {
      throw new Error(`Slug already tracked: ${slug}`);
    }
    store.roots.push({
      slug,
      path,
      name: nameFromPath(path),
      is_agent: false,
      linked: true,
      status: { kind: "Synced" },
    });
    store.files[slug] = store.files[slug] ?? {};
    store.entries[slug] = seedEntriesFromDefaults(slug);
    emitStatus();
    return slug;
  },
  import_installed_agents: () => ({ added: [], failed: [] }),
  link_root: (args) => {
    const slug = argString(args, "slug");
    const path = argString(args, "path");
    if (store.roots.some((row) => row.slug === slug)) {
      throw new Error(`Slug already tracked: ${slug}`);
    }
    store.roots.push({
      slug,
      path,
      name: nameFromPath(path),
      is_agent: false,
      linked: true,
      status: { kind: "Synced" },
    });
    store.files[slug] = store.files[slug] ?? {
      "CLAUDE.md": {
        text: `# ${slug}\n`,
        binary: false,
        too_large: false,
      },
    };
    store.entries[slug] = store.entries[slug] ?? [
      { key: "CLAUDE.md", kind: "file", covering: [] },
    ];
    store.linkable = store.linkable.filter((item) => item.slug !== slug);
    emitStatus();
  },
  remove_root: (args) => {
    const slug = argString(args, "slug");
    store.roots = store.roots.filter((row) => row.slug !== slug);
    delete store.files[slug];
    delete store.conflicts[slug];
    delete store.entries[slug];
    delete store.pickerExtra[slug];
    emitStatus();
  },
  wipe_cloud_data: () => {
    const readded = store.roots.map((row) => row.slug);
    emitStatus();
    return { readded, failed: [] };
  },
  recover_root: (args) => {
    const row = requireRoot(argString(args, "slug"));
    row.status = { kind: "Synced" };
    if (row.path.includes("/gone/")) {
      row.path = `/Users/demo/Recovered/${row.slug}`;
    }
    emitStatus();
  },
  list_linkable: () =>
    store.linkable.filter(
      (row) => !store.roots.some((item) => item.slug === row.slug),
    ),
  list_entries: (args) =>
    withCovering(store.entries[argString(args, "slug")] ?? []),
  list_entry_children: (args) =>
    listChildren(argString(args, "slug"), String(args.rel ?? "")),
  inspect_entry: (args) =>
    inspectPreview(argString(args, "slug"), argString(args, "rel")),
  track_entry: (args) => {
    const slug = argString(args, "slug");
    const rel = argString(args, "rel");
    const preview = inspectPreview(slug, rel);
    const secrets =
      preview.kind === "file"
        ? preview.sensitivity === "secret"
          ? [rel]
          : []
        : preview.secret_descendants;
    if (
      secrets.length > 0 &&
      args.confirmedSensitive !== true &&
      args.confirmed_sensitive !== true
    ) {
      return {
        outcome: "confirm_sensitive",
        paths: secrets,
        more: preview.secret_descendants_more,
      };
    }
    if (preview.kind === "file") {
      const skipped = preview.skipped_too_large[0];
      if (skipped) {
        throw new Error(
          `${rel} is ${skipped.bytes} bytes (limit ${maxFileBytes()} bytes)`,
        );
      }
    } else if (preview.confirmation_required) {
      const approved = confirmedFolderBytes(args) ?? 0;
      if (approved < preview.bytes) {
        return {
          outcome: "needs_confirmation",
          bytes: preview.bytes,
          folder_limit: preview.folder_limit,
          confirmation_required: true,
          skipped_too_large: preview.skipped_too_large,
        };
      }
    }
    const kind: EntryKind = preview.kind;
    const key = kind === "directory" ? (rel.endsWith("/") ? rel : `${rel}/`) : rel;
    const list = store.entries[slug] ?? (store.entries[slug] = []);
    if (!list.some((entry) => entry.key === key)) {
      list.push({ key, kind, covering: [] });
    }
    dropExclude(slug, key);
    store.entries[slug] = withCovering(list);
    return { outcome: "done" };
  },
  untrack_entry: (args) => {
    const slug = argString(args, "slug");
    const rel = argString(args, "rel");
    if (!plainRel(rel, false)) throw new Error(`unsafe path ${rel}`);
    const list = store.entries[slug] ?? [];
    const exact = list.find((entry) => entry.key === rel || entry.key === `${rel}/`);
    const cover = coveringKey(list, rel);
    if (exact && !cover) {
      store.entries[slug] = withCovering(list.filter((entry) => entry.key !== exact.key));
      return store.entries[slug];
    }
    if (cover || exact) {
      if (exact) {
        store.entries[slug] = withCovering(list.filter((entry) => entry.key !== exact.key));
      }
      const key = excludeKey(slug, rel);
      const excluded = store.excludes[slug] ?? (store.excludes[slug] = []);
      if (!excluded.includes(key)) excluded.push(key);
      return store.entries[slug] ?? [];
    }
    throw new Error(`cannot untrack ${rel}: not an explicit include entry`);
  },
  icloud_dir: () => icloudPath(),
  list_gdrive_mounts: () => gdriveMounts(),
  login_item_enabled: () => store.loginItem,
  update_available: () => null,
  prompt_update: () => undefined,
  set_login_item: (args) => {
    store.loginItem = Boolean(args.on);
  },
  default_patterns: () => store.defaultPatterns,
  set_default_patterns: (args) => {
    store.defaultPatterns = stringPatterns(args.patterns);
  },
  sensitive_patterns: () => store.sensitivePatterns,
  set_sensitive_patterns: (args) => {
    store.sensitivePatterns = stringPatterns(args.patterns);
  },
  pattern_catalogs: () =>
    PATTERN_CATALOGS.map(({ id, label, builtin }) => ({
      id,
      label,
      lines: catalogLines(id, builtin),
    })),
  set_pattern_catalog: (args) => {
    const catalog = argString(args, "catalog");
    const patterns = stringPatterns(args.patterns);
    if (catalog === "projects") {
      store.defaultPatterns = patterns;
      return;
    }
    if (!PATTERN_CATALOGS.some((row) => row.id === catalog)) {
      throw new Error(`unknown pattern catalog ${catalog}`);
    }
    store.agentPatterns[catalog] = patterns;
  },
  default_ignore: () => store.defaultIgnore,
  set_default_ignore: (args) => {
    store.defaultIgnore = typeof args.ignore === "string" ? args.ignore : "";
  },
  max_file_mb: () => store.maxFileMb,
  set_max_file_mb: (args) => {
    if (typeof args.mb !== "number" || !Number.isFinite(args.mb)) {
      throw new Error("mockapp: mb is required");
    }
    store.maxFileMb = args.mb;
  },
  max_seed_folder_mb: () => store.maxSeedFolderMb,
  set_max_seed_folder_mb: (args) => {
    if (typeof args.mb !== "number" || !Number.isFinite(args.mb)) {
      throw new Error("mockapp: mb is required");
    }
    store.maxSeedFolderMb = args.mb;
  },
};

export async function route(
  cmd: string,
  args: Record<string, unknown>,
): Promise<unknown> {
  const handler = handlers[cmd];
  if (!handler) {
    console.info(`[mockapp] invoke '${cmd}'`, args, "→ unhandled");
    throw new Error(`mockapp: no handler for ${cmd}`);
  }
  return handler(args);
}
