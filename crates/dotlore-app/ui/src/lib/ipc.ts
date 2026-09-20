import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import { errorMessage } from "./errors";
import type {
  ConflictView,
  FileContent,
  ResolutionDto,
  ResolveResultDto,
  RootRow,
  StatusPayload,
} from "./types";

export type WorkSnapshot = {
  inflight: number;
  banner: string | null;
};

let inflight = 0;
let banner: string | null = null;
let snapshot: WorkSnapshot = { inflight: 0, banner: null };
const listeners = new Set<() => void>();

function emit(): void {
  snapshot = { inflight, banner };
  for (const listener of listeners) listener();
}

export function subscribeWork(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function getWorkSnapshot(): WorkSnapshot {
  return snapshot;
}

export function setBanner(message: string | null): void {
  if (banner === message) return;
  banner = message;
  emit();
}

/** Count in-flight writes; on failure, set the command-error banner. */
async function run<T>(op: () => Promise<T>): Promise<T> {
  inflight += 1;
  banner = null;
  emit();
  try {
    return await op();
  } catch (err) {
    banner = errorMessage(err, "Something went wrong");
    emit();
    throw err;
  } finally {
    inflight = Math.max(0, inflight - 1);
    emit();
  }
}

export function listRoots(): Promise<RootRow[]> {
  return invoke("list_roots");
}

export function trackedFiles(slug: string): Promise<string[]> {
  return invoke("tracked_files", { slug });
}

export function readFile(slug: string, rel: string): Promise<FileContent> {
  return invoke("read_file", { slug, rel });
}

export function conflicts(slug: string): Promise<ConflictView[]> {
  return invoke("conflicts", { slug });
}

export function openResolution(
  slug: string,
  rel: string,
): Promise<ResolutionDto> {
  return invoke("open_resolution", { slug, rel });
}

export function closeResolution(slug: string, rel: string): Promise<void> {
  return invoke("close_resolution", { slug, rel });
}

export function resolveConflict(
  slug: string,
  rel: string,
  discardSiblings: string[],
  content: string,
): Promise<ResolveResultDto> {
  return run(() =>
    invoke("resolve_conflict", { slug, rel, discardSiblings, content }),
  );
}

/** Keep one side of a binary conflict. Backend discards every sibling. */
export function resolveBinary(
  slug: string,
  rel: string,
  keep: "live" | "other",
  sibling?: string | null,
): Promise<ResolveResultDto> {
  return run(() =>
    invoke("resolve_binary", { slug, rel, keep, sibling: sibling ?? null }),
  );
}

export function providerDir(): Promise<string | null> {
  return invoke("provider_dir");
}

export function gitMissing(): Promise<boolean> {
  return invoke("git_missing");
}

export function syncNow(): Promise<void> {
  return run(() => invoke("sync_now"));
}

export function setProvider(dir: string): Promise<void> {
  return run(() => invoke("set_provider", { dir }));
}

export function addRoot(path: string, slug?: string): Promise<string> {
  return run(() => invoke("add_root", { path, slug: slug ?? null }));
}

export function linkRoot(slug: string, path: string): Promise<void> {
  return run(() => invoke("link_root", { slug, path }));
}

export function removeRoot(slug: string): Promise<void> {
  return run(() => invoke("remove_root", { slug }));
}

export function recoverRoot(slug: string): Promise<void> {
  return run(() => invoke("recover_root", { slug }));
}

export function listLinkable(): Promise<string[]> {
  return invoke("list_linkable");
}

export function icloudDir(): Promise<string> {
  return invoke("icloud_dir");
}

export function listGdriveMounts(): Promise<string[]> {
  return invoke("list_gdrive_mounts");
}

export function loginItemEnabled(): Promise<boolean> {
  return invoke("login_item_enabled");
}

export function setLoginItem(on: boolean): Promise<void> {
  return run(() => invoke("set_login_item", { on }));
}

export function listenStatus(
  handler: (payload: StatusPayload) => void,
): Promise<UnlistenFn> {
  return listen<StatusPayload>("dotlore://status", (event) => {
    handler(event.payload);
  });
}
