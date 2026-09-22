import type { ConflictView, ResolutionDto } from "@/lib/types";

const NOTES_LIVE = `# Daily notes

- Review the Dotlore conflict resolver
- Keep CLAUDE.md short
`;

const NOTES_OTHER = `# Daily notes

Written on the studio Mac.

- Different bullet
- Keep both ideas
`;

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
  sibling?: ConflictView,
): ResolutionDto {
  const liveBytes =
    liveText === null ? (binary ? 4096 : 0) : new TextEncoder().encode(liveText).length;
  const otherText = binary ? null : NOTES_OTHER;
  const otherBytes = otherText === null ? 5120 : new TextEncoder().encode(otherText).length;
  return {
    slug,
    live: rel,
    live_text: liveText,
    binary,
    live_bytes_len: liveBytes,
    siblings: sibling
      ? [
          {
            path: sibling.sibling,
            device_name: sibling.loserName,
            is_me: sibling.loserIsMe,
            text: otherText,
            bytes_len: otherBytes,
          },
        ]
      : [],
  };
}
