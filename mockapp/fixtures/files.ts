import type { FileContent, FileSync, TrackedFile } from "@/lib/types";

export type FileRecord = {
  text: string | null;
  binary: boolean;
  too_large: boolean;
  bytes?: number;
  state?: FileSync;
};

const DOTLORE_CLAUDE = `# Dotlore

Sync git-ignored AI-agent config between your own machines through a cloud folder.
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

A private journal on this machine.
`;

const NOTES_CLAUDE = `# Daily notes

Working notes for the Dotlore desktop app. Newest items go at the top of
each section; move finished work to the log at the bottom.

## Today

- Review the Dotlore conflict resolver
- Check the chunk counter on long files
- Keep CLAUDE.md short

## Conventions

- Use pnpm for every script
- Run the mockapp before touching the real app
- Keep Rust and TypeScript types in sync
- Prefer small commits with one-line messages
- Never commit secrets or tokens

## Build

1. Install dependencies with pnpm install
2. Start the mockapp with pnpm mockapp:dev
3. Run pnpm exec vitest run before pushing
4. Build the app with pnpm tauri build

## Review checklist

- Result pane starts empty for each chunk
- Arrows move between chunks
- Keep all copies one side exactly
- Resolve stays disabled until every chunk is picked
- Scroll sync follows the left pane

## Ideas

- Inline blame for each chunk
- Remember the last side picked per file
- Show device names in the tree

## Open questions

- Should Keep all ask for confirmation?
- How should binary files preview?
- Do we need a three-way view later?

## Devices

- studio: desk machine, main build box
- laptop: travel machine, battery tests
- mini: always-on sync host

## Log

- Mon: styled the sidebar
- Tue: file tree and context menu
- Wed: first pass on the resolver
- Thu: fixed scroll sync on long files
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
      ".claude/hooks/format.py": textFile("def run():\n    return 1\n"),
      ".codex/config.toml": textFile('model = "gpt"\n'),
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
      "README.md": textFile("This folder is gone on this machine.\n"),
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

/** Mirrors `project::SECRET_PATTERNS` in `src-tauri/src/project.rs`. */
export const SECRET_PATTERNS: readonly string[] = [
  ".env",
  ".env.*",
  "*.env",
  "*.pem",
  "*.key",
  "*.p12",
  "*.p8",
  "id_rsa*",
  "id_ed25519*",
  ".npmrc",
  ".netrc",
  ".pypirc",
  "credentials*",
  "secrets*",
  "auth.json",
  ".credentials.json",
  "!credentials*.md",
  "!credentials*.txt",
  "!secrets*.md",
  "!secrets*.txt",
];

function globRegex(glob: string): RegExp {
  const body = glob
    .split("")
    .map((ch) => (ch === "*" ? "[^/]*" : ch === "?" ? "[^/]" : ch.replace(/[\\^$.|+()[\]{}]/g, "\\$&")))
    .join("");
  return new RegExp(`^${body}$`);
}

/**
 * A small gitignore-like matcher: a slash-free pattern matches a name at any
 * depth, a pattern with a slash is anchored to the root, a trailing `/` matches
 * a folder (so every file under it), `!` negates, and the last match wins. The
 * path itself is checked before its parent folders, as in Rust.
 */
function secretMatch(rel: string, patterns: readonly string[]): boolean {
  const parts = rel.split("/");
  for (let depth = parts.length; depth > 0; depth -= 1) {
    const isDir = depth < parts.length;
    let hit: boolean | null = null;
    for (const raw of patterns) {
      const negate = raw.startsWith("!");
      let body = negate ? raw.slice(1) : raw;
      const dirOnly = body.endsWith("/");
      if (dirOnly) body = body.slice(0, -1);
      if (body.length === 0 || (dirOnly && !isDir)) continue;
      const anchored = body.includes("/");
      const subject = anchored ? parts.slice(0, depth).join("/") : parts[depth - 1];
      if (globRegex(body.replace(/^\//, "")).test(subject)) hit = !negate;
    }
    if (hit !== null) return hit;
  }
  return false;
}

export function mockSensitivity(
  rel: string,
  patterns: readonly string[] = SECRET_PATTERNS,
): TrackedFile["sensitivity"] {
  if (secretMatch(rel, patterns)) return "secret";
  const name = rel.split("/").at(-1) ?? rel;
  return name === ".mcp.json" ? "tokenHint" : null;
}

export function toTrackedFile(
  rel: string,
  record: FileRecord,
  patterns: readonly string[] = SECRET_PATTERNS,
): TrackedFile {
  return {
    rel,
    bytes: toFileContent(record).bytes_len,
    state: record.state ?? (record.too_large ? "TooLarge" : "Synced"),
    sensitivity: mockSensitivity(rel, patterns),
  };
}
