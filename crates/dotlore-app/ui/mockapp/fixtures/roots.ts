import type { LinkableRow, RootRow } from "@/lib/types";

export const ICLOUD_PROVIDER =
  "/Users/demo/Library/Mobile Documents/com~apple~CloudDocs/dotlore";

export const GDRIVE_MOUNTS = [
  "/Users/demo/Library/CloudStorage/GoogleDrive-demo",
];

export const LINKABLE_ROWS: LinkableRow[] = [
  { slug: "old-mac-notes", display_name: "Old Mac Notes", is_agent: false },
];

export function demoRoots(): RootRow[] {
  return [
    {
      slug: "dotlore",
      path: "/Users/demo/git/dotlore",
      name: "dotlore",
      is_agent: true,
      linked: true,
      status: { kind: "Synced" },
    },
    {
      slug: "memlore",
      path: "/Users/demo/git/memlore",
      name: "memlore",
      is_agent: false,
      linked: true,
      status: { kind: "Synced" },
    },
    {
      slug: "notes",
      path: "/Users/demo/Notes",
      name: "Notes",
      is_agent: false,
      linked: true,
      status: { kind: "Conflicts", detail: 2 },
    },
    {
      slug: "missing-proj",
      path: "/Users/demo/gone/project",
      name: "missing-proj",
      is_agent: false,
      linked: true,
      status: { kind: "RootMissing" },
    },
    {
      slug: LINKABLE_ROWS[0]!.slug,
      path: "",
      name: LINKABLE_ROWS[0]!.display_name,
      is_agent: LINKABLE_ROWS[0]!.is_agent,
      linked: false,
      status: { kind: "Pending" },
    },
  ];
}
