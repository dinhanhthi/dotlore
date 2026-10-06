import {
  cloudDevices,
  hasUnnamedCloud,
  type KeepAllChoice,
} from "@/lib/conflicts";
import { errorMessage } from "@/lib/errors";
import { BLOCKED } from "@/lib/ipc";
import type { ConflictView, ResolutionDto, ResolveResultDto } from "@/lib/types";

const LIVE_LABEL = "Keep all from this machine" as const;
const CLOUD_LABEL = "Keep all from cloud" as const;

export type QuickResolveCloudChoice = "unnamed" | { deviceId: string };

export type QuickResolveCloudItem = {
  label: string;
  choice: QuickResolveCloudChoice;
  /** Word used in the confirm title. `"cloud"` for the unnamed bucket. */
  device: string;
};

export type QuickResolveMenu = {
  live: { label: typeof LIVE_LABEL };
  cloudMenuLabel: typeof CLOUD_LABEL;
  cloud: QuickResolveCloudItem[];
};

export type QuickResolveCopyInput = {
  keep: "live" | "other";
  /** Live path when `batchSize` is 1. */
  rel?: string;
  /** Cloud device name. `"cloud"` for the unnamed bucket. Omitted: the only named device. */
  device?: string;
  /** Views of the single file, used to count discarded versions. */
  views?: ConflictView[];
  /** Files this choice resolves. `1` is one file; any other count uses the multi-file copy. */
  batchSize: number;
  /** Conflicted files in the scope, compared with `batchSize` for a partial cloud keep. */
  scopeCount: number;
};

function slashed(path: string): string {
  return path.replace(/\\/g, "/");
}

function baseName(rel: string): string {
  return slashed(rel).split("/").pop() ?? rel;
}

function collidingNames(names: string[]): Set<string> {
  const counts = new Map<string, number>();
  for (const name of names) counts.set(name, (counts.get(name) ?? 0) + 1);
  const colliding = new Set<string>();
  for (const [name, count] of counts) {
    if (count > 1) colliding.add(name);
  }
  return colliding;
}

function deviceItemLabel(
  name: string,
  id8: string,
  many: boolean,
  colliding: Set<string>,
): string {
  if (!many) return CLOUD_LABEL;
  return colliding.has(name) ? `${name} (${id8})` : name;
}

/**
 * Menu items for the conflict views in one scope. One cloud choice is a
 * single item; several are per-device items under `cloudMenuLabel`.
 * Built from `cloudDevices` and `hasUnnamedCloud`, so this machine is never
 * listed as a cloud device.
 */
export function quickResolveItems(views: ConflictView[]): QuickResolveMenu {
  const devices = cloudDevices(views);
  const unnamed = hasUnnamedCloud(views);
  const many = devices.length + (unnamed ? 1 : 0) > 1;
  const colliding = collidingNames(devices.map((device) => device.name));
  const cloud: QuickResolveCloudItem[] = devices.map((device) => ({
    label: deviceItemLabel(device.name, device.id8, many, colliding),
    choice: { deviceId: device.id8 },
    device: device.name,
  }));
  if (unnamed) {
    cloud.push({
      label: many ? "Noname cloud" : CLOUD_LABEL,
      choice: "unnamed",
      device: "cloud",
    });
  }
  return {
    live: { label: LIVE_LABEL },
    cloudMenuLabel: CLOUD_LABEL,
    cloud,
  };
}

function otherVersionCount(views: ConflictView[]): number {
  const others = views.filter((view) => !view.loserIsMe).length;
  // A loserIsMe view means the live file is another device's winner.
  if (views.some((view) => view.loserIsMe)) return others + 1;
  return others === 0 ? 1 : others;
}

function cloudDeviceWord(device: string | undefined, views: ConflictView[]): string {
  if (device) return device;
  const named = cloudDevices(views);
  return named.length === 1 ? named[0].name : "cloud";
}

function oneFileCopy(
  input: QuickResolveCopyInput,
): { title: string; description: string; action: string } {
  const name = baseName(input.rel ?? "");
  const views = input.views ?? [];
  if (input.keep === "live") {
    const count = otherVersionCount(views);
    return {
      title: `Keep all from this machine for ${name}?`,
      description:
        count === 1
          ? "Discards the version from the other device."
          : `Discards all ${count} versions from other devices.`,
      action: LIVE_LABEL,
    };
  }
  const device = cloudDeviceWord(input.device, views);
  return {
    title: `Keep all from ${device} for ${name}?`,
    description:
      cloudDevices(views).length > 1
        ? "This machine's version and any other device versions of this file are discarded."
        : "This machine's version is discarded.",
    action: CLOUD_LABEL,
  };
}

function manyFilesCopy(
  input: QuickResolveCopyInput,
): { title: string; description: string; action: string } {
  const { batchSize, scopeCount } = input;
  if (input.keep === "live") {
    return {
      title: `Keep all from this machine for ${batchSize} files?`,
      description: `Discards every other device's version of these ${batchSize} files.`,
      action: LIVE_LABEL,
    };
  }
  const device = cloudDeviceWord(input.device, input.views ?? []);
  const title = `Keep all from ${device} for ${batchSize} files?`;
  if (batchSize < scopeCount) {
    const rest = scopeCount - batchSize;
    return {
      title,
      description: `Resolves ${batchSize} files that have a version from ${device}. Each of those files is fully resolved, so this machine's version and every other device's version of that file are discarded. The other ${rest} conflicted files stay unresolved.`,
      action: CLOUD_LABEL,
    };
  }
  return {
    title,
    description: `Resolves all ${batchSize} conflicted files. Each file is fully resolved: this machine's version and every other device's version of that file are discarded.`,
    action: CLOUD_LABEL,
  };
}

export function quickResolveCopy(
  input: QuickResolveCopyInput,
): { title: string; description: string; action: string } {
  return input.batchSize === 1 ? oneFileCopy(input) : manyFilesCopy(input);
}

/** Snapshot sibling path for `siblingRel`, or `null` when it is gone. */
export function pickSiblingPath(
  dto: ResolutionDto,
  siblingRel: string,
): string | null {
  const wanted = slashed(siblingRel);
  return dto.siblings.find((sibling) => slashed(sibling.path) === wanted)?.path ?? null;
}

/** One file inside a keep-all confirm. `keep` is that file's whole-file resolution. */
export type QuickResolveFile = {
  rel: string;
  keep: "live" | "other";
  siblingRel?: string;
  views: ConflictView[];
};

/**
 * One confirm. A file click is a list of one. `choice` is the menu side;
 * each file's `keep` is what `resolveBinary` writes.
 */
export type QuickResolveTarget = {
  choice: KeepAllChoice;
  device?: string;
  scopeCount: number;
  files: QuickResolveFile[];
};

/** Confirm copy for a target. Wording follows `choice`, not each file's `keep`. */
export function quickResolveTargetCopy(
  target: QuickResolveTarget,
): { title: string; description: string; action: string } | null {
  const file = target.files[0];
  if (!file) return null;
  return quickResolveCopy({
    keep: target.choice === "live" ? "live" : "other",
    rel: file.rel,
    device: target.device,
    views: file.views,
    batchSize: target.files.length,
    scopeCount: target.scopeCount,
  });
}

export type KeepAllApplyFile = {
  rel: string;
  keep: "live" | "other";
  siblingRel?: string;
};

export type KeepAllResult = {
  status: "done" | "stopped" | "blocked" | "busy";
  applied: string[];
  failedRel?: string;
  message?: string;
};

export type KeepAllOps = {
  openResolution: (slug: string, rel: string) => Promise<ResolutionDto>;
  resolveBinary: (
    slug: string,
    rel: string,
    keep: "live" | "other",
    sibling?: string | null,
  ) => Promise<ResolveResultDto | typeof BLOCKED>;
  closeResolution: (slug: string, rel: string) => Promise<void>;
};

const STALE_MESSAGE = "The file changed on another device. Review and try again.";
const PENDING_MESSAGE = "Sync has not finished yet. Try again in a moment.";
const MISSING_MESSAGE = "That version is no longer available. Refresh and try again.";
const BUSY_MESSAGE = "Another keep-all is still running.";

let keepAllBusy = false;

/** Clears the batch flag. A thrown test must not leave the next test busy. */
export function resetKeepAllBusyForTest(): void {
  keepAllBusy = false;
}

function withFile(sentence: string, failedRel: string): string {
  return `${sentence} ${failedRel}`;
}

function stopped(
  applied: string[],
  failedRel: string,
  message: string,
): KeepAllResult {
  return { status: "stopped", applied: [...applied], failedRel, message };
}

/** Files still to resolve. A second confirm retries only these. */
export function keepAllRemainder<T extends { rel: string }>(
  files: readonly T[],
  applied: ReadonlySet<string>,
): T[] {
  return files.filter((file) => !applied.has(file.rel));
}

export type KeepAllDialogAction = {
  message: string | null;
  refresh: boolean;
  resolved: boolean;
  close: boolean;
};

/** What the dialog does after a batch: busy only shows its message. */
export function keepAllDialogAction(result: KeepAllResult): KeepAllDialogAction {
  if (result.status === "busy") {
    return {
      message: result.message ?? BUSY_MESSAGE,
      refresh: false,
      resolved: false,
      close: false,
    };
  }
  const done = result.status === "done";
  return {
    message: result.status === "stopped" ? (result.message ?? null) : null,
    refresh: true,
    resolved: done,
    close: done || result.status === "blocked",
  };
}

/**
 * Resolve each file on its own snapshot: open, resolve, close, then the next.
 * A second caller gets `busy` and does not open a snapshot.
 */
export async function applyKeepAll(
  slug: string,
  files: readonly KeepAllApplyFile[],
  ops: KeepAllOps,
): Promise<KeepAllResult> {
  if (keepAllBusy) return { status: "busy", applied: [], message: BUSY_MESSAGE };
  keepAllBusy = true;
  const applied: string[] = [];
  try {
    for (const file of files) {
      let dto: ResolutionDto;
      try {
        dto = await ops.openResolution(slug, file.rel);
      } catch (cause) {
        return stopped(applied, file.rel, errorMessage(cause, "Could not resolve the conflict"));
      }
      try {
        const early = await resolveOpened(slug, file, dto, ops);
        if (early) return { ...early, applied: [...applied] };
        applied.push(file.rel);
      } catch (cause) {
        return stopped(applied, file.rel, errorMessage(cause, "Could not resolve the conflict"));
      } finally {
        await ops.closeResolution(slug, file.rel).catch(() => {
          // Nothing to release.
        });
      }
    }
    return { status: "done", applied };
  } finally {
    keepAllBusy = false;
  }
}

async function resolveOpened(
  slug: string,
  file: KeepAllApplyFile,
  dto: ResolutionDto,
  ops: KeepAllOps,
): Promise<Omit<KeepAllResult, "applied"> | null> {
  if (file.keep === "live") {
    return classify(await ops.resolveBinary(slug, file.rel, "live"), file.rel);
  }
  const path = file.siblingRel ? pickSiblingPath(dto, file.siblingRel) : null;
  if (path === null) {
    return {
      status: "stopped",
      failedRel: file.rel,
      message: withFile(MISSING_MESSAGE, file.rel),
    };
  }
  return classify(await ops.resolveBinary(slug, file.rel, "other", path), file.rel);
}

function classify(
  result: ResolveResultDto | typeof BLOCKED,
  failedRel: string,
): Omit<KeepAllResult, "applied"> | null {
  if (result === BLOCKED) return { status: "blocked", failedRel };
  if (result.outcome === "applied") return null;
  const sentence = result.outcome === "stale" ? STALE_MESSAGE : PENDING_MESSAGE;
  return { status: "stopped", failedRel, message: withFile(sentence, failedRel) };
}
