import type { ConflictView, ResolutionDto } from "@/lib/types";

const NOTES_OTHER = `# Daily notes (studio)

Written on the studio machine.

## Today

- Write the release notes
- Different bullet, keep both ideas

## Conventions

- Use pnpm for every script
- Run the mockapp before touching the real app
- Keep Rust and TypeScript types in sync
- Format with the project Prettier config
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
- Thu: tested on the studio machine`;

const JOURNAL_STUDIO = `Morning. Styled the sidebar, then the file tree.
Afternoon on the studio machine: context menu for conflicts.
`;

const JOURNAL_LAPTOP = `Morning on the laptop. Styled the sidebar only.
`;

/** Other-side text per sibling path; anything else falls back to `NOTES_OTHER`. */
const SIBLING_TEXT: Record<string, string> = {
  "journal.conflict-studio-5e6f7a8b.md": JOURNAL_STUDIO,
  "journal.conflict-laptop-9c0d1e2f.md": JOURNAL_LAPTOP,
};

export function demoConflicts(): Record<string, ConflictView[]> {
  return {
    notes: [
      {
        live: "CLAUDE.md",
        sibling: "CLAUDE.conflict-studio-a1b2c3d4.md",
        loserId8: "a1b2c3d4",
        loserName: "studio",
        loserIsMe: false,
      },
      {
        live: "journal.md",
        sibling: "journal.conflict-studio-5e6f7a8b.md",
        loserId8: "5e6f7a8b",
        loserName: "studio",
        loserIsMe: false,
      },
      {
        live: "journal.md",
        sibling: "journal.conflict-laptop-9c0d1e2f.md",
        loserId8: "9c0d1e2f",
        loserName: "laptop",
        loserIsMe: false,
      },
      {
        live: "assets/logo.png",
        sibling: "assets/logo.conflict-studio-deadbeef.png",
        loserId8: "deadbeef",
        loserName: "studio",
        loserIsMe: false,
      },
    ],
  };
}

export function demoResolution(
  slug: string,
  rel: string,
  liveText: string | null,
  binary: boolean,
  siblings: ConflictView[],
): ResolutionDto {
  const liveBytes =
    liveText === null ? (binary ? 4096 : 0) : new TextEncoder().encode(liveText).length;
  return {
    slug,
    live: rel,
    live_text: liveText,
    binary,
    live_bytes_len: liveBytes,
    siblings: siblings.map((sibling) => {
      const otherText = binary ? null : (SIBLING_TEXT[sibling.sibling] ?? NOTES_OTHER);
      const otherBytes =
        otherText === null ? 5120 : new TextEncoder().encode(otherText).length;
      return {
        path: sibling.sibling,
        device_name: sibling.loserName,
        is_me: sibling.loserIsMe,
        text: otherText,
        bytes_len: otherBytes,
      };
    }),
  };
}
