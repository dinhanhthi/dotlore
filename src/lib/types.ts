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
  linked: boolean;
  status: RootStatus;
};

/** Mirrors `commands::LinkableRow` (snake_case fields, no rename). */
export type LinkableRow = {
  slug: string;
  display_name: string;
  is_agent: boolean;
};

/** Mirrors `engine::FileSync` (no rename_all — PascalCase variants). */
export type FileSync = "Synced" | "TooLarge" | "Pending";

/** Mirrors `project::Sensitivity` (`rename_all = "camelCase"`). */
export type Sensitivity = "secret" | "tokenHint";

/** Mirrors `engine::TrackedFile` (snake_case fields, no rename). */
export type TrackedFile = {
  rel: string;
  bytes: number;
  state: FileSync;
  sensitivity: Sensitivity | null;
};

/** Mirrors `engine::EntryKind` (`rename_all = "lowercase"`). */
export type EntryKind = "file" | "directory";

/** Mirrors `engine::EntryView` (`rename_all = "camelCase"`). */
export type EntryView = {
  key: string;
  kind: EntryKind;
  covering: string[];
};

/** Mirrors `commands::PickerRow` (no rename). */
export type PickerRow = {
  name: string;
  kind: string;
  rel: string;
  sensitivity: Sensitivity | null;
};

/** Mirrors `commands::SkippedFileDto` (snake_case fields). */
export type SkippedFileDto = {
  rel: string;
  bytes: number;
};

/** Mirrors `commands::InspectedEntryDto` (snake_case fields). */
export type InspectedEntryDto = {
  kind: EntryKind;
  bytes: number;
  folder_limit: number;
  confirmation_required: boolean;
  skipped_too_large: SkippedFileDto[];
  sensitivity: Sensitivity | null;
  secret_descendants: string[];
  secret_descendants_more: boolean;
};

/** Mirrors `commands::ImportAgentFailureDto` (snake_case fields). */
export type ImportAgentFailureDto = {
  path: string;
  message: string;
};

/** Mirrors `commands::ImportAgentsDto`. */
export type ImportAgentsDto = {
  added: string[];
  failed: ImportAgentFailureDto[];
};

/** Mirrors `commands::TrackResultDto` (`tag = "outcome"`, snake_case). */
export type TrackResultDto =
  | { outcome: "done" }
  | { outcome: "confirm_sensitive"; paths: string[]; more: boolean }
  | {
      outcome: "needs_confirmation";
      bytes: number;
      folder_limit: number;
      confirmation_required: boolean;
      skipped_too_large: SkippedFileDto[];
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
