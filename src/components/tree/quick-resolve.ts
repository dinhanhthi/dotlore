import type { ConflictView, ResolutionDto } from "@/lib/types";

export type QuickResolveCloudItem = {
  label: string;
  siblingRel: string;
  device: string;
};

function slashed(path: string): string {
  return path.replace(/\\/g, "/");
}

function baseName(rel: string): string {
  return slashed(rel).split("/").pop() ?? rel;
}

/**
 * Menu items for one conflicted file. One sibling labels the cloud item
 * inline; several become per-device items under a "Keep cloud" submenu.
 */
export function quickResolveItems(views: ConflictView[]): {
  live: { label: "Keep this machine" };
  cloud: QuickResolveCloudItem[];
} {
  const single = views.length === 1;
  return {
    live: { label: "Keep this machine" },
    cloud: views.map((view) => ({
      label: single ? `Keep cloud (${view.loserName})` : view.loserName,
      siblingRel: slashed(view.sibling),
      device: view.loserName,
    })),
  };
}

export function quickResolveCopy(
  keep: "live" | "other",
  rel: string,
  views: ConflictView[],
  device?: string,
): { title: string; description: string; action: string } {
  const name = baseName(rel);
  if (keep === "live") {
    const count = views.length;
    return {
      title: `Keep this machine's version of ${name}?`,
      description:
        count === 1
          ? "Discards the version from the other device."
          : `Discards all ${count} versions from other devices.`,
      action: "Keep this machine",
    };
  }
  const from = device ?? views[0]?.loserName ?? "the cloud";
  return {
    title: `Keep ${from}'s version of ${name}?`,
    description:
      views.length > 1
        ? "This machine's version and any other device versions are discarded."
        : "This machine's version is discarded.",
    action: "Keep cloud",
  };
}

/** Snapshot sibling path for `siblingRel`, or `null` when it is gone. */
export function pickSiblingPath(
  dto: ResolutionDto,
  siblingRel: string,
): string | null {
  const wanted = slashed(siblingRel);
  return dto.siblings.find((sibling) => slashed(sibling.path) === wanted)?.path ?? null;
}
