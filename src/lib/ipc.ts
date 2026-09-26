import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import { errorMessage } from "./errors";
import type {
  ConflictView,
  EntryView,
  FileContent,
  ImportAgentsDto,
  InspectedEntryDto,
  LinkableRow,
  PickerRow,
  ResolutionDto,
  ResolveResultDto,
  RootRow,
  StatusPayload,
  TrackedFile,
  TrackResultDto,
} from "./types";

/** A pending question before a track batch can continue. */
export type TrackConfirm =
  | { kind: "folder_limit"; rel: string; bytes: number; folderLimit: number }
  | {
      kind: "sensitive";
      rels: string[];
      paths: string[];
      more: boolean;
      canSkip: boolean;
    };

/** Folder-limit prompts answer only `"all"` (Track) or `"cancel"`. */
export type TrackConfirmAnswer = "all" | "skip" | "cancel";

/** Secret paths listed in one sensitive prompt. */
const SENSITIVE_PATH_CAP = 20;

/** One command failure in this session's log. */
export type ErrorLogEntry = { id: number; message: string; at: number };

/** Entries kept in the session log; the oldest drop first. */
const ERROR_LOG_CAP = 50;

export type WorkSnapshot = {
  inflight: number;
  syncing: number;
  banner: string | null;
  /** A background task with its own footer copy. Never counts toward `inflight`. */
  taskLabel: string | null;
  trackConfirm: TrackConfirm | null;
  /** This session's command failures, newest first. */
  errors: ErrorLogEntry[];
};

let inflight = 0;
let syncing = 0;
let banner: string | null = null;
let taskLabel: string | null = null;
let trackConfirm: TrackConfirm | null = null;
let confirmResolve: ((answer: TrackConfirmAnswer) => void) | null = null;
let errorLog: ErrorLogEntry[] = [];
let nextErrorId = 1;
let snapshot: WorkSnapshot = {
  inflight: 0,
  syncing: 0,
  banner: null,
  taskLabel: null,
  trackConfirm: null,
  errors: [],
};
const listeners = new Set<() => void>();

function emit(): void {
  snapshot = {
    inflight,
    syncing,
    banner,
    taskLabel,
    trackConfirm,
    errors: errorLog,
  };
  for (const listener of listeners) listener();
}

function setTaskLabel(label: string | null): void {
  if (taskLabel === label) return;
  taskLabel = label;
  emit();
}

function waitForTrackConfirm(next: TrackConfirm): Promise<TrackConfirmAnswer> {
  trackConfirm = next;
  emit();
  return new Promise((resolve) => {
    confirmResolve = resolve;
  });
}

/** Resolve the paused track confirmation. A second call is a no-op. */
export function answerTrackConfirm(answer: TrackConfirmAnswer): void {
  const resolve = confirmResolve;
  confirmResolve = null;
  trackConfirm = null;
  emit();
  resolve?.(answer);
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

/** Log a command failure and show it in the banner. */
export function reportError(message: string): void {
  const entry = { id: nextErrorId++, message, at: Date.now() };
  errorLog = [entry, ...errorLog].slice(0, ERROR_LOG_CAP);
  banner = message;
  emit();
}

export function dismissError(id: number): void {
  errorLog = errorLog.filter((entry) => entry.id !== id);
  emit();
}

export function clearErrors(): void {
  errorLog = [];
  emit();
}

/**
 * Count in-flight writes; on failure, set the command-error banner. A banner a
 * finished task left behind is not cleared here — the toast dismisses itself.
 */
async function run<T>(op: () => Promise<T>): Promise<T> {
  inflight += 1;
  emit();
  try {
    return await op();
  } catch (err) {
    reportError(errorMessage(err, "Something went wrong"));
    throw err;
  } finally {
    inflight = Math.max(0, inflight - 1);
    emit();
  }
}

/**
 * Returned by [`runTask`] when another task already holds the slot.
 *
 * A symbol, not `null`: Tauri serializes a command returning `()` as `null`,
 * so a `null` sentinel makes a successful void write indistinguishable from
 * a refused one and the caller skips its follow-up refresh.
 */
export const BLOCKED = Symbol("blocked");

/**
 * Run a write in the background: the footer shows `label` with a spinner and
 * the rest of the window stays usable. [`BLOCKED`] when another task already
 * holds the slot.
 */
export async function runTask<T>(
  label: string,
  op: () => Promise<T>,
): Promise<T | typeof BLOCKED> {
  if (taskLabel !== null || confirmResolve !== null) {
    setBanner("Wait for the current task to finish");
    return BLOCKED;
  }
  setTaskLabel(label);
  try {
    return await op();
  } catch (err) {
    reportError(errorMessage(err, "Something went wrong"));
    throw err;
  } finally {
    setTaskLabel(null);
  }
}

export function listRoots(): Promise<RootRow[]> {
  return invoke("list_roots");
}

export function trackedFiles(slug: string): Promise<TrackedFile[]> {
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
): Promise<ResolveResultDto | typeof BLOCKED> {
  return runTask(`Resolving ${rel}…`, () =>
    invoke<ResolveResultDto>("resolve_conflict", {
      slug,
      rel,
      discardSiblings,
      content,
    }),
  );
}

/** Keep one side of a binary conflict. Backend discards every sibling. */
export function resolveBinary(
  slug: string,
  rel: string,
  keep: "live" | "other",
  sibling?: string | null,
): Promise<ResolveResultDto | typeof BLOCKED> {
  return runTask(`Resolving ${rel}…`, () =>
    invoke<ResolveResultDto>("resolve_binary", {
      slug,
      rel,
      keep,
      sibling: sibling ?? null,
    }),
  );
}

export function providerDir(): Promise<string | null> {
  return invoke("provider_dir");
}

/** The provider's `dotlore` folder, or the provider before the first publish. */
export function openCloudFolder(): Promise<void> {
  return invoke("open_cloud_folder");
}

/** The state dir this app runs on (`DOTLORE_HOME`). */
export function appHome(): Promise<string> {
  return invoke("app_home");
}

export function openAppHome(): Promise<void> {
  return invoke("open_app_home");
}

export function gitMissing(): Promise<boolean> {
  return invoke("git_missing");
}

export function syncNow(): Promise<void> {
  syncing += 1;
  emit();
  return run(() => invoke<void>("sync_now")).finally(() => {
    syncing = Math.max(0, syncing - 1);
    emit();
  });
}

export function setProvider(dir: string): Promise<void | typeof BLOCKED> {
  return runTask("Switching cloud folder…", () =>
    invoke<void>("set_provider", { dir }),
  );
}

/**
 * Register a project. Does not take the global busy lock — the file tree
 * shows its own seeding state while pattern files are copied.
 */
export function addRoot(path: string, slug?: string): Promise<string> {
  return invoke("add_root", { path, slug: slug ?? null });
}

export function importInstalledAgents(): Promise<ImportAgentsDto | typeof BLOCKED> {
  return runTask("Looking for agent folders…", () =>
    invoke<ImportAgentsDto>("import_installed_agents"),
  );
}

export function linkRoot(slug: string, path: string): Promise<void | typeof BLOCKED> {
  return runTask(`Linking ${slug}…`, () =>
    invoke<void>("link_root", { slug, path }),
  );
}

export function removeRoot(slug: string): Promise<void | typeof BLOCKED> {
  return runTask(`Removing ${slug}…`, () =>
    invoke<void>("remove_root", { slug }),
  );
}

export type WipeReport = {
  readded: string[];
  failed: { slug: string; error: string }[];
};

export const WIPE_LABEL = "Wiping cloud data…";

export function wipeCloudData(): Promise<WipeReport | typeof BLOCKED> {
  return runTask(WIPE_LABEL, () =>
    invoke<WipeReport>("wipe_cloud_data"),
  );
}

export function recoverRoot(slug: string): Promise<void | typeof BLOCKED> {
  return runTask(`Rebuilding ${slug}…`, () =>
    invoke<void>("recover_root", { slug }),
  );
}

export function listLinkable(): Promise<LinkableRow[]> {
  return invoke("list_linkable");
}

export function listEntries(slug: string): Promise<EntryView[]> {
  return invoke("list_entries", { slug });
}

export function listEntryChildren(
  slug: string,
  rel: string,
): Promise<PickerRow[]> {
  return invoke("list_entry_children", { slug, rel });
}

export function inspectEntry(
  slug: string,
  rel: string,
): Promise<InspectedEntryDto> {
  return invoke("inspect_entry", { slug, rel });
}

export function trackEntry(
  slug: string,
  rel: string,
  options: { confirmedFolderBytes?: number | null; confirmedSensitive?: boolean } = {},
): Promise<TrackResultDto> {
  return run(() =>
    invoke("track_entry", {
      slug,
      rel,
      confirmedFolderBytes: options.confirmedFolderBytes ?? null,
      confirmedSensitive: options.confirmedSensitive ?? false,
    }),
  );
}

export function untrackEntry(
  slug: string,
  rel: string,
): Promise<EntryView[] | typeof BLOCKED> {
  return runTask(`Untracking ${rel}…`, () =>
    invoke<EntryView[]>("untrack_entry", { slug, rel }),
  );
}

export type TrackBatchOp = {
  rel: string;
  action: "track" | "untrack";
  confirmedFolderBytes?: number | null;
};

/**
 * Apply staged track/untrack marks without the global busy lock.
 * The footer reads `taskLabel` while this runs.
 */
export async function applyTrackBatch(
  slug: string,
  ops: TrackBatchOp[],
): Promise<void> {
  if (ops.length === 0 || taskLabel !== null || confirmResolve !== null) return;
  setBanner(null);
  setTaskLabel("Updating tracked files…");
  const declined: string[] = [];
  const stillOver: string[] = [];
  let firstError: string | null = null;
  let cancelled = false;
  let runOps = ops;
  let confirmedOps = new Set<TrackBatchOp>();
  try {
    try {
      const sensitive = new Set<TrackBatchOp>();
      const paths: string[] = [];
      let more = false;
      for (const op of ops) {
        if (op.action !== "track") continue;
        const info = await inspectEntry(slug, op.rel);
        if (info.sensitivity === "secret") {
          sensitive.add(op);
          paths.push(op.rel);
        } else if (info.secret_descendants.length > 0) {
          sensitive.add(op);
          paths.push(...info.secret_descendants);
        }
        if (info.secret_descendants_more) more = true;
      }
      if (sensitive.size > 0) {
        const unique = [...new Set(paths)].sort();
        setTaskLabel("Confirm tracking sensitive files…");
        const answer = await waitForTrackConfirm({
          kind: "sensitive",
          rels: ops.filter((op) => sensitive.has(op)).map((op) => op.rel),
          paths: unique.slice(0, SENSITIVE_PATH_CAP),
          more: more || unique.length > SENSITIVE_PATH_CAP,
          canSkip: ops.some((op) => !sensitive.has(op)),
        });
        if (answer === "cancel") cancelled = true;
        else if (answer === "skip") {
          runOps = ops.filter((op) => !sensitive.has(op));
          declined.push(...ops.filter((op) => sensitive.has(op)).map((op) => op.rel));
        } else confirmedOps = sensitive;
      }
    } catch (err) {
      firstError = errorMessage(err, "Something went wrong");
    }
    for (let i = 0; !cancelled && firstError === null && i < runOps.length; i++) {
      const op = runOps[i]!;
      const verb = op.action === "track" ? "Tracking" : "Untracking";
      const progress = runOps.length > 1 ? ` (${i + 1} of ${runOps.length})` : "";
      setTaskLabel(`${verb} ${op.rel}…${progress}`);
      try {
        if (op.action === "untrack") {
          await invoke("untrack_entry", { slug, rel: op.rel });
          continue;
        }
        let confirmedFolderBytes = op.confirmedFolderBytes ?? null;
        let confirmedSensitive = confirmedOps.has(op);
        let confirmedFolder = false;
        for (;;) {
          const result = await invoke<TrackResultDto>("track_entry", {
            slug,
            rel: op.rel,
            confirmedFolderBytes,
            confirmedSensitive,
          });
          if (result.outcome === "done") break;
          if (result.outcome === "confirm_sensitive") {
            if (confirmedSensitive) throw new Error(`Could not track ${op.rel}`);
            setTaskLabel(`Confirm tracking ${op.rel}…${progress}`);
            const answer = await waitForTrackConfirm({
              kind: "sensitive",
              rels: [op.rel],
              paths: result.paths,
              more: result.more,
              canSkip: false,
            });
            if (answer !== "all") {
              declined.push(op.rel);
              break;
            }
            confirmedSensitive = true;
            continue;
          }
          if (confirmedFolder) {
            stillOver.push(op.rel);
            break;
          }
          setTaskLabel(`Confirm tracking ${op.rel}…${progress}`);
          const answer = await waitForTrackConfirm({
            kind: "folder_limit",
            rel: op.rel,
            bytes: result.bytes,
            folderLimit: result.folder_limit,
          });
          if (answer !== "all") {
            declined.push(op.rel);
            break;
          }
          confirmedFolderBytes = result.bytes;
          confirmedFolder = true;
        }
      } catch (err) {
        firstError = errorMessage(err, "Something went wrong");
        break;
      }
    }
    if (cancelled) {
      setBanner("Nothing was changed");
    } else if (firstError !== null) {
      reportError(firstError);
    } else if (declined.length > 0 || stillOver.length > 0) {
      const parts: string[] = [];
      if (declined.length === 1) parts.push(`${declined[0]} was not tracked`);
      else if (declined.length > 1) {
        parts.push(`${declined.length} items were not tracked`);
      }
      if (stillOver.length === 1) {
        parts.push(`${stillOver[0]} is over the folder limit and was not tracked`);
      } else if (stillOver.length > 1) {
        parts.push(
          `${stillOver.length} folders are over the limit and were not tracked`,
        );
      }
      setBanner(parts.join(". "));
    }
  } finally {
    confirmResolve = null;
    trackConfirm = null;
    taskLabel = null;
    emit();
  }
}

export function defaultPatterns(): Promise<string[]> {
  return invoke("default_patterns");
}

export function setDefaultPatterns(patterns: string[]): Promise<void> {
  return run(() => invoke("set_default_patterns", { patterns }));
}

export function defaultIgnore(): Promise<string> {
  return invoke("default_ignore");
}

export function setDefaultIgnore(ignore: string): Promise<void> {
  return run(() => invoke("set_default_ignore", { ignore }));
}

export type PatternCatalog = {
  id: string;
  label: string;
  lines: string[];
};

export function patternCatalogs(): Promise<PatternCatalog[]> {
  return invoke("pattern_catalogs");
}

export function setPatternCatalog(
  catalog: string,
  patterns: string[],
): Promise<void> {
  return run(() => invoke("set_pattern_catalog", { catalog, patterns }));
}

export function sensitivePatterns(): Promise<string[]> {
  return invoke("sensitive_patterns");
}

export function setSensitivePatterns(patterns: string[]): Promise<void> {
  return run(() => invoke("set_sensitive_patterns", { patterns }));
}

export function maxFileMb(): Promise<number> {
  return invoke("max_file_mb");
}

export function setMaxFileMb(mb: number): Promise<void> {
  return run(() => invoke("set_max_file_mb", { mb }));
}

export function maxSeedFolderMb(): Promise<number> {
  return invoke("max_seed_folder_mb");
}

export function setMaxSeedFolderMb(mb: number): Promise<void> {
  return run(() => invoke("set_max_seed_folder_mb", { mb }));
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

/** `dotlore://update-progress`, from `updater.rs`. */
export type UpdateProgress =
  | { phase: "downloading"; percent: number | null }
  | { phase: "installing" }
  | { phase: "idle" };

/** The version the last update check found, or null. */
export function updateAvailable(): Promise<string | null> {
  return invoke("update_available");
}

/** Open the native install prompt for the update already found. */
export function promptUpdate(): Promise<void> {
  return invoke("prompt_update");
}

export function listenUpdate(
  handler: (version: string | null) => void,
): Promise<UnlistenFn> {
  return listen<string | null>("dotlore://update", (event) => {
    handler(event.payload);
  });
}

export function listenUpdateProgress(
  handler: (progress: UpdateProgress) => void,
): Promise<UnlistenFn> {
  return listen<UpdateProgress>("dotlore://update-progress", (event) => {
    handler(event.payload);
  });
}
