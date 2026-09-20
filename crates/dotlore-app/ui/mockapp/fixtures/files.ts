import type { FileContent } from "@/lib/types";

export type FileRecord = {
  text: string | null;
  binary: boolean;
  too_large: boolean;
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

\`dotlore-core\` drives the system git binary. Each tracked root gets a private
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

export function demoFiles(): Record<string, Record<string, FileRecord>> {
  return {
    dotlore: {
      "CLAUDE.md": { text: DOTLORE_CLAUDE, binary: false, too_large: false },
      ".claude/settings.json": {
        text: DOTLORE_SETTINGS,
        binary: false,
        too_large: false,
      },
      "docs/architecture.md": {
        text: DOTLORE_ARCH,
        binary: false,
        too_large: false,
      },
    },
    memlore: {
      "CLAUDE.md": { text: MEMLORE_CLAUDE, binary: false, too_large: false },
      "AGENTS.md": {
        text: "Use the Memlore conventions in this repo.\n",
        binary: false,
        too_large: false,
      },
    },
    notes: {
      "CLAUDE.md": { text: NOTES_CLAUDE, binary: false, too_large: false },
      "journal.md": { text: NOTES_JOURNAL, binary: false, too_large: false },
      "assets/logo.png": { text: null, binary: true, too_large: false },
    },
    "missing-proj": {
      "README.md": {
        text: "This folder is gone on this Mac.\n",
        binary: false,
        too_large: false,
      },
    },
  };
}

export function toFileContent(record: FileRecord): FileContent {
  const bytes =
    record.text === null
      ? record.binary
        ? 4096
        : 0
      : new TextEncoder().encode(record.text).length;
  return {
    text: record.text,
    binary: record.binary,
    too_large: record.too_large,
    bytes_len: record.too_large ? 2_000_000 : bytes,
  };
}
