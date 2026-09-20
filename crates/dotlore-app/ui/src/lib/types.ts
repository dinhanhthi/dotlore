/** Mirrors `engine::RootStatus` (`#[serde(tag = "kind", content = "detail")]`). */
export type RootStatus =
  | { kind: "Synced" }
  | { kind: "Conflicts"; detail: number }
  | { kind: "Pending" }
  | { kind: "RootMissing" }
  | { kind: "GitMissing" }
  | { kind: "Error"; detail: string };

/** Mirrors `state::RootRow` (snake_case fields, no rename). */
export type RootRow = {
  slug: string;
  path: string;
  name: string;
  is_agent: boolean;
  status: RootStatus;
};

/** Mirrors `engine::ConflictView` (`rename_all = "camelCase"`). */
export type ConflictView = {
  live: string;
  sibling: string;
  loserId8: string;
  loserName: string;
  loserIsMe: boolean;
};

/** Mirrors `commands::FileContent` (snake_case fields). */
export type FileContent = {
  text: string | null;
  binary: boolean;
  too_large: boolean;
  bytes_len: number;
};

/** Mirrors `state::StatusPayload`. */
export type StatusPayload = {
  roots: RootRow[];
  error: string | null;
};

/** Mirrors `commands::SiblingDto` (snake_case fields). */
export type SiblingDto = {
  path: string;
  device_name: string;
  is_me: boolean;
  text: string | null;
  bytes_len: number;
};

/** Mirrors `commands::ResolutionDto` (snake_case fields). */
export type ResolutionDto = {
  slug: string;
  live: string;
  live_text: string | null;
  binary: boolean;
  live_bytes_len: number;
  siblings: SiblingDto[];
};

/** Mirrors `commands::ResolveResultDto` (`tag = "outcome"`, lowercase). */
export type ResolveResultDto =
  | { outcome: "applied" }
  | { outcome: "stale"; refreshed: ResolutionDto }
  | { outcome: "pending" };
