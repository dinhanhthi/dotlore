import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type {
  ConflictView,
  FileContent,
  RootRow,
  StatusPayload,
} from "./types";

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

export function providerDir(): Promise<string | null> {
  return invoke("provider_dir");
}

export function gitMissing(): Promise<boolean> {
  return invoke("git_missing");
}

export function syncNow(): Promise<void> {
  return invoke("sync_now");
}

export function listenStatus(
  handler: (payload: StatusPayload) => void,
): Promise<UnlistenFn> {
  return listen<StatusPayload>("dotlore://status", (event) => {
    handler(event.payload);
  });
}
