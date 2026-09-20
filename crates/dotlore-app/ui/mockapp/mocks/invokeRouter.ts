import { defaultSlug } from "@/lib/slug";
import type { RootRow } from "@/lib/types";

import { toFileContent } from "../fixtures/files";
import { __emit } from "./event";
import {
  fileRecord,
  gdriveMounts,
  icloudPath,
  resolutionFor,
  store,
  syncConflictCount,
} from "./store";

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

const handlers: Record<
  string,
  (args: Record<string, unknown>) => unknown
> = {
  list_roots: () => store.roots,
  tracked_files: (args) => {
    const slug = argString(args, "slug");
    return Object.keys(store.files[slug] ?? {});
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
      status: { kind: "Synced" },
    });
    store.files[slug] = store.files[slug] ?? {};
    emitStatus();
    return slug;
  },
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
      status: { kind: "Synced" },
    });
    store.files[slug] = store.files[slug] ?? {
      "CLAUDE.md": {
        text: `# ${slug}\n`,
        binary: false,
        too_large: false,
      },
    };
    store.linkable = store.linkable.filter((item) => item !== slug);
    emitStatus();
  },
  remove_root: (args) => {
    const slug = argString(args, "slug");
    store.roots = store.roots.filter((row) => row.slug !== slug);
    delete store.files[slug];
    delete store.conflicts[slug];
    emitStatus();
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
      (slug) => !store.roots.some((row) => row.slug === slug),
    ),
  icloud_dir: () => icloudPath(),
  list_gdrive_mounts: () => gdriveMounts(),
  login_item_enabled: () => store.loginItem,
  set_login_item: (args) => {
    store.loginItem = Boolean(args.on);
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
