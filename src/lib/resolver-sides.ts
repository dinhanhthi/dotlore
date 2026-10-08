import type { ResolutionDto } from "./types";

/** Label of the live file when it is not this machine's version: what every device now has. */
export const SYNCED_LABEL = "synced version";

/** One version the resolver can show. `source` is `null` for the live file, else a sibling path. */
export type ResolverSide = {
  source: string | null;
  label: string;
  text: string | null;
  bytesLen: number;
};

/** This machine's version and the cloud versions it is compared against. */
export type ResolverSides = {
  mine: ResolverSide;
  cloud: ResolverSide[];
};

/**
 * Split a resolution into this machine's side and the cloud's.
 *
 * The merge keeps the winner in the live file and the loser in a sibling, so
 * when this machine lost (`is_me`), its bytes are that sibling and the live
 * file is another device's version.
 */
export function resolverSides(dto: ResolutionDto): ResolverSides {
  const live: ResolverSide = {
    source: null,
    label: SYNCED_LABEL,
    text: dto.live_text,
    bytesLen: dto.live_bytes_len,
  };
  const siblings: ResolverSide[] = dto.siblings.map((s) => ({
    source: s.path,
    label: s.device_name,
    text: s.text,
    bytesLen: s.bytes_len,
  }));
  const index = dto.siblings.findIndex((s) => s.is_me);
  if (index < 0) {
    return { mine: { ...live, label: "this machine" }, cloud: siblings };
  }
  const mine = { ...siblings[index], label: "this machine" };
  // A second loss on the same path leaves another `is_me` sibling.
  const rest = siblings
    .map((side, i) => (dto.siblings[i].is_me ? { ...side, label: "this machine (other copy)" } : side))
    .filter((_, i) => i !== index);
  return { mine, cloud: [live, ...rest] };
}

/**
 * Title of the column opposite this machine: the device a sibling came from,
 * or the synced live file. Both versions are in the cloud, so never "cloud".
 */
export function otherTitle(side: ResolverSide): string {
  return side.source === null ? side.label : `from ${side.label}`;
}

/** The sibling file a comparison of `mine` against `other` would discard. */
export function pairSibling(mine: ResolverSide, other: ResolverSide): string | null {
  return other.source ?? mine.source;
}

/** `resolve_binary` arguments that keep `side`. */
export function keepArgs(side: ResolverSide): { keep: "live" | "other"; sibling: string | null } {
  return side.source === null
    ? { keep: "live", sibling: null }
    : { keep: "other", sibling: side.source };
}
