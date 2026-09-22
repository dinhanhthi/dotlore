import type { FileContent, FileSync, TrackedFile } from "@/lib/types";

export type FileRecord = {
  text: string | null;
  binary: boolean;
  too_large: boolean;
  bytes?: number;
  state?: FileSync;
};

const DOTLORE_CLAUDE = `# Dotlore

Sync git-ignored AI-agent config between your own Macs through a cloud folder.
`;

const DOTLORE_SETTINGS = `{
  "model": "claude-opus",
  "permissions": {
    "allow": ["Read", "Edit"]
  }
}
`;

const DOTLORE_ARCH = `# Architecture

The engine drives the system git binary. Each tracked root gets a private
staging repo under Application Support.
`;

const MEMLORE_CLAUDE = `# Memlore

A private journal on this Mac.
`;

const NOTES_CLAUDE = `# Daily notes

- Review the Dotlore conflict resolver
- Keep CLAUDE.md short
`;

const NOTES_JOURNAL = `Morning. Styled the sidebar, then the file tree.
`;

const LIMIT_BYTES = 50 * 1024 * 1024;
/** ~60 % of the 50 MB file ceiling — warning (yellow) in the tree. */
const WARNING_BYTES = Math.round(LIMIT_BYTES * 0.6);
/** Well over the 50 MB ceiling — danger (red) / TooLarge. */
const TOO_LARGE_BYTES = 80 * 1024 * 1024;

function textFile(text: string): FileRecord {
  return {
    text,
    binary: false,
    too_large: false,
    bytes: new TextEncoder().encode(text).length,
    state: "Synced",
  };
}

export function demoFiles(): Record<string, Record<string, FileRecord>> {
  return {
    dotlore: {
      "CLAUDE.md": textFile(DOTLORE_CLAUDE),
      ".claude/settings.json": textFile(DOTLORE_SETTINGS),
      "docs/architecture.md": textFile(DOTLORE_ARCH),
      "docs/video.bin": {
        text: null,
        binary: true,
        too_large: false,
        bytes: WARNING_BYTES,
        state: "Synced",
      },
      "docs/dump.bin": {
        text: null,
        binary: true,
        too_large: true,
        bytes: TOO_LARGE_BYTES,
        state: "TooLarge",
      },
    },
    memlore: {
      "CLAUDE.md": textFile(MEMLORE_CLAUDE),
      "AGENTS.md": textFile("Use the Memlore conventions in this repo.\n"),
    },
    notes: {
      "CLAUDE.md": textFile(NOTES_CLAUDE),
      "journal.md": textFile(NOTES_JOURNAL),
      "assets/logo.png": {
        text: null,
        binary: true,
        too_large: false,
        bytes: 4096,
        state: "Synced",
      },
    },
    "missing-proj": {
      "README.md": textFile("This folder is gone on this Mac.\n"),
    },
  };
}

export function toFileContent(record: FileRecord): FileContent {
  const computed =
    record.text === null
      ? record.binary
        ? 4096
        : 0
      : new TextEncoder().encode(record.text).length;
  const bytes = record.bytes ?? (record.too_large ? 2_000_000 : computed);
  return {
    text: record.text,
    binary: record.binary,
    too_large: record.too_large,
    bytes_len: bytes,
  };
}

export function toTrackedFile(rel: string, record: FileRecord): TrackedFile {
  return {
    rel,
    bytes: toFileContent(record).bytes_len,
    state: record.state ?? (record.too_large ? "TooLarge" : "Synced"),
  };
}
